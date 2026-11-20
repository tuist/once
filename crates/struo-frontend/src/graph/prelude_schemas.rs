//! Evaluating the Starlark prelude and module files, and reading the target
//! kind schemas they export.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};
use starlark::values::dict::DictRef;
use starlark::values::list::ListRef;
use starlark::values::Value;

use crate::error::{Error, Result};

use super::{
    AttrSchema, Capability, DepSchema, SourceReference, TargetKindExample, TargetKindExampleRoot,
    TargetKindExampleSource, TargetKindSchema, ToolRequirement,
};

#[derive(Debug, Clone)]
pub(super) struct TargetKindSchemaSource {
    example_root: TargetKindExampleRoot,
    example_base: String,
}

impl TargetKindSchemaSource {
    pub(super) fn built_in_prelude() -> Self {
        Self {
            example_root: TargetKindExampleRoot::BuiltInPrelude,
            example_base: String::new(),
        }
    }

    pub(super) fn workspace(root: &Path, module_file: &str) -> Self {
        Self {
            example_root: TargetKindExampleRoot::Workspace {
                root: root.to_path_buf(),
            },
            example_base: parent_dir(module_file),
        }
    }

    pub(super) fn example_source(
        &self,
        path: &str,
    ) -> std::result::Result<TargetKindExampleSource, String> {
        crate::examples::example_source(self.example_root.clone(), &self.example_base, path)
    }
}

pub(super) fn parent_dir(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_default()
}

pub(super) fn starlark_prelude_target_kind_schemas() -> Result<Vec<TargetKindSchema>> {
    static BUILT_IN_TARGET_KIND_SCHEMAS: OnceLock<Vec<TargetKindSchema>> = OnceLock::new();

    if let Some(schemas) = BUILT_IN_TARGET_KIND_SCHEMAS.get() {
        return Ok(schemas.clone());
    }

    let schemas = parse_target_kind_schemas(
        crate::modules::BUILT_IN_MODULE_PATH,
        crate::modules::built_in_module_source(),
    )?;
    let _ = BUILT_IN_TARGET_KIND_SCHEMAS.set(schemas.clone());
    Ok(schemas)
}

/// Evaluate a Starlark module source and read its exported target kind symbols.
///
/// Split out from [`starlark_prelude_target_kind_schemas`] so the error paths
/// (parse failure, missing exports, wrong types) are reachable from tests
/// without depending on the compiled-in prelude staying valid, and so they
/// keep working if the prelude ever becomes user-configurable.
pub(super) fn parse_target_kind_schemas(path: &str, source: &str) -> Result<Vec<TargetKindSchema>> {
    prelude_exports(source)
        .target_kinds
        .clone()
        .map_err(|message| prelude_message(path, &message))
}

/// What one evaluation of a prelude source declares.
///
/// Both halves are kept as results rather than being combined, so a workspace
/// whose native project declarations are malformed still answers questions
/// about target kinds, exactly as it did when the two were read separately.
pub(crate) struct PreludeExports {
    pub(crate) target_kinds: std::result::Result<Vec<TargetKindSchema>, String>,
    pub(crate) native_projects:
        std::result::Result<Vec<crate::native_project::NativeProjectSchema>, String>,
}

/// Everything a prelude source declares, evaluated at most once per process.
///
/// Target kinds and native projects are two questions about the same megabyte
/// of Starlark, and an invocation asks both. Evaluating it twice is the largest
/// fixed cost of loading a workspace, so the source is evaluated once and both
/// answers are read out of the same module.
///
/// Keyed by the source rather than by the path it was given, because the same
/// text reaches this under two names. A failure is not remembered, so the
/// caller that hit it reports it under its own path.
pub(crate) fn prelude_exports(source: &str) -> std::sync::Arc<PreludeExports> {
    use std::sync::{Arc, Mutex, OnceLock};

    static EVALUATED: OnceLock<Mutex<BTreeMap<String, Arc<PreludeExports>>>> = OnceLock::new();

    let evaluated = EVALUATED.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Some(exports) = evaluated
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(source)
    {
        return Arc::clone(exports);
    }
    let exports = Arc::new(evaluate_prelude_exports(source));
    evaluated
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(source.to_string(), Arc::clone(&exports));
    exports
}

