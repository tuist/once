//! Atomic editor for `once.toml` manifests.
//!
//! Callers describe their edit as a list of [`EditOperation`]s
//! (`create`, `update`, `delete`) against a single manifest source
//! string. [`apply_operations`] runs the whole list through
//! `toml_edit`, preserving formatting and comments where it can, and
//! returns either the new manifest body or a list of structured
//! [`Diagnostic`]s explaining what went wrong.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};
use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, Value};

use crate::graph::{built_in_target_kind_schemas_result, Diagnostic, TargetKindSchema};
use crate::target_validator::validate_target;

/// One mutation against the `[[target]]` array in a `once.toml`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EditOperation {
    /// Append a new `[[target]]` block. Fails if a target with the
    /// same name already exists in the manifest.
    Create { target: TargetSpec },
    /// Replace the named fields of an existing `[[target]]`. Fields
    /// left unset (`None`) are preserved as-is.
    Update {
        target_name: String,
        #[serde(default)]
        set: TargetUpdate,
    },
    /// Remove the named `[[target]]`.
    Delete { target_name: String },
}

/// Full description of a target as the editor will write it.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TargetSpec {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub deps: Vec<String>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub srcs: Vec<String>,
    #[serde(default)]
    pub attrs: JsonMap<String, JsonValue>,
}

/// Partial update for an existing target. Any field left as `None`
/// keeps its current value; setting `attrs` to `Some(map)` replaces
/// the whole attrs table (callers needing a merge must read, merge,
/// then call update with the merged map).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TargetUpdate {
    pub name: Option<String>,
    pub kind: Option<String>,
    pub deps: Option<Vec<String>>,
    pub dependencies: Option<BTreeMap<String, Vec<String>>>,
    pub srcs: Option<Vec<String>>,
    pub attrs: Option<JsonMap<String, JsonValue>>,
}

/// Apply the operations against `toml_src` and return the resulting
/// manifest body. Any operation failure aborts the whole batch and
/// returns diagnostics; the input string is never partially modified.
pub fn apply_operations(
    toml_src: &str,
    operations: &[EditOperation],
) -> Result<String, Vec<Diagnostic>> {
    let schemas =
        built_in_target_kind_schemas_result().map_err(|err| schema_load_diagnostic(&err))?;
    apply_operations_with_schemas(toml_src, operations, &schemas)
}

/// Apply operations using the caller-provided target kind schema set.
pub fn apply_operations_with_schemas(
    toml_src: &str,
    operations: &[EditOperation],
    schemas: &[TargetKindSchema],
) -> Result<String, Vec<Diagnostic>> {
    let mut doc: DocumentMut = toml_src.parse().map_err(|err: toml_edit::TomlError| {
        vec![Diagnostic::new(
            "toml_parse_error",
            format!("could not parse `once.toml`: {err}"),
        )]
    })?;

    ensure_target_array(&mut doc)?;

    for op in operations {
        apply_one(&mut doc, op, schemas)?;
    }
    Ok(doc.to_string())
}

fn ensure_target_array(doc: &mut DocumentMut) -> Result<(), Vec<Diagnostic>> {
    if !doc.contains_key("target") {
        doc.insert("target", Item::ArrayOfTables(ArrayOfTables::new()));
        return Ok(());
    }
    if doc["target"].as_array_of_tables().is_none() {
        return Err(vec![Diagnostic::new(
            "invalid_target_section",
            "the top-level `target` key must be an array of tables (`[[target]]`)",
        )
        .with_repair(
            "delete the conflicting `target` value and let the editor re-create it as `[[target]]`",
        )]);
    }
    Ok(())
}

fn apply_one(
    doc: &mut DocumentMut,
    op: &EditOperation,
    schemas: &[TargetKindSchema],
) -> Result<(), Vec<Diagnostic>> {
    match op {
        EditOperation::Create { target } => create(doc, target, schemas),
        EditOperation::Update { target_name, set } => update(doc, target_name, set, schemas),
        EditOperation::Delete { target_name } => delete(doc, target_name),
    }
}

