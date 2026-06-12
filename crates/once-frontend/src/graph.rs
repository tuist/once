//! Typed build graph model and built-in rule metadata.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};
use starlark::values::dict::DictRef;
use starlark::values::list::ListRef;
use starlark::values::Value;

use crate::analysis::select_branches;
use crate::error::{Error, Result};
use crate::target::{AttrValue, Target};
use crate::workspace::load_workspace;

/// Fully qualified graph target label.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TargetLabel {
    /// Package path that owns the target.
    pub package: String,
    /// Target name inside the package manifest.
    pub name: String,
    /// Canonical target id, formed from package and name.
    pub id: String,
}

/// Target record after manifest loading and rule metadata attachment.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GraphTarget {
    /// Canonical target label.
    pub label: TargetLabel,
    /// Rule kind declared by the target manifest. Matched against the
    /// prelude's `RULES` list to attach schema, capabilities, and
    /// providers.
    pub kind: String,
    /// Canonical dependency target ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    /// Source file patterns declared by the target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub srcs: Vec<String>,
    /// Typed target attributes parsed from the manifest.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attrs: BTreeMap<String, AttrValue>,
    /// Operations exposed by the target's rule schema.
    pub capabilities: Vec<Capability>,
    /// Providers emitted by this target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
    /// Non-fatal graph loading diagnostics for this target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

/// Operation exposed by a rule, such as build, run, or test.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Capability {
    /// Capability name.
    pub name: String,
    /// Output groups produced by this capability.
    pub output_groups: Vec<String>,
    /// Output groups that must already exist before this capability runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires_outputs: Vec<String>,
}

/// Diagnostic emitted while constructing the typed graph.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Diagnostic {
    /// Stable diagnostic code.
    pub code: String,
    /// Human-readable diagnostic message.
    pub message: String,
    /// Canonical target id this diagnostic is scoped to, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Attribute name this diagnostic is scoped to, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribute: Option<String>,
    /// Suggested repairs that an agent can apply or present.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repairs: Vec<String>,
}

/// Rule metadata used for schema queries and graph target enrichment.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RuleSchema {
    /// Rule kind matched by `target.kind`.
    pub kind: String,
    /// Human-readable rule description.
    pub docs: String,
    /// Attribute schema for this rule.
    pub attrs: Vec<AttrSchema>,
    /// Dependency expectations for this rule.
    pub deps: Vec<DepSchema>,
    /// Providers emitted by targets of this rule.
    pub providers: Vec<String>,
    /// Capabilities exposed by targets of this rule.
    pub capabilities: Vec<Capability>,
    /// Runnable starter workspaces. Each example bundles the files an
    /// agent or human needs to copy to get a working target of this
    /// rule kind, along with a one-line "use this when..." hint.
    pub examples: Vec<RuleExample>,
}

/// A runnable starter workspace for a rule. Resolved from the
/// `prelude/examples/<slug>/` directory bundled into the binary.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RuleExample {
    /// Human-readable example title.
    pub name: String,
    /// Stable identifier used to reference this example.
    pub slug: String,
    /// One-line "use this when..." hint that helps callers choose
    /// between examples for the same rule kind.
    pub use_when: String,
    /// Every file in the example bundle, sorted by path.
    pub files: Vec<RuleExampleFile>,
}

/// A single file inside a [`RuleExample`].
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RuleExampleFile {
    /// Path relative to the example workspace root.
    pub path: String,
    /// File contents as a UTF-8 string.
    pub contents: String,
}

/// Attribute metadata exposed by a rule schema.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AttrSchema {
    /// Attribute name under `[target.attrs]`.
    pub name: String,
    /// Human-readable type name.
    pub ty: String,
    /// Whether the attribute must be present.
    pub required: bool,
    /// Default value rendered as rule metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Human-readable attribute description.
    pub docs: String,
    /// Whether the value can vary by configuration.
    pub configurable: bool,
}

/// Dependency metadata exposed by a rule schema.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DepSchema {
    /// Dependency attribute name.
    pub name: String,
    /// Providers accepted by this dependency edge.
    pub expected_providers: Vec<String>,
    /// Human-readable dependency description.
    pub docs: String,
}

pub fn load_graph_workspace(root: &Path) -> Result<Vec<GraphTarget>> {
    let targets = load_workspace(root)?;
    graph_from_targets_result(&targets)
}

#[must_use]
pub fn graph_from_targets(targets: &[Target]) -> Vec<GraphTarget> {
    targets.iter().map(GraphTarget::from).collect()
}