pub(super) fn evaluate_prelude_exports(source: &str) -> PreludeExports {
    let path = crate::modules::BUILT_IN_MODULE_PATH;
    Module::with_temp_heap(|module| {
        let ast = match AstModule::parse(path, source.to_string(), &Dialect::Standard) {
            Ok(ast) => ast,
            Err(error) => {
                let message = format!("{error:?}");
                return PreludeExports {
                    target_kinds: Err(message.clone()),
                    native_projects: Err(message),
                };
            }
        };
        let globals = crate::analysis::globals_for_prelude();
        let mut eval = Evaluator::new(&module);
        if let Err(error) = eval.eval_module(ast, &globals) {
            let message = format!("{error:?}");
            return PreludeExports {
                target_kinds: Err(message.clone()),
                native_projects: Err(message),
            };
        }
        let exported = crate::modules::exported_target_kind_values(&module);
        let target_kinds = if exported.is_empty() {
            Err("no target kind symbols exported".to_string())
        } else {
            target_kind_schemas_from_exports(&exported, &TargetKindSchemaSource::built_in_prelude())
        };
        let native_projects = crate::native_project::native_project_schemas_from_module(&module);
        PreludeExports {
            target_kinds,
            native_projects,
        }
    })
}

pub(super) fn parse_target_kind_schemas_with_context(
    path: &str,
    source: &str,
    source_context: &TargetKindSchemaSource,
) -> Result<Vec<TargetKindSchema>> {
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse(path, source.to_string(), &Dialect::Standard)
            .map_err(|error| prelude_error(path, error))?;
        let globals = crate::analysis::globals_for_prelude();
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)
            .map_err(|error| prelude_error(path, error))?;
        let target_kinds = crate::modules::exported_target_kind_values(&module);
        if target_kinds.is_empty() {
            return Err(prelude_message(path, "no target kind symbols exported"));
        }
        target_kind_schemas_from_exports(&target_kinds, source_context)
            .map_err(|message| prelude_message(path, &message))
    })
}

pub(super) fn prelude_error(path: &str, error: impl std::fmt::Debug) -> Error {
    // The starlark crate's errors carry their detail in Debug, not Display,
    // so reach through Debug to surface variable-not-found / type-mismatch
    // diagnostics instead of an empty `Eval` body.
    prelude_message(path, &format!("{error:?}"))
}

pub(super) fn prelude_message(path: &str, message: &str) -> Error {
    Error::Eval {
        path: path.to_string(),
        message: message.to_string(),
    }
}

