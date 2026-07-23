//! Schema-only validation for a single `[[target]]` table.
//!
//! Given a [`TargetSpec`] (the shape the editor produces) and the
//! workspace's target kind registry, [`validate_target`] returns a list of
//! [`Diagnostic`]s. An empty list means the target shape matches the
//! target kind's declared contract. The check is local: it does not resolve
//! dep references or read other manifests.

use serde_json::Value as JsonValue;

use crate::graph::{AttrSchema, Diagnostic, TargetKindSchema};
use crate::manifest_editor::TargetSpec;
use crate::target_ref::validate_target_name;

/// Validate `target` against `schemas`. Returns an empty `Vec` if the
/// target's shape is acceptable to its target kind.
#[must_use]
pub fn validate_target(target: &TargetSpec, schemas: &[TargetKindSchema]) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    if !validate_target_identity(target, &mut diagnostics) {
        return diagnostics;
    }
    validate_target_visibility(target, &mut diagnostics);

    let Some(schema) = schemas.iter().find(|s| s.kind == target.kind) else {
        diagnostics.push(
            Diagnostic::new(
                "unknown_target_kind",
                format!("target kind `{}` is not registered", target.kind),
            )
            .with_target(target.name.as_str())
            .with_attribute("kind")
            .with_repair(suggest_known_kinds(schemas)),
        );
        return diagnostics;
    };

    validate_dependency_roles(target, schema, &mut diagnostics);
    validate_required_attrs(target, schema, &mut diagnostics);

    for (attr_name, attr_value) in &target.attrs {
        let Some(attr_schema) = schema.attrs.iter().find(|a| &a.name == attr_name) else {
            diagnostics.push(
                Diagnostic::new(
                    "unknown_attr",
                    format!(
                        "target kind `{}` does not declare attribute `{}`",
                        schema.kind, attr_name
                    ),
                )
                .with_target(target.name.as_str())
                .with_attribute(attr_name.as_str())
                .with_repair(suggest_known_attrs(schema)),
            );
            continue;
        };
        validate_attr(target, attr_name, attr_value, attr_schema, &mut diagnostics);
    }

    diagnostics
}

fn validate_target_identity(target: &TargetSpec, diagnostics: &mut Vec<Diagnostic>) -> bool {
    if target.name.trim().is_empty() {
        diagnostics.push(
            Diagnostic::new("target_name_required", "target `name` must not be empty")
                .with_attribute("name"),
        );
    } else if let Err(err) = validate_target_name(&target.name) {
        diagnostics.push(
            Diagnostic::new("invalid_target_name", err.to_string())
                .with_target(target.name.as_str())
                .with_attribute("name"),
        );
    }

    if target.kind.trim().is_empty() {
        diagnostics.push(
            Diagnostic::new("target_kind_required", "target `kind` must not be empty")
                .with_target(target.name.as_str())
                .with_attribute("kind"),
        );
        return false;
    }
    true
}

fn validate_target_visibility(target: &TargetSpec, diagnostics: &mut Vec<Diagnostic>) {
    for entry in &target.visibility {
        if crate::workspace_validator::visibility_entry_is_valid("", entry) {
            continue;
        }
        diagnostics.push(
            Diagnostic::new(
                "invalid_visibility",
                format!("target `{}` has invalid visibility entry `{entry}`", target.name),
            )
            .with_target(target.name.as_str())
            .with_attribute("visibility")
            .with_repair(
                "Use `public`, `private`, an exact target such as `tools/Generator`, a package entry such as `package:apps`, or a subtree entry such as `subtree:apps`",
            ),
        );
    }
}

fn validate_dependency_roles(
    target: &TargetSpec,
    schema: &TargetKindSchema,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for role in target.dependencies.keys() {
        if role == "deps" || !schema.deps.iter().any(|edge| &edge.name == role) {
            diagnostics.push(
                Diagnostic::new(
                    "unknown_dependency_role",
                    format!(
                        "target kind `{}` does not declare dependency role `{role}`",
                        schema.kind
                    ),
                )
                .with_target(target.name.as_str())
                .with_attribute(format!("dependencies.{role}"))
                .with_repair(suggest_known_dependency_roles(schema)),
            );
        }
    }
}