pub fn graph_from_targets_result(targets: &[Target]) -> Result<Vec<GraphTarget>> {
    let schemas = built_in_rule_schemas_result()?;
    Ok(targets
        .iter()
        .map(|target| graph_target_from_schema(target, &schemas))
        .collect())
}

#[must_use]
pub fn built_in_rule_schemas() -> Vec<RuleSchema> {
    let Ok(schemas) = built_in_rule_schemas_result() else {
        return vec![script_schema()];
    };
    schemas
}

pub fn built_in_rule_schemas_result() -> Result<Vec<RuleSchema>> {
    let mut schemas = starlark_prelude_rule_schemas()?;
    schemas.push(script_schema());
    Ok(schemas)
}

#[must_use]
pub fn built_in_rule_schema(kind: &str) -> Option<RuleSchema> {
    built_in_rule_schemas()
        .into_iter()
        .find(|schema| schema.kind == kind)
}

impl From<&Target> for GraphTarget {
    fn from(target: &Target) -> Self {
        graph_target_from_schema(target, &built_in_rule_schemas())
    }
}

fn graph_target_from_schema(target: &Target, schemas: &[RuleSchema]) -> GraphTarget {
    let schema = schemas.iter().find(|schema| schema.kind == target.kind);
    let mut diagnostics = if schema.is_some() {
        Vec::new()
    } else {
        vec![Diagnostic {
            code: "unknown_rule_kind".to_string(),
            message: format!("target kind `{}` has no built-in schema", target.kind),
            target: Some(target.id()),
            attribute: None,
            repairs: Vec::new(),
        }]
    };
    let attrs = graph_attrs(target);
    if let Some(schema) = schema {
        for attr_schema in &schema.attrs {
            if attr_schema.configurable {
                continue;
            }
            if let Some(value) = attrs.get(&attr_schema.name) {
                if select_branches(value).is_some() {
                    diagnostics.push(Diagnostic {
                        code: "select_on_non_configurable_attr".to_string(),
                        message: format!(
                            "attribute `{}` is not configurable but uses `select()`",
                            attr_schema.name
                        ),
                        repairs: Vec::new(),
                    });
                }
            }
        }
    }
    GraphTarget {
        label: TargetLabel {
            package: target.package.clone(),
            name: target.name.clone(),
            id: target.id(),
        },
        kind: target.kind.clone(),
        deps: target.deps.clone(),
        srcs: target.srcs.clone(),
        attrs,
        capabilities: schema
            .as_ref()
            .map_or_else(Vec::new, |schema| schema.capabilities.clone()),
        providers: schema.map_or_else(Vec::new, |schema| schema.providers.clone()),
        diagnostics,
    }
}

fn graph_attrs(target: &Target) -> BTreeMap<String, AttrValue> {
    if !target.typed_attrs.is_empty() {
        return target.typed_attrs.clone();
    }
    target
        .attrs
        .iter()
        .map(|(key, value)| (key.clone(), AttrValue::String(value.clone())))
        .collect()
}

fn starlark_prelude_rule_schemas() -> Result<Vec<RuleSchema>> {
    const PRELUDE_PATH: &str = "once//prelude/apple.star";
    let source = include_str!("../prelude/apple.star");
    parse_rule_schemas(PRELUDE_PATH, source)
}

/// Evaluate a Starlark prelude source and read its `RULES` export.
///
/// Split out from [`starlark_prelude_rule_schemas`] so the error paths
/// (parse failure, missing export, wrong types) are reachable from tests
/// without depending on the compiled-in prelude staying valid, and so they
/// keep working if the prelude ever becomes user-configurable.
fn parse_rule_schemas(path: &str, source: &str) -> Result<Vec<RuleSchema>> {
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse(path, source.to_string(), &Dialect::Standard)
            .map_err(|error| prelude_error(path, error))?;
        let globals = crate::analysis::globals_for_prelude();
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)
            .map_err(|error| prelude_error(path, error))?;
        let rules = module
            .get("RULES")
            .ok_or_else(|| prelude_message(path, "missing RULES export"))?;
        rule_schemas_from_value(rules).map_err(|message| prelude_message(path, &message))
    })
}

fn prelude_error(path: &str, error: impl std::fmt::Debug) -> Error {
    // The starlark crate's errors carry their detail in Debug, not Display,
    // so reach through Debug to surface variable-not-found / type-mismatch
    // diagnostics instead of an empty `Eval` body.
    prelude_message(path, &format!("{error:?}"))
}

fn prelude_message(path: &str, message: &str) -> Error {
    Error::Eval {
        path: path.to_string(),
        message: message.to_string(),
    }
}