fn create(
    doc: &mut DocumentMut,
    spec: &TargetSpec,
    schemas: &[TargetKindSchema],
) -> Result<(), Vec<Diagnostic>> {
    if spec.name.trim().is_empty() {
        return Err(vec![Diagnostic::new(
            "target_name_required",
            "`create` operations must declare a non-empty `name`",
        )
        .with_attribute("name")]);
    }
    if spec.kind.trim().is_empty() {
        return Err(vec![Diagnostic::new(
            "target_kind_required",
            "`create` operations must declare a non-empty `kind`",
        )
        .with_target(spec.name.as_str())
        .with_attribute("kind")]);
    }
    if find_target_index(doc, &spec.name).is_some() {
        return Err(vec![Diagnostic::new(
            "target_already_exists",
            format!("a target named `{}` already exists", spec.name),
        )
        .with_target(spec.name.as_str())
        .with_repair(format!(
            "rename the new target, or `update` the existing `{}`",
            spec.name
        ))]);
    }

    validate_spec(spec, schemas)?;

    let table = build_target_table(spec)?;
    targets_mut(doc).push(table);
    Ok(())
}

fn update(
    doc: &mut DocumentMut,
    target_name: &str,
    set: &TargetUpdate,
    schemas: &[TargetKindSchema],
) -> Result<(), Vec<Diagnostic>> {
    let Some(index) = find_target_index(doc, target_name) else {
        return Err(vec![Diagnostic::new(
            "target_not_found",
            format!("no target named `{target_name}` in this manifest"),
        )
        .with_target(target_name)]);
    };

    if let Some(new_name) = set.name.as_deref() {
        if new_name.trim().is_empty() {
            return Err(vec![Diagnostic::new(
                "target_name_required",
                "`update.set.name` must be non-empty when present",
            )
            .with_target(target_name)
            .with_attribute("name")]);
        }
        if new_name != target_name && find_target_index(doc, new_name).is_some() {
            return Err(vec![Diagnostic::new(
                "target_already_exists",
                format!("renaming to `{new_name}` would clash with an existing target"),
            )
            .with_target(target_name)
            .with_attribute("name")]);
        }
    }

    let table = targets_mut(doc)
        .get_mut(index)
        .expect("find_target_index returned a valid index");
    let mut updated = table.clone();
    apply_update(&mut updated, set, target_name)?;
    validate_spec(&target_spec_from_table(&updated), schemas)?;
    *table = updated;
    Ok(())
}