fn validate_required_attrs(
    target: &TargetSpec,
    schema: &TargetKindSchema,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for attr_schema in &schema.attrs {
        if attr_schema.required && !target.attrs.contains_key(&attr_schema.name) {
            diagnostics.push(
                Diagnostic::new(
                    "missing_required_attr",
                    format!(
                        "target kind `{}` requires attribute `{}`",
                        schema.kind, attr_schema.name
                    ),
                )
                .with_target(target.name.as_str())
                .with_attribute(attr_schema.name.as_str())
                .with_repair(format!(
                    "Set `target.attrs.{}` to a value of type `{}`",
                    attr_schema.name, attr_schema.ty
                )),
            );
        }
    }
}

fn suggest_known_dependency_roles(schema: &TargetKindSchema) -> String {
    let roles = schema
        .deps
        .iter()
        .map(|edge| edge.name.as_str())
        .filter(|name| *name != "deps")
        .collect::<Vec<_>>();
    if roles.is_empty() {
        return "Remove the named dependency role; this target kind declares none".to_string();
    }
    format!("Use one of: {}", roles.join(", "))
}

fn validate_attr(
    target: &TargetSpec,
    attr_name: &str,
    attr_value: &JsonValue,
    attr_schema: &AttrSchema,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !attr_schema.implemented {
        diagnostics.push(
            Diagnostic::new(
                "unimplemented_attr",
                format!(
                    "target kind `{}` declares attribute `{attr_name}` for discovery, but does not implement it yet",
                    target.kind
                ),
            )
            .with_target(target.name.as_str())
            .with_attribute(attr_name)
            .with_repair(format!(
                "Remove `target.attrs.{attr_name}` until the target kind implements it"
            )),
        );
        return;
    }
    if let Some(branches) = select_branches(attr_value) {
        validate_select_attr(target, attr_name, attr_schema, branches, diagnostics);
    } else if let Err(err) = check_type(attr_value, &attr_schema.ty) {
        diagnostics.push(type_mismatch(target, attr_name, &attr_schema.ty, &err));
    }
}

fn validate_select_attr(
    target: &TargetSpec,
    attr_name: &str,
    attr_schema: &AttrSchema,
    branches: &serde_json::Map<String, JsonValue>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !attr_schema.configurable {
        diagnostics.push(
            Diagnostic::new(
                "select_on_non_configurable_attr",
                format!("attribute `{attr_name}` is not configurable but uses `select()`"),
            )
            .with_target(target.name.as_str())
            .with_attribute(attr_name)
            .with_repair(format!(
                "Replace the select value with one `{}` value",
                attr_schema.ty
            )),
        );
        return;
    }
    for (branch, branch_value) in branches {
        if let Err(err) = check_type(branch_value, &attr_schema.ty) {
            diagnostics.push(type_mismatch(
                target,
                attr_name,
                &attr_schema.ty,
                &format!("select branch `{branch}`: {err}"),
            ));
        }
    }
}

fn type_mismatch(target: &TargetSpec, attr_name: &str, ty: &str, err: &str) -> Diagnostic {
    Diagnostic::new(
        "attr_type_mismatch",
        format!("attribute `{attr_name}` expects type `{ty}`: {err}"),
    )
    .with_target(target.name.as_str())
    .with_attribute(attr_name)
    .with_repair(format!(
        "Set `target.attrs.{attr_name}` to a value of type `{ty}`"
    ))
}

fn select_branches(value: &JsonValue) -> Option<&serde_json::Map<String, JsonValue>> {
    let JsonValue::Object(map) = value else {
        return None;
    };
    if map.len() != 1 {
        return None;
    }
    let Some(JsonValue::Object(branches)) = map.get("select") else {
        return None;
    };
    Some(branches)
}