fn rule_schemas_from_value(value: Value<'_>) -> std::result::Result<Vec<RuleSchema>, String> {
    list(value, "RULES")?
        .iter()
        .enumerate()
        .map(|(index, rule)| rule_schema_from_value(rule, &format!("RULES[{index}]")))
        .collect()
}

fn rule_schema_from_value(value: Value<'_>, path: &str) -> std::result::Result<RuleSchema, String> {
    let example_slugs = field_string_list(value, path, "examples")?;
    let examples = example_slugs
        .into_iter()
        .enumerate()
        .map(|(index, slug)| {
            crate::examples::load_example(&slug).ok_or_else(|| {
                format!("{path}.examples[{index}] references unknown example slug `{slug}`")
            })
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(RuleSchema {
        kind: field_string(value, path, "kind")?,
        docs: field_string(value, path, "docs")?,
        attrs: field_list(value, path, "attrs")?
            .iter()
            .enumerate()
            .map(|(index, attr)| attr_schema_from_value(attr, &format!("{path}.attrs[{index}]")))
            .collect::<std::result::Result<Vec<_>, _>>()?,
        deps: field_list(value, path, "deps")?
            .iter()
            .enumerate()
            .map(|(index, dep)| dep_schema_from_value(dep, &format!("{path}.deps[{index}]")))
            .collect::<std::result::Result<Vec<_>, _>>()?,
        providers: field_string_list(value, path, "providers")?,
        capabilities: field_list(value, path, "capabilities")?
            .iter()
            .enumerate()
            .map(|(index, capability)| {
                capability_from_value(capability, &format!("{path}.capabilities[{index}]"))
            })
            .collect::<std::result::Result<Vec<_>, _>>()?,
        examples,
    })
}

fn attr_schema_from_value(value: Value<'_>, path: &str) -> std::result::Result<AttrSchema, String> {
    Ok(AttrSchema {
        name: field_string(value, path, "name")?,
        ty: field_string(value, path, "ty")?,
        required: field_bool(value, path, "required")?,
        default: optional_field_string(value, path, "default")?,
        docs: field_string(value, path, "docs")?,
        configurable: field_bool(value, path, "configurable")?,
    })
}

fn dep_schema_from_value(value: Value<'_>, path: &str) -> std::result::Result<DepSchema, String> {
    Ok(DepSchema {
        name: field_string(value, path, "name")?,
        expected_providers: field_string_list(value, path, "expected_providers")?,
        docs: field_string(value, path, "docs")?,
    })
}

fn capability_from_value(value: Value<'_>, path: &str) -> std::result::Result<Capability, String> {
    Ok(Capability {
        name: field_string(value, path, "name")?,
        output_groups: field_string_list(value, path, "output_groups")?,
        requires_outputs: field_string_list(value, path, "requires_outputs")?,
    })
}

fn field_value<'v>(
    value: Value<'v>,
    path: &str,
    field: &str,
) -> std::result::Result<Value<'v>, String> {
    let dict = DictRef::from_value(value)
        .ok_or_else(|| format!("{path} should be a dict, got `{}`", value.get_type()))?;
    dict.get_str(field)
        .ok_or_else(|| format!("{path} is missing `{field}`"))
}

fn field_string(value: Value<'_>, path: &str, field: &str) -> std::result::Result<String, String> {
    let value = field_value(value, path, field)?;
    value.unpack_str().map(ToOwned::to_owned).ok_or_else(|| {
        format!(
            "{path}.{field} should be a string, got `{}`",
            value.get_type()
        )
    })
}

fn optional_field_string(
    value: Value<'_>,
    path: &str,
    field: &str,
) -> std::result::Result<Option<String>, String> {
    let value = field_value(value, path, field)?;
    if value.is_none() {
        return Ok(None);
    }
    value
        .unpack_str()
        .map(|value| Some(value.to_owned()))
        .ok_or_else(|| {
            format!(
                "{path}.{field} should be a string or None, got `{}`",
                value.get_type()
            )
        })
}

fn field_bool(value: Value<'_>, path: &str, field: &str) -> std::result::Result<bool, String> {
    let value = field_value(value, path, field)?;
    value.unpack_bool().ok_or_else(|| {
        format!(
            "{path}.{field} should be a bool, got `{}`",
            value.get_type()
        )
    })
}

fn field_list<'v>(
    value: Value<'v>,
    path: &str,
    field: &str,
) -> std::result::Result<&'v ListRef<'v>, String> {
    let value = field_value(value, path, field)?;
    list(value, &format!("{path}.{field}"))
}