fn validate_spec(spec: &TargetSpec, schemas: &[TargetKindSchema]) -> Result<(), Vec<Diagnostic>> {
    let diagnostics = validate_target(spec, schemas);
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

fn schema_load_diagnostic(err: &crate::Error) -> Vec<Diagnostic> {
    vec![Diagnostic::new("target_kind_schema_load_failed", err.to_string()).with_attribute("kind")]
}

fn delete(doc: &mut DocumentMut, target_name: &str) -> Result<(), Vec<Diagnostic>> {
    let Some(index) = find_target_index(doc, target_name) else {
        return Err(vec![Diagnostic::new(
            "target_not_found",
            format!("no target named `{target_name}` to delete"),
        )
        .with_target(target_name)]);
    };
    targets_mut(doc).remove(index);
    Ok(())
}

fn apply_update(
    table: &mut Table,
    set: &TargetUpdate,
    original_name: &str,
) -> Result<(), Vec<Diagnostic>> {
    if let Some(new_name) = &set.name {
        table.insert("name", Item::Value(Value::from(new_name.as_str())));
    }
    if let Some(new_kind) = &set.kind {
        if new_kind.trim().is_empty() {
            return Err(vec![Diagnostic::new(
                "target_kind_required",
                "`update.set.kind` must be non-empty when present",
            )
            .with_target(original_name)
            .with_attribute("kind")]);
        }
        table.insert("kind", Item::Value(Value::from(new_kind.as_str())));
    }
    if let Some(deps) = &set.deps {
        table.insert("deps", Item::Value(Value::Array(string_array(deps))));
    }
    if let Some(dependencies) = &set.dependencies {
        table.insert(
            "dependencies",
            Item::Table(build_dependencies_table(dependencies)),
        );
    }
    if let Some(srcs) = &set.srcs {
        table.insert("srcs", Item::Value(Value::Array(string_array(srcs))));
    }
    if let Some(attrs) = &set.attrs {
        let attrs_table = build_attrs_table(attrs, original_name)?;
        table.insert("attrs", Item::Table(attrs_table));
    }
    Ok(())
}

fn build_target_table(spec: &TargetSpec) -> Result<Table, Vec<Diagnostic>> {
    let mut table = Table::new();
    table.set_implicit(false);
    table.insert("name", Item::Value(Value::from(spec.name.as_str())));
    table.insert("kind", Item::Value(Value::from(spec.kind.as_str())));
    if !spec.deps.is_empty() {
        table.insert("deps", Item::Value(Value::Array(string_array(&spec.deps))));
    }
    if !spec.dependencies.is_empty() {
        table.insert(
            "dependencies",
            Item::Table(build_dependencies_table(&spec.dependencies)),
        );
    }
    if !spec.srcs.is_empty() {
        table.insert("srcs", Item::Value(Value::Array(string_array(&spec.srcs))));
    }
    if !spec.attrs.is_empty() {
        let attrs_table = build_attrs_table(&spec.attrs, &spec.name)?;
        table.insert("attrs", Item::Table(attrs_table));
    }
    Ok(table)
}

fn target_spec_from_table(table: &Table) -> TargetSpec {
    TargetSpec {
        name: table
            .get("name")
            .and_then(Item::as_str)
            .unwrap_or_default()
            .to_string(),
        kind: table
            .get("kind")
            .and_then(Item::as_str)
            .unwrap_or_default()
            .to_string(),
        deps: string_vec_from_item(table.get("deps")),
        dependencies: dependencies_from_item(table.get("dependencies")),
        srcs: string_vec_from_item(table.get("srcs")),
        attrs: attrs_from_item(table.get("attrs")),
    }
}

fn build_dependencies_table(dependencies: &BTreeMap<String, Vec<String>>) -> Table {
    let mut table = Table::new();
    table.set_implicit(false);
    for (name, values) in dependencies {
        table.insert(name, Item::Value(Value::Array(string_array(values))));
    }
    table
}

fn dependencies_from_item(item: Option<&Item>) -> BTreeMap<String, Vec<String>> {
    let Some(Item::Table(table)) = item else {
        return BTreeMap::new();
    };
    table
        .iter()
        .map(|(name, item)| (name.to_string(), string_vec_from_item(Some(item))))
        .collect()
}

fn string_vec_from_item(item: Option<&Item>) -> Vec<String> {
    item.and_then(Item::as_array)
        .map(|array| {
            array
                .iter()
                .filter_map(|value| value.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn attrs_from_item(item: Option<&Item>) -> JsonMap<String, JsonValue> {
    let Some(Item::Table(table)) = item else {
        return JsonMap::new();
    };
    table
        .iter()
        .filter_map(|(key, value)| json_from_item(value).map(|value| (key.to_string(), value)))
        .collect()
}

fn json_from_item(item: &Item) -> Option<JsonValue> {
    match item {
        Item::Value(value) => json_from_value(value),
        Item::Table(table) => Some(JsonValue::Object(
            table
                .iter()
                .filter_map(|(key, value)| {
                    json_from_item(value).map(|value| (key.to_string(), value))
                })
                .collect(),
        )),
        _ => None,
    }
}

fn json_from_value(value: &Value) -> Option<JsonValue> {
    if let Some(value) = value.as_bool() {
        return Some(JsonValue::Bool(value));
    }
    if let Some(value) = value.as_integer() {
        return Some(JsonValue::Number(value.into()));
    }
    if let Some(value) = value.as_float() {
        return serde_json::Number::from_f64(value).map(JsonValue::Number);
    }
    if let Some(value) = value.as_str() {
        return Some(JsonValue::String(value.to_string()));
    }
    if let Some(array) = value.as_array() {
        return Some(JsonValue::Array(
            array.iter().filter_map(json_from_value).collect(),
        ));
    }
    if let Some(table) = value.as_inline_table() {
        return Some(JsonValue::Object(
            table
                .iter()
                .filter_map(|(key, value)| {
                    json_from_value(value).map(|value| (key.to_string(), value))
                })
                .collect(),
        ));
    }
    None
}

fn build_attrs_table(
    attrs: &JsonMap<String, JsonValue>,
    target_name: &str,
) -> Result<Table, Vec<Diagnostic>> {
    let mut attrs_table = Table::new();
    attrs_table.set_implicit(false);
    let mut sorted: BTreeMap<&String, &JsonValue> = BTreeMap::new();
    for (key, value) in attrs {
        sorted.insert(key, value);
    }
    for (key, value) in sorted {
        let item = json_to_item(value).map_err(|err| {
            vec![Diagnostic::new(
                "attr_unrepresentable",
                format!("attribute `{key}` cannot be written to TOML: {err}"),
            )
            .with_target(target_name)
            .with_attribute(key.as_str())]
        })?;
        attrs_table.insert(key, item);
    }
    Ok(attrs_table)
}

fn string_array(values: &[String]) -> Array {
    let mut array = Array::new();
    for value in values {
        array.push(value.as_str());
    }
    array
}

fn json_to_item(value: &JsonValue) -> Result<Item, String> {
    Ok(match value {
        JsonValue::Null => return Err("`null` has no TOML representation".to_string()),
        JsonValue::Bool(b) => Item::Value(Value::from(*b)),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Item::Value(Value::from(i))
            } else if let Some(f) = n.as_f64() {
                Item::Value(Value::from(f))
            } else {
                return Err(format!("number `{n}` is out of range"));
            }
        }
        JsonValue::String(s) => Item::Value(Value::from(s.as_str())),
        JsonValue::Array(items) => {
            let mut array = Array::new();
            for item in items {
                let nested = json_to_value(item)?;
                array.push(nested);
            }
            Item::Value(Value::Array(array))
        }
        JsonValue::Object(map) => {
            let mut table = toml_edit::InlineTable::new();
            for (key, value) in map {
                let nested = json_to_value(value)?;
                table.insert(key, nested);
            }
            Item::Value(Value::InlineTable(table))
        }
    })
}

fn json_to_value(value: &JsonValue) -> Result<Value, String> {
    match json_to_item(value)? {
        Item::Value(v) => Ok(v),
        _ => Err("expected scalar or inline value".to_string()),
    }
}

fn find_target_index(doc: &DocumentMut, name: &str) -> Option<usize> {
    let targets = doc.get("target")?.as_array_of_tables()?;
    targets.iter().position(|t| {
        t.get("name")
            .and_then(Item::as_str)
            .is_some_and(|n| n == name)
    })
}

fn targets_mut(doc: &mut DocumentMut) -> &mut ArrayOfTables {
    doc["target"]
        .as_array_of_tables_mut()
        .expect("ensure_target_array guarantees the array exists")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn target(name: &str, kind: &str) -> TargetSpec {
        let mut spec = TargetSpec {
            name: name.to_string(),
            kind: kind.to_string(),
            ..Default::default()
        };
        spec.attrs.insert("platform".to_string(), json!("ios"));
        spec
    }

    #[test]
    fn create_appends_target_to_empty_manifest() {
        let out = apply_operations(
            "",
            &[EditOperation::Create {
                target: target("Hello", "apple_library"),
            }],
        )
        .expect("create");
        assert!(out.contains("[[target]]"));
        assert!(out.contains("name = \"Hello\""));
        assert!(out.contains("kind = \"apple_library\""));
    }

    #[test]
    fn create_preserves_existing_targets() {
        let src = r#"
[[target]]
name = "Existing"
kind = "apple_library"
"#;
        let out = apply_operations(
            src,
            &[EditOperation::Create {
                target: target("Hello", "apple_library"),
            }],
        )
        .expect("create");
        assert!(out.contains("name = \"Existing\""));
        assert!(out.contains("name = \"Hello\""));
    }

    #[test]
    fn create_serializes_attrs_in_sorted_order() {
        let mut spec = target("Hello", "apple_library");
        spec.attrs.insert("platform".to_string(), json!("ios"));
        spec.attrs.insert("minimum_os".to_string(), json!("17.0"));
        let out = apply_operations("", &[EditOperation::Create { target: spec }]).expect("create");
        let min = out.find("minimum_os").expect("minimum_os present");
        let plat = out.find("platform").expect("platform present");
        // Sorted: minimum_os comes before platform alphabetically.
        assert!(
            min < plat,
            "attrs should serialize in sorted order, got:\n{out}"
        );
    }

    #[test]
    fn create_and_update_serialize_named_dependency_roles() {
        let mut spec = TargetSpec {
            name: "Library".to_string(),
            kind: "rust_library".to_string(),
            ..Default::default()
        };
        spec.dependencies
            .insert("proc_macro_deps".to_string(), vec!["./derive".to_string()]);
        let created = apply_operations("", &[EditOperation::Create { target: spec }])
            .expect("create named dependency role");
        assert!(created.contains("[target.dependencies]"));
        assert!(created.contains("proc_macro_deps = [\"./derive\"]"));

        let updated = apply_operations(
            &created,
            &[EditOperation::Update {
                target_name: "Library".to_string(),
                set: TargetUpdate {
                    dependencies: Some(BTreeMap::from([(
                        "link_deps".to_string(),
                        vec!["./native".to_string()],
                    )])),
                    ..Default::default()
                },
            }],
        )
        .expect("update named dependency role");
        assert!(updated.contains("link_deps = [\"./native\"]"));
        assert!(!updated.contains("proc_macro_deps"));
    }

    #[test]
    fn create_rejects_duplicate_target_names() {
        let src = r#"
[[target]]
name = "Hello"
kind = "apple_library"
"#;
        let diagnostics = apply_operations(
            src,
            &[EditOperation::Create {
                target: target("Hello", "apple_library"),
            }],
        )
        .expect_err("duplicate must fail");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "target_already_exists");
        assert_eq!(diagnostics[0].target.as_deref(), Some("Hello"));
    }

    #[test]
    fn create_rejects_empty_name() {
        let mut spec = target("", "apple_library");
        spec.attrs.insert("platform".to_string(), json!("ios"));
        let diagnostics = apply_operations("", &[EditOperation::Create { target: spec }])
            .expect_err("missing name must fail");
        assert_eq!(diagnostics[0].code, "target_name_required");
        assert_eq!(diagnostics[0].attribute.as_deref(), Some("name"));
    }

    #[test]
    fn create_validates_against_target_kind_schema() {
        let mut spec = target("Hello", "apple_library");
        spec.attrs.clear();
        let diagnostics = apply_operations("", &[EditOperation::Create { target: spec }])
            .expect_err("missing platform must fail");
        assert_eq!(diagnostics[0].code, "missing_required_attr");
        assert_eq!(diagnostics[0].attribute.as_deref(), Some("platform"));
    }

    #[test]
    fn update_replaces_only_set_fields() {
        let src = r#"
[[target]]
name = "Hello"
kind = "apple_library"
srcs = ["Sources/**/*.swift"]

[target.attrs]
platform = "ios"
"#;
        let out = apply_operations(
            src,
            &[EditOperation::Update {
                target_name: "Hello".to_string(),
                set: TargetUpdate {
                    deps: Some(vec!["./Other".to_string()]),
                    ..Default::default()
                },
            }],
        )
        .expect("update");
        assert!(out.contains("deps = [\"./Other\"]"));
        assert!(out.contains("srcs = [\"Sources/**/*.swift\"]"));
        assert!(out.contains("platform = \"ios\""));
    }

    #[test]
    fn update_can_rename_target() {
        let src = r#"
[[target]]
name = "Old"
kind = "apple_library"

[target.attrs]
platform = "ios"
"#;
        let out = apply_operations(
            src,
            &[EditOperation::Update {
                target_name: "Old".to_string(),
                set: TargetUpdate {
                    name: Some("New".to_string()),
                    ..Default::default()
                },
            }],
        )
        .expect("update rename");
        assert!(out.contains("name = \"New\""));
        assert!(!out.contains("name = \"Old\""));
    }

    #[test]
    fn update_replaces_attrs_table_when_set() {
        let src = r#"
[[target]]
name = "Hello"
kind = "apple_library"

[target.attrs]
platform = "ios"
minimum_os = "17.0"
"#;
        let out = apply_operations(
            src,
            &[EditOperation::Update {
                target_name: "Hello".to_string(),
                set: TargetUpdate {
                    attrs: Some({
                        let mut m = JsonMap::new();
                        m.insert("platform".to_string(), json!("macos"));
                        m
                    }),
                    ..Default::default()
                },
            }],
        )
        .expect("update attrs");
        assert!(out.contains("platform = \"macos\""));
        assert!(!out.contains("minimum_os"));
    }

    #[test]
    fn update_validates_merged_target_against_target_kind_schema() {
        let src = r#"
[[target]]
name = "Hello"
kind = "apple_library"

[target.attrs]
platform = "ios"
"#;
        let diagnostics = apply_operations(
            src,
            &[EditOperation::Update {
                target_name: "Hello".to_string(),
                set: TargetUpdate {
                    attrs: Some(JsonMap::new()),
                    ..Default::default()
                },
            }],
        )
        .expect_err("removing required platform must fail");
        assert_eq!(diagnostics[0].code, "missing_required_attr");
        assert_eq!(diagnostics[0].attribute.as_deref(), Some("platform"));
    }

    #[test]
    fn update_rejects_renaming_into_existing_target() {
        let src = r#"
[[target]]
name = "A"
kind = "apple_library"

[[target]]
name = "B"
kind = "apple_library"
"#;
        let diagnostics = apply_operations(
            src,
            &[EditOperation::Update {
                target_name: "A".to_string(),
                set: TargetUpdate {
                    name: Some("B".to_string()),
                    ..Default::default()
                },
            }],
        )
        .expect_err("rename clash must fail");
        assert_eq!(diagnostics[0].code, "target_already_exists");
    }

    #[test]
    fn update_target_not_found_diagnoses_with_scope() {
        let diagnostics = apply_operations(
            "",
            &[EditOperation::Update {
                target_name: "Missing".to_string(),
                set: TargetUpdate::default(),
            }],
        )
        .expect_err("missing target must fail");
        assert_eq!(diagnostics[0].code, "target_not_found");
        assert_eq!(diagnostics[0].target.as_deref(), Some("Missing"));
    }

    #[test]
    fn delete_removes_target() {
        let src = r#"
[[target]]
name = "Hello"
kind = "apple_library"

[[target]]
name = "Keep"
kind = "apple_library"
"#;
        let out = apply_operations(
            src,
            &[EditOperation::Delete {
                target_name: "Hello".to_string(),
            }],
        )
        .expect("delete");
        assert!(!out.contains("name = \"Hello\""));
        assert!(out.contains("name = \"Keep\""));
    }

    #[test]
    fn delete_target_not_found_diagnoses() {
        let diagnostics = apply_operations(
            "",
            &[EditOperation::Delete {
                target_name: "Missing".to_string(),
            }],
        )
        .expect_err("delete missing must fail");
        assert_eq!(diagnostics[0].code, "target_not_found");
    }

    #[test]
    fn rejects_non_array_target_key() {
        let src = "target = \"oops\"";
        let diagnostics = apply_operations(
            src,
            &[EditOperation::Create {
                target: target("Hello", "apple_library"),
            }],
        )
        .expect_err("non-array target must fail");
        assert_eq!(diagnostics[0].code, "invalid_target_section");
    }

    #[test]
    fn batch_operations_apply_in_order() {
        let src = r#"
[[target]]
name = "Old"
kind = "apple_library"
"#;
        let out = apply_operations(
            src,
            &[
                EditOperation::Delete {
                    target_name: "Old".to_string(),
                },
                EditOperation::Create {
                    target: target("New", "apple_library"),
                },
            ],
        )
        .expect("batch");
        assert!(!out.contains("name = \"Old\""));
        assert!(out.contains("name = \"New\""));
    }

    #[test]
    fn batch_failure_does_not_partially_mutate_the_input() {
        // Note: apply_operations returns either Ok(new_string) or Err; we
        // never see a partial mutation because the caller still holds the
        // original `toml_src`. This test pins that behavior.
        let src = r#"
[[target]]
name = "Hello"
kind = "apple_library"
"#;
        let diagnostics = apply_operations(
            src,
            &[
                EditOperation::Delete {
                    target_name: "Hello".to_string(),
                },
                EditOperation::Delete {
                    target_name: "Hello".to_string(),
                },
            ],
        )
        .expect_err("second delete fails");
        assert_eq!(diagnostics[0].code, "target_not_found");
    }

    #[test]
    fn deserializes_edit_operation_from_json() {
        let raw = json!({
            "op": "create",
            "target": {
                "name": "Hello",
                "kind": "apple_library",
                "srcs": ["Sources/**/*.swift"],
                "attrs": { "platform": "ios" }
            }
        });
        let op: EditOperation = serde_json::from_value(raw).expect("deserialize");
        match op {
            EditOperation::Create { target } => {
                assert_eq!(target.name, "Hello");
                assert_eq!(target.kind, "apple_library");
                assert_eq!(target.srcs, vec!["Sources/**/*.swift".to_string()]);
                assert_eq!(target.attrs.get("platform"), Some(&json!("ios")));
            }
            other => panic!("expected Create, got {other:?}"),
        }
    }
}