fn check_type(value: &JsonValue, ty: &str) -> Result<(), String> {
    let ty = ty.trim();
    if let Some(inner) = strip_wrapper(ty, "list<") {
        let JsonValue::Array(items) = value else {
            return Err(format!("expected an array, got {}", json_type(value)));
        };
        for (index, item) in items.iter().enumerate() {
            check_type(item, inner).map_err(|err| format!("element [{index}]: {err}"))?;
        }
        return Ok(());
    }
    if let Some(inner) = strip_wrapper(ty, "map<") {
        let JsonValue::Object(entries) = value else {
            return Err(format!("expected a table, got {}", json_type(value)));
        };
        let (key_ty, value_ty) = split_map_params(inner)?;
        if key_ty != "string" {
            return Err(format!(
                "map key type `{key_ty}` is not supported; TOML keys are always strings",
            ));
        }
        for (key, sub) in entries {
            check_type(sub, value_ty).map_err(|err| format!("entry `{key}`: {err}"))?;
        }
        return Ok(());
    }
    match ty {
        "string" | "target" => match value {
            JsonValue::String(_) => Ok(()),
            _ => Err(format!("expected a string, got {}", json_type(value))),
        },
        "bool" => match value {
            JsonValue::Bool(_) => Ok(()),
            _ => Err(format!("expected a bool, got {}", json_type(value))),
        },
        "int" | "integer" => match value {
            JsonValue::Number(n) if n.is_i64() => Ok(()),
            _ => Err(format!("expected an integer, got {}", json_type(value))),
        },
        "float" => match value {
            JsonValue::Number(n) if n.is_f64() => Ok(()),
            _ => Err(format!(
                "expected a floating-point number, got {}",
                json_type(value)
            )),
        },
        other => Err(format!(
            "unknown attribute type `{other}` declared by target kind schema"
        )),
    }
}

fn strip_wrapper<'a>(ty: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = ty.strip_prefix(prefix)?;
    rest.strip_suffix('>')
}

fn split_map_params(inner: &str) -> Result<(&str, &str), String> {
    let mut parts = inner.splitn(2, ',');
    let key = parts
        .next()
        .ok_or_else(|| "missing map key type".to_string())?
        .trim();
    let value = parts
        .next()
        .ok_or_else(|| format!("map type `map<{inner}>` is missing the value type"))?
        .trim();
    Ok((key, value))
}

fn json_type(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "bool",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "table",
    }
}

fn suggest_known_kinds(schemas: &[TargetKindSchema]) -> String {
    let kinds: Vec<&str> = schemas.iter().map(|s| s.kind.as_str()).collect();
    format!("known target kinds: {}", kinds.join(", "))
}