fn field_string_list(
    value: Value<'_>,
    path: &str,
    field: &str,
) -> std::result::Result<Vec<String>, String> {
    let field_path = format!("{path}.{field}");
    list(field_value(value, path, field)?, &field_path)?
        .iter()
        .enumerate()
        .map(|(index, item)| {
            item.unpack_str().map(ToOwned::to_owned).ok_or_else(|| {
                format!(
                    "{field_path}[{index}] should be a string, got `{}`",
                    item.get_type()
                )
            })
        })
        .collect()
}

fn list<'v>(value: Value<'v>, path: &str) -> std::result::Result<&'v ListRef<'v>, String> {
    ListRef::from_value(value)
        .ok_or_else(|| format!("{path} should be a list, got `{}`", value.get_type()))
}

fn script_schema() -> RuleSchema {
    RuleSchema {
        kind: "script".to_string(),
        docs: "Migration target that wraps an annotated script action.".to_string(),
        attrs: vec![
            attr(
                "script_path",
                "string",
                true,
                None,
                "Workspace-relative script path",
                false,
            ),
            attr(
                "script_runtime",
                "string",
                true,
                None,
                "Runtime parsed from the script shebang",
                false,
            ),
        ],
        deps: Vec::new(),
        providers: vec!["script_action".to_string()],
        capabilities: vec![capability("run", &["default"], &[])],
        examples: Vec::new(),
    }
}

fn attr(
    name: &str,
    ty: &str,
    required: bool,
    default: Option<&str>,
    docs: &str,
    configurable: bool,
) -> AttrSchema {
    AttrSchema {
        name: name.to_string(),
        ty: ty.to_string(),
        required,
        default: default.map(str::to_string),
        docs: docs.to_string(),
        configurable,
    }
}

