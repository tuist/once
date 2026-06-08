//! Typed build graph model and built-in rule metadata.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use starlark::environment::{GlobalsBuilder, Module};
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};
use starlark::values::dict::DictRef;
use starlark::values::list::ListRef;
use starlark::values::Value;

use crate::error::Result;
use crate::target::{AttrValue, Target};
use crate::workspace::load_workspace;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TargetLabel {
    pub package: String,
    pub name: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GraphTarget {
    pub label: TargetLabel,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub srcs: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attrs: BTreeMap<String, AttrValue>,
    pub capabilities: Vec<Capability>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Capability {
    pub name: String,
    pub output_groups: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires_outputs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repairs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RuleSchema {
    pub kind: String,
    pub docs: String,
    pub attrs: Vec<AttrSchema>,
    pub deps: Vec<DepSchema>,
    pub providers: Vec<String>,
    pub capabilities: Vec<Capability>,
    pub examples: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AttrSchema {
    pub name: String,
    pub ty: String,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    pub docs: String,
    pub configurable: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DepSchema {
    pub name: String,
    pub expected_providers: Vec<String>,
    pub docs: String,
}

pub fn load_graph_workspace(root: &Path) -> Result<Vec<GraphTarget>> {
    let targets = load_workspace(root)?;
    Ok(graph_from_targets(&targets))
}

#[must_use]
pub fn graph_from_targets(targets: &[Target]) -> Vec<GraphTarget> {
    targets.iter().map(GraphTarget::from).collect()
}

#[must_use]
pub fn built_in_rule_schemas() -> Vec<RuleSchema> {
    let mut schemas = starlark_prelude_rule_schemas();
    schemas.push(script_schema());
    schemas
}

#[must_use]
pub fn built_in_rule_schema(kind: &str) -> Option<RuleSchema> {
    built_in_rule_schemas()
        .into_iter()
        .find(|schema| schema.kind == kind)
}

impl From<&Target> for GraphTarget {
    fn from(target: &Target) -> Self {
        let schema = built_in_rule_schema(&target.kind);
        let diagnostics = if schema.is_some() {
            Vec::new()
        } else {
            vec![Diagnostic {
                code: "unknown_rule_kind".to_string(),
                message: format!("target kind `{}` has no built-in schema", target.kind),
                repairs: Vec::new(),
            }]
        };
        GraphTarget {
            label: TargetLabel {
                package: target.package.clone(),
                name: target.name.clone(),
                id: target.id(),
            },
            kind: target.kind.clone(),
            deps: target.deps.clone(),
            srcs: target.srcs.clone(),
            attrs: graph_attrs(target),
            capabilities: schema
                .as_ref()
                .map_or_else(Vec::new, |schema| schema.capabilities.clone()),
            providers: schema.map_or_else(Vec::new, |schema| schema.providers),
            diagnostics,
        }
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

fn starlark_prelude_rule_schemas() -> Vec<RuleSchema> {
    let source = include_str!("../prelude/apple.star");
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse(
            "once//prelude/apple.star",
            source.to_string(),
            &Dialect::Standard,
        )
        .expect("built-in Apple Starlark prelude should parse");
        let globals = GlobalsBuilder::standard().build();
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)
            .expect("built-in Apple Starlark prelude should evaluate");
        let rules = module
            .get("APPLE_RULES")
            .expect("built-in Apple Starlark prelude should export APPLE_RULES");
        rule_schemas_from_value(rules)
            .expect("built-in Apple Starlark prelude should produce valid rule schemas")
    })
}

fn rule_schemas_from_value(value: Value<'_>) -> std::result::Result<Vec<RuleSchema>, String> {
    list(value, "APPLE_RULES")?
        .iter()
        .enumerate()
        .map(|(index, rule)| rule_schema_from_value(rule, &format!("APPLE_RULES[{index}]")))
        .collect()
}

fn rule_schema_from_value(value: Value<'_>, path: &str) -> std::result::Result<RuleSchema, String> {
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
        examples: field_string_list(value, path, "examples")?,
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
        assert_eq!(
            app.capabilities
                .iter()
                .map(|capability| capability.name.as_str())
                .collect::<Vec<_>>(),
            vec!["build", "run"]
        );
        assert!(app
            .capabilities
            .iter()
            .any(|capability| capability.name == "run"
                && capability.requires_outputs == vec!["bundle"]));
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
                "apple_library",
                "apple_framework",
                "apple_application",
                "apple_test_bundle",
                "script"
            ]
        );
    }
}