fn suggest_known_attrs(schema: &TargetKindSchema) -> String {
    if schema.attrs.is_empty() {
        return format!("target kind `{}` declares no attributes", schema.kind);
    }
    let names: Vec<&str> = schema
        .attrs
        .iter()
        .map(|a: &AttrSchema| a.name.as_str())
        .collect();
    format!(
        "attributes declared by target kind `{}`: {}",
        schema.kind,
        names.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::built_in_target_kind_schemas;
    use serde_json::json;
    use serde_json::Map as JsonMap;

    fn schema_named(kind: &str) -> TargetKindSchema {
        built_in_target_kind_schemas()
            .into_iter()
            .find(|s| s.kind == kind)
            .expect("target kind exists")
    }

    #[test]
    fn unimplemented_attributes_fail_static_validation() {
        let mut target = target("Library", "rust_library");
        target
            .attrs
            .insert("doc_deps".to_string(), json!(["DocumentationOnly"]));

        let diagnostics = validate_target(&target, &built_in_target_kind_schemas());
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "unimplemented_attr")
            .expect("unimplemented attribute diagnostic");

        assert_eq!(diagnostic.attribute.as_deref(), Some("doc_deps"));
        assert!(diagnostic.repairs[0].contains("Remove"));
        assert!(schema_named("rust_library")
            .attrs
            .iter()
            .find(|attr| attr.name == "doc_deps")
            .is_some_and(|attr| !attr.implemented));
    }

    fn target(name: &str, kind: &str) -> TargetSpec {
        TargetSpec {
            name: name.to_string(),
            kind: kind.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn missing_required_attr_produces_diagnostic_with_attribute_scope() {
        let schemas = vec![schema_named("apple_library")];
        // apple_library requires `platform`.
        let target = target("Hello", "apple_library");
        let diagnostics = validate_target(&target, &schemas);
        let missing = diagnostics
            .iter()
            .find(|d| {
                d.code == "missing_required_attr" && d.attribute.as_deref() == Some("platform")
            })
            .expect("missing platform diagnostic");
        assert_eq!(missing.target.as_deref(), Some("Hello"));
        assert!(missing.repairs[0].contains("target.attrs.platform"));
        assert!(missing.repairs[0].contains("string"));
    }

    #[test]
    fn unknown_kind_produces_diagnostic_with_repair() {
        let schemas = built_in_target_kind_schemas();
        let target = target("Hello", "mystery_kind");
        let diagnostics = validate_target(&target, &schemas);
        let unknown = diagnostics
            .iter()
            .find(|d| d.code == "unknown_target_kind")
            .expect("unknown_target_kind diagnostic");
        assert!(!unknown.repairs.is_empty());
        assert!(unknown.repairs[0].contains("apple_library"));
    }

    #[test]
    fn unknown_attr_lists_known_attrs_as_repair() {
        let schemas = built_in_target_kind_schemas();
        let mut t = target("Hello", "apple_library");
        t.attrs.insert("platform".to_string(), json!("ios"));
        t.attrs.insert("wat".to_string(), json!("nope"));
        let diagnostics = validate_target(&t, &schemas);
        let unknown = diagnostics
            .iter()
            .find(|d| d.code == "unknown_attr" && d.attribute.as_deref() == Some("wat"))
            .expect("unknown wat diagnostic");
        assert!(unknown.repairs[0].contains("platform"));
    }

    #[test]
    fn named_dependency_roles_validate_against_the_selected_kind() {
        let schemas = built_in_target_kind_schemas();
        let mut valid = target("Library", "rust_library");
        valid
            .dependencies
            .insert("proc_macro_deps".to_string(), vec!["./derive".to_string()]);
        assert!(validate_target(&valid, &schemas).is_empty());

        valid
            .dependencies
            .insert("compiler_plugins".to_string(), vec!["./plugin".to_string()]);
        let diagnostics = validate_target(&valid, &schemas);
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "unknown_dependency_role")
            .expect("unknown dependency role diagnostic");
        assert_eq!(
            diagnostic.attribute.as_deref(),
            Some("dependencies.compiler_plugins")
        );
        assert!(diagnostic.repairs[0].contains("proc_macro_deps"));
        assert!(diagnostic.repairs[0].contains("link_deps"));
    }

    #[test]
    fn type_mismatch_diagnoses_with_scope() {
        let schemas = built_in_target_kind_schemas();
        let mut t = target("Hello", "apple_library");
        // platform should be a string; pass a number.
        t.attrs.insert("platform".to_string(), json!(42));
        let diagnostics = validate_target(&t, &schemas);
        let mismatch = diagnostics
            .iter()
            .find(|d| d.code == "attr_type_mismatch" && d.attribute.as_deref() == Some("platform"))
            .expect("mismatch diagnostic");
        assert!(mismatch.message.contains("string"));
        assert!(mismatch.message.contains("number"));
        assert!(mismatch.repairs[0].contains("target.attrs.platform"));
        assert!(mismatch.repairs[0].contains("string"));
    }

    #[test]
    fn list_of_strings_validates_each_element() {
        let schemas = built_in_target_kind_schemas();
        let mut t = target("Hello", "apple_library");
        t.attrs.insert("platform".to_string(), json!("ios"));
        // sdk_frameworks is list<string>. Pass a list with one bad entry.
        t.attrs
            .insert("sdk_frameworks".to_string(), json!(["UIKit", 42]));
        let diagnostics = validate_target(&t, &schemas);
        let mismatch = diagnostics
            .iter()
            .find(|d| d.attribute.as_deref() == Some("sdk_frameworks"))
            .expect("mismatch diagnostic");
        assert!(mismatch.message.contains("element [1]"));
    }

    #[test]
    fn map_validates_value_type() {
        let schemas = built_in_target_kind_schemas();
        let mut t = target("Hello", "apple_application");
        t.attrs.insert("platform".to_string(), json!("ios"));
        t.attrs.insert("bundle_id".to_string(), json!("dev.once.X"));
        // info_plist_substitutions is map<string,string>. Pass a bool value.
        let mut subs = JsonMap::new();
        subs.insert("KEY".to_string(), json!(true));
        t.attrs.insert(
            "info_plist_substitutions".to_string(),
            JsonValue::Object(subs),
        );
        let diagnostics = validate_target(&t, &schemas);
        let mismatch = diagnostics
            .iter()
            .find(|d| d.attribute.as_deref() == Some("info_plist_substitutions"))
            .expect("mismatch diagnostic");
        assert!(mismatch.message.contains("entry `KEY`"));
    }

    #[test]
    fn configurable_select_validates_each_branch_value() {
        let schemas = built_in_target_kind_schemas();
        let mut t = target("Hello", "apple_library");
        t.attrs.insert("platform".to_string(), json!("ios"));
        t.attrs.insert(
            "sdk_frameworks".to_string(),
            json!({ "select": { "ios": ["UIKit"], "default": [] } }),
        );
        let diagnostics = validate_target(&t, &schemas);
        assert!(
            diagnostics.is_empty(),
            "expected select value to validate, got {diagnostics:?}"
        );
    }

    #[test]
    fn configurable_select_reports_branch_type_mismatch() {
        let schemas = built_in_target_kind_schemas();
        let mut t = target("Hello", "apple_library");
        t.attrs.insert("platform".to_string(), json!("ios"));
        t.attrs.insert(
            "sdk_frameworks".to_string(),
            json!({ "select": { "ios": "UIKit" } }),
        );
        let diagnostics = validate_target(&t, &schemas);
        let mismatch = diagnostics
            .iter()
            .find(|d| d.code == "attr_type_mismatch")
            .expect("branch mismatch diagnostic");
        assert!(mismatch.message.contains("select branch `ios`"));
    }

    #[test]
    fn select_on_non_configurable_attr_reports_specific_code() {
        let schemas = built_in_target_kind_schemas();
        let mut t = target("Hello", "apple_library");
        t.attrs.insert(
            "platform".to_string(),
            json!({ "select": { "default": "ios" } }),
        );
        let diagnostics = validate_target(&t, &schemas);
        let diagnostic = diagnostics
            .iter()
            .find(|d| d.code == "select_on_non_configurable_attr")
            .expect("non-configurable select diagnostic");
        assert_eq!(diagnostic.attribute.as_deref(), Some("platform"));
        assert!(!diagnostic.repairs.is_empty());
    }

    #[test]
    fn float_types_match_the_public_module_contract() {
        assert!(check_type(&json!(1.5), "float").is_ok());
        assert!(check_type(&json!([1.5, 2.5]), "list<float>").is_ok());
        assert!(check_type(&json!({ "ratio": 1.5 }), "map<string,float>").is_ok());
        assert!(check_type(&json!(1), "float").is_err());
    }

    #[test]
    fn valid_minimal_apple_library_returns_no_diagnostics() {
        let schemas = built_in_target_kind_schemas();
        let mut t = target("Hello", "apple_library");
        t.attrs.insert("platform".to_string(), json!("ios"));
        let diagnostics = validate_target(&t, &schemas);
        assert!(
            diagnostics.is_empty(),
            "expected no diagnostics, got: {diagnostics:?}",
        );
    }

    #[test]
    fn empty_name_diagnoses_with_attribute_scope_only() {
        let schemas = built_in_target_kind_schemas();
        let mut t = target("", "apple_library");
        t.attrs.insert("platform".to_string(), json!("ios"));
        let diagnostics = validate_target(&t, &schemas);
        let name_diag = diagnostics
            .iter()
            .find(|d| d.code == "target_name_required")
            .expect("name diagnostic");
        assert_eq!(name_diag.attribute.as_deref(), Some("name"));
        assert!(name_diag.target.is_none());
    }
}