fn capability(name: &str, output_groups: &[&str], requires_outputs: &[&str]) -> Capability {
    Capability {
        name: name.to_string(),
        output_groups: output_groups
            .iter()
            .map(|group| (*group).to_string())
            .collect(),
        requires_outputs: requires_outputs
            .iter()
            .map(|group| (*group).to_string())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::Target;

    #[test]
    fn apple_application_exposes_build_and_run() {
        let target = Target {
            package: "apps/ios".to_string(),
            kind: "apple_application".to_string(),
            name: "App".to_string(),
            deps: vec!["apps/ios/AppKit".to_string()],
            srcs: Vec::new(),
            attrs: BTreeMap::new(),
            typed_attrs: BTreeMap::new(),
        };

        let graph = graph_from_targets(&[target]);
        let app = &graph[0];
        assert_eq!(app.label.id, "apps/ios/App");
        // Assert membership rather than order: capability order is defined by
        // the prelude and reordering it there should not break this test.
        let mut names = app
            .capabilities
            .iter()
            .map(|capability| capability.name.as_str())
            .collect::<Vec<_>>();
        names.sort_unstable();
        assert_eq!(names, vec!["build", "run"]);
        assert!(app
            .capabilities
            .iter()
            .any(|capability| capability.name == "run"
                && capability.requires_outputs == vec!["bundle"]));
    }

    #[test]
    fn parse_rule_schemas_rejects_invalid_syntax() {
        let err = parse_rule_schemas("test.star", "RULES = [").unwrap_err();
        assert!(matches!(err, Error::Eval { .. }));
    }

    #[test]
    fn parse_rule_schemas_requires_rules_export() {
        let err = parse_rule_schemas("test.star", "OTHER = []").unwrap_err();
        match err {
            Error::Eval { message, .. } => assert!(message.contains("missing RULES export")),
            other => panic!("expected Error::Eval, got {other:?}"),
        }
    }

    #[test]
    fn parse_rule_schemas_requires_list_export() {
        let err = parse_rule_schemas("test.star", "RULES = 7").unwrap_err();
        match err {
            Error::Eval { message, .. } => assert!(message.contains("should be a list")),
            other => panic!("expected Error::Eval, got {other:?}"),
        }
    }

    #[test]
    fn parse_rule_schemas_reports_missing_rule_field() {
        let err = parse_rule_schemas("test.star", r#"RULES = [{"kind": "x"}]"#).unwrap_err();
        match err {
            Error::Eval { message, .. } => assert!(message.contains("is missing")),
            other => panic!("expected Error::Eval, got {other:?}"),
        }
    }

    #[test]
    fn parse_rule_schemas_accepts_minimal_valid_rule() {
        let source = r#"
RULES = [
    {
        "kind": "demo",
        "docs": "Demo rule",
        "attrs": [],
        "deps": [],
        "providers": [],
        "capabilities": [],
        "examples": [],
    },
]
"#;
        let schemas = parse_rule_schemas("test.star", source).unwrap();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].kind, "demo");
    }

    #[test]
    fn unknown_kind_gets_diagnostic_and_no_capabilities() {
        let target = Target {
            package: "apps/ios".to_string(),
            kind: "mystery_rule".to_string(),
            name: "Thing".to_string(),
            deps: Vec::new(),
            srcs: Vec::new(),
            attrs: BTreeMap::new(),
            typed_attrs: BTreeMap::new(),
        };

        let graph = graph_from_targets(&[target]);
        let thing = &graph[0];
        assert!(thing.capabilities.is_empty());
        assert!(thing.providers.is_empty());
        assert_eq!(thing.diagnostics.len(), 1);
        assert_eq!(thing.diagnostics[0].code, "unknown_rule_kind");
        assert!(thing.diagnostics[0]
            .message
            .contains("`mystery_rule` has no built-in schema"));
    }

    #[test]
    fn graph_attrs_fall_back_to_string_attrs_when_untyped() {
        let mut attrs = BTreeMap::new();
        attrs.insert("platform".to_string(), "ios".to_string());
        let target = Target {
            package: "apps/ios".to_string(),
            kind: "apple_library".to_string(),
            name: "AppCore".to_string(),
            deps: Vec::new(),
            srcs: Vec::new(),
            attrs,
            typed_attrs: BTreeMap::new(),
        };

        let graph = graph_from_targets(&[target]);
        assert_eq!(
            graph[0].attrs.get("platform"),
            Some(&AttrValue::String("ios".to_string()))
        );
    }

    #[test]
    fn typed_attrs_take_precedence_over_string_attrs() {
        let mut attrs = BTreeMap::new();
        attrs.insert("enable_testing".to_string(), "true".to_string());
        let mut typed_attrs = BTreeMap::new();
        typed_attrs.insert("enable_testing".to_string(), AttrValue::Bool(true));
        let target = Target {
            package: "apps/ios".to_string(),
            kind: "apple_library".to_string(),
            name: "AppCore".to_string(),
            deps: Vec::new(),
            srcs: Vec::new(),
            attrs,
            typed_attrs,
        };

        let graph = graph_from_targets(&[target]);
        assert_eq!(
            graph[0].attrs.get("enable_testing"),
            Some(&AttrValue::Bool(true))
        );
    }

    #[test]
    fn select_on_non_configurable_attribute_emits_a_diagnostic() {
        // module_name is declared `configurable = False` in the
        // prelude. A select() on it should surface a diagnostic, not
        // be silently accepted.
        let mut typed_attrs = BTreeMap::new();
        let mut select_outer = BTreeMap::new();
        let mut branches = BTreeMap::new();
        branches.insert(
            "default".to_string(),
            AttrValue::String("Default".to_string()),
        );
        select_outer.insert("select".to_string(), AttrValue::Map(branches));
        typed_attrs.insert("module_name".to_string(), AttrValue::Map(select_outer));
        typed_attrs.insert("platform".to_string(), AttrValue::String("ios".to_string()));
        let target = Target {
            package: "apps/ios".to_string(),
            kind: "apple_library".to_string(),
            name: "AppCore".to_string(),
            deps: Vec::new(),
            srcs: Vec::new(),
            attrs: BTreeMap::new(),
            typed_attrs,
        };
        let graph = graph_from_targets(&[target]);
        let diag = graph[0]
            .diagnostics
            .iter()
            .find(|d| d.code == "select_on_non_configurable_attr")
            .expect("expected select_on_non_configurable_attr diagnostic");
        assert!(diag.message.contains("module_name"), "{}", diag.message);
    }

    #[test]
    fn apple_library_schema_exposes_multi_arch_attributes() {
        let schema = built_in_rule_schema("apple_library").expect("apple_library schema");
        let attr_names: Vec<&str> = schema.attrs.iter().map(|attr| attr.name.as_str()).collect();
        assert!(
            attr_names.contains(&"archs"),
            "apple_library should expose an archs attribute, got {attr_names:?}"
        );
        assert!(
            attr_names.contains(&"mac_catalyst"),
            "apple_library should expose a mac_catalyst attribute, got {attr_names:?}"
        );
    }

    #[test]
    fn built_in_schema_contains_apple_rule_set() {
        let kinds = built_in_rule_schemas()
            .into_iter()
            .map(|schema| schema.kind)
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                "swift_macro",
                "apple_library",
                "apple_framework",
                "apple_application",
                "apple_test_bundle",
                "script"
            ]
        );
    }
}