pub(super) fn target_kind_schemas_from_exports(
    target_kinds: &[crate::modules::TargetKindExport<'_>],
    source_context: &TargetKindSchemaSource,
) -> std::result::Result<Vec<TargetKindSchema>, String> {
    let schemas = target_kinds
        .iter()
        .map(|kind| target_kind_schema_from_value(kind.value, kind.name, source_context))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    validate_unique_target_kinds(&schemas)?;
    Ok(schemas)
}

pub(crate) fn target_kind_schemas_from_compiled_exports(
    target_kinds: &[crate::modules::TargetKindExport<'_>],
) -> std::result::Result<Vec<TargetKindSchema>, String> {
    target_kind_schemas_from_exports(target_kinds, &TargetKindSchemaSource::built_in_prelude())
}

pub(super) fn validate_unique_target_kinds(
    schemas: &[TargetKindSchema],
) -> std::result::Result<(), String> {
    let mut seen = BTreeMap::new();
    for (index, schema) in schemas.iter().enumerate() {
        if let Some(first_index) = seen.insert(schema.kind.as_str(), index) {
            return Err(format!(
                "target kind `{}` is declared more than once (target kind export {first_index} and {index})",
                schema.kind
            ));
        }
    }
    Ok(())
}

pub(super) fn append_script_schema(schemas: &mut Vec<TargetKindSchema>) -> Result<()> {
    if schemas.iter().any(|schema| schema.kind == "script") {
        return Err(prelude_message(
            crate::modules::COMBINED_MODULE_PATH,
            "target kind `script` is reserved for Struo script targets",
        ));
    }
    schemas.push(script_schema());
    Ok(())
}

pub(super) fn target_kind_schema_from_value(
    value: Value<'_>,
    path: &str,
    source_context: &TargetKindSchemaSource,
) -> std::result::Result<TargetKindSchema, String> {
    let kind = crate::modules::target_kind(value, path)?;
    let has_resolver = DictRef::from_value(value)
        .and_then(|target_kind| target_kind.get_str("resolver"))
        .is_some_and(|resolver| !resolver.is_none());
    let examples = field_list(value, path, "examples")?
        .iter()
        .enumerate()
        .map(|(index, example)| {
            example_from_value(
                example,
                &format!("{path}.examples[{index}]"),
                source_context,
            )
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(TargetKindSchema {
        kind,
        docs: field_string(value, path, "docs")?,
        has_resolver,
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
        tools: field_list(value, path, "tools")?
            .iter()
            .enumerate()
            .map(|(index, tool)| {
                tool_requirement_from_value(tool, &format!("{path}.tools[{index}]"))
            })
            .collect::<std::result::Result<Vec<_>, _>>()?,
        source_references: field_list(value, path, "source_references")?
            .iter()
            .enumerate()
            .map(|(index, reference)| {
                source_reference_from_value(
                    reference,
                    &format!("{path}.source_references[{index}]"),
                )
            })
            .collect::<std::result::Result<Vec<_>, _>>()?,
        examples,
    })
}

pub(super) fn source_reference_from_value(
    value: Value<'_>,
    path: &str,
) -> std::result::Result<SourceReference, String> {
    let dict = DictRef::from_value(value).ok_or_else(|| {
        format!(
            "{path} should be a source_reference(...) value, got `{}`",
            value.get_type()
        )
    })?;
    let marker = dict
        .get_str("_struo_source_reference")
        .and_then(Value::unpack_bool)
        .unwrap_or(false);
    if !marker {
        return Err(format!("{path} should be a source_reference(...) value"));
    }
    Ok(SourceReference {
        system: non_empty_field_string(value, path, "system")?,
        symbol: non_empty_field_string(value, path, "symbol")?,
        url: non_empty_field_string(value, path, "url")?,
        use_when: non_empty_field_string(value, path, "use_when")?,
        content_digest: optional_non_empty_field_string(value, path, "content_digest")?,
    })
}

pub(super) fn attr_schema_from_value(
    value: Value<'_>,
    path: &str,
) -> std::result::Result<AttrSchema, String> {
    let schema = AttrSchema {
        name: field_string(value, path, "name")?,
        ty: field_string(value, path, "ty")?,
        required: field_bool(value, path, "required")?,
        default: optional_field_string(value, path, "default")?,
        docs: field_string(value, path, "docs")?,
        configurable: field_bool(value, path, "configurable")?,
        implemented: field_bool(value, path, "implemented")?,
        allowed_values: field_string_list(value, path, "allowed_values")?,
        disallowed_values: field_string_list(value, path, "disallowed_values")?,
    };
    if !schema.allowed_values.is_empty() && schema.ty != "string" && schema.ty != "target" {
        return Err(format!(
            "{path}.allowed_values is supported only for string and target attributes"
        ));
    }
    if !schema.disallowed_values.is_empty() && schema.ty != "string" && schema.ty != "target" {
        return Err(format!(
            "{path}.disallowed_values is supported only for string and target attributes"
        ));
    }
    Ok(schema)
}

pub(super) fn dep_schema_from_value(
    value: Value<'_>,
    path: &str,
) -> std::result::Result<DepSchema, String> {
    let schema = DepSchema {
        name: field_string(value, path, "name")?,
        expected_providers: field_string_list(value, path, "expected_providers")?,
        docs: field_string(value, path, "docs")?,
        min_count: field_non_negative_usize(value, path, "min_count")?,
        max_count: optional_field_non_negative_usize(value, path, "max_count")?,
    };
    if schema
        .max_count
        .is_some_and(|max_count| schema.min_count > max_count)
    {
        return Err(format!("{path}.min_count must not exceed {path}.max_count"));
    }
    Ok(schema)
}

pub(super) fn capability_from_value(
    value: Value<'_>,
    path: &str,
) -> std::result::Result<Capability, String> {
    Ok(Capability {
        name: field_string(value, path, "name")?,
        output_groups: field_string_list(value, path, "output_groups")?,
        requires_outputs: field_string_list(value, path, "requires_outputs")?,
    })
}

pub(super) fn tool_requirement_from_value(
    value: Value<'_>,
    path: &str,
) -> std::result::Result<ToolRequirement, String> {
    Ok(ToolRequirement {
        name: non_empty_field_string(value, path, "name")?,
        executables: field_string_list(value, path, "executables")?,
    })
}

pub(super) fn example_from_value(
    value: Value<'_>,
    path: &str,
    source_context: &TargetKindSchemaSource,
) -> std::result::Result<TargetKindExample, String> {
    let dict = DictRef::from_value(value).ok_or_else(|| {
        format!(
            "{path} should be an example(...) value, got `{}`",
            value.get_type()
        )
    })?;
    let marker = dict
        .get_str("_struo_example")
        .and_then(Value::unpack_bool)
        .unwrap_or(false);
    if !marker {
        return Err(format!("{path} should be an example(...) value"));
    }
    let slug = non_empty_field_string(value, path, "slug")?;
    let name = non_empty_field_string(value, path, "name")?;
    let use_when = non_empty_field_string(value, path, "use_when")?;
    let example_path = non_empty_field_string(value, path, "path")?;
    let platforms = field_string_list(value, path, "platforms").unwrap_or_default();
    let source = source_context
        .example_source(&example_path)
        .map_err(|message| format!("{path}.path {message}"))?;
    Ok(TargetKindExample {
        name,
        slug,
        use_when,
        platforms,
        source,
    })
}

pub(super) fn field_value<'v>(
    value: Value<'v>,
    path: &str,
    field: &str,
) -> std::result::Result<Value<'v>, String> {
    let dict = DictRef::from_value(value)
        .ok_or_else(|| format!("{path} should be a dict, got `{}`", value.get_type()))?;
    dict.get_str(field)
        .ok_or_else(|| format!("{path} is missing `{field}`"))
}

pub(super) fn field_string(
    value: Value<'_>,
    path: &str,
    field: &str,
) -> std::result::Result<String, String> {
    let value = field_value(value, path, field)?;
    value.unpack_str().map(ToOwned::to_owned).ok_or_else(|| {
        format!(
            "{path}.{field} should be a string, got `{}`",
            value.get_type()
        )
    })
}

pub(super) fn non_empty_field_string(
    value: Value<'_>,
    path: &str,
    field: &str,
) -> std::result::Result<String, String> {
    let value = field_string(value, path, field)?;
    if value.trim().is_empty() {
        return Err(format!("{path}.{field} should be non-empty"));
    }
    Ok(value)
}

pub(super) fn optional_field_string(
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

pub(super) fn optional_non_empty_field_string(
    value: Value<'_>,
    path: &str,
    field: &str,
) -> std::result::Result<Option<String>, String> {
    let value = optional_field_string(value, path, field)?;
    if value
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(format!("{path}.{field} should be non-empty when set"));
    }
    Ok(value)
}

pub(super) fn field_bool(
    value: Value<'_>,
    path: &str,
    field: &str,
) -> std::result::Result<bool, String> {
    let value = field_value(value, path, field)?;
    value.unpack_bool().ok_or_else(|| {
        format!(
            "{path}.{field} should be a bool, got `{}`",
            value.get_type()
        )
    })
}

pub(super) fn field_non_negative_usize(
    value: Value<'_>,
    path: &str,
    field: &str,
) -> std::result::Result<usize, String> {
    let value = field_value(value, path, field)?;
    let integer = value.unpack_i32().ok_or_else(|| {
        format!(
            "{path}.{field} should be a non-negative integer, got `{}`",
            value.get_type()
        )
    })?;
    usize::try_from(integer).map_err(|_| format!("{path}.{field} should be a non-negative integer"))
}

pub(super) fn optional_field_non_negative_usize(
    value: Value<'_>,
    path: &str,
    field: &str,
) -> std::result::Result<Option<usize>, String> {
    let value = field_value(value, path, field)?;
    if value.is_none() {
        return Ok(None);
    }
    let integer = value.unpack_i32().ok_or_else(|| {
        format!(
            "{path}.{field} should be a non-negative integer or None, got `{}`",
            value.get_type()
        )
    })?;
    usize::try_from(integer)
        .map(Some)
        .map_err(|_| format!("{path}.{field} should be a non-negative integer or None"))
}

pub(super) fn field_list<'v>(
    value: Value<'v>,
    path: &str,
    field: &str,
) -> std::result::Result<&'v ListRef<'v>, String> {
    let value = field_value(value, path, field)?;
    list(value, &format!("{path}.{field}"))
}

pub(super) fn field_string_list(
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

pub(super) fn list<'v>(
    value: Value<'v>,
    path: &str,
) -> std::result::Result<&'v ListRef<'v>, String> {
    ListRef::from_value(value)
        .ok_or_else(|| format!("{path} should be a list, got `{}`", value.get_type()))
}

pub(super) fn script_schema() -> TargetKindSchema {
    TargetKindSchema {
        kind: "script".to_string(),
        docs: "Adapter target that wraps existing executable automation.".to_string(),
        has_resolver: false,
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
        tools: Vec::new(),
        source_references: Vec::new(),
        examples: Vec::new(),
    }
}

pub(super) fn attr(
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
        implemented: true,
        allowed_values: Vec::new(),
        disallowed_values: Vec::new(),
    }
}

pub(super) fn capability(
    name: &str,
    output_groups: &[&str],
    requires_outputs: &[&str],
) -> Capability {
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
