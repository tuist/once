//! Typed build graph model and built-in target kind metadata.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::analysis::select_branches;
use crate::error::Result;
use crate::manifest::BuildConfiguration;
use crate::target::{AttrValue, Target};
use crate::workspace::{load_workspace, load_workspace_with_configuration};

mod prelude_schemas;

use prelude_schemas::{
    append_script_schema, parse_target_kind_schemas_with_context, prelude_message, script_schema,
    starlark_prelude_target_kind_schemas, validate_unique_target_kinds, TargetKindSchemaSource,
};
pub(crate) use prelude_schemas::{prelude_exports, target_kind_schemas_from_compiled_exports};

/// Fully qualified graph target label.
#[derive(Deserialize, Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TargetLabel {
    /// Package path that owns the target.
    pub package: String,
    /// Target name inside the package manifest.
    pub name: String,
    /// Canonical target id, formed from package and name.
    pub id: String,
}

/// Target record after manifest loading and target kind metadata attachment.
#[derive(Deserialize, Debug, Clone, Serialize, PartialEq)]
pub struct GraphTarget {
    /// Canonical target label.
    pub label: TargetLabel,
    /// Target kind declared by the target manifest. Matched against exported
    /// Starlark target kind symbols to attach schema, capabilities, and
    /// providers.
    pub kind: String,
    /// Canonical dependency target ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    /// Provider-checked dependency ids grouped by target-kind-defined role.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependency_edges: BTreeMap<String, Vec<String>>,
    /// Source file patterns declared by the target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub srcs: Vec<String>,
    /// Targets and packages allowed to depend on this target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visibility: Vec<String>,
    /// Typed target attributes parsed from the manifest.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attrs: BTreeMap<String, AttrValue>,
    /// Operations exposed by the target kind schema.
    pub capabilities: Vec<Capability>,
    /// Providers emitted by this target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
    /// Tools required to analyze or execute this target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolRequirement>,
    /// Non-fatal graph loading diagnostics for this target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

impl GraphTarget {
    /// Every dependency id, including the legacy `deps` role and named roles.
    pub fn dependency_ids(&self) -> impl Iterator<Item = &String> {
        self.deps
            .iter()
            .chain(self.dependency_edges.values().flatten())
    }
}

/// Operation exposed by a target kind, such as build, lint, run, or test.
#[derive(Deserialize, Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Capability {
    /// Capability name.
    pub name: String,
    /// Output groups produced by this capability.
    pub output_groups: Vec<String>,
    /// Output groups that must already exist before this capability runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires_outputs: Vec<String>,
}

/// A tool made available to a target through the workspace tool environment.
#[derive(Deserialize, Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ToolRequirement {
    /// Logical tool name declared by the workspace.
    pub name: String,
    /// Executable names the target kind may invoke from this tool.
    pub executables: Vec<String>,
}

/// Diagnostic emitted while constructing the typed graph.
#[derive(Deserialize, Debug, Clone, Serialize, PartialEq, Eq)]
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

impl Diagnostic {
    /// Create a diagnostic with no target, attribute, or repairs.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            target: None,
            attribute: None,
            repairs: Vec::new(),
        }
    }

    /// Scope this diagnostic to a target.
    #[must_use]
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// Scope this diagnostic to an attribute.
    #[must_use]
    pub fn with_attribute(mut self, attribute: impl Into<String>) -> Self {
        self.attribute = Some(attribute.into());
        self
    }

    /// Add one suggested repair.
    #[must_use]
    pub fn with_repair(mut self, repair: impl Into<String>) -> Self {
        self.repairs.push(repair.into());
        self
    }
}

/// Target kind metadata used for schema queries and graph target enrichment.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TargetKindSchema {
    /// Target kind matched by `target.kind`.
    pub kind: String,
    /// Human-readable target kind description.
    pub docs: String,
    /// Attribute schema for this target kind.
    pub attrs: Vec<AttrSchema>,
    /// Whether this target kind expands locked graph data before validation.
    #[serde(skip)]
    pub(crate) has_resolver: bool,
    /// Dependency expectations for this target kind.
    pub deps: Vec<DepSchema>,
    /// Providers emitted by targets of this kind.
    pub providers: Vec<String>,
    /// Capabilities exposed by targets of this kind.
    pub capabilities: Vec<Capability>,
    /// Tools required by implementations of this target kind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolRequirement>,
    /// External build-system concepts that this target kind can replace or
    /// reproduce during incremental adoption.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_references: Vec<SourceReference>,
    /// Runnable starter workspaces. Each example is a lightweight
    /// descriptor; callers load the file bundle only when they choose
    /// a starter to materialize.
    pub examples: Vec<TargetKindExample>,
}

impl TargetKindSchema {
    /// Whether this target kind expands an authoritative project graph.
    pub fn has_resolver(&self) -> bool {
        self.has_resolver
    }
}

/// External build-system concept related to a target kind.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SourceReference {
    /// Name of the source build system or rule set.
    pub system: String,
    /// Rule, plug-in, task family, or other source symbol.
    pub symbol: String,
    /// Primary documentation for the source concept.
    pub url: String,
    /// Selection guidance for partial graph adoption.
    pub use_when: String,
    /// Digest returned when the authoritative source was fetched. A missing
    /// value indicates a documentation-only reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<String>,
}

/// A runnable starter descriptor for a target kind.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TargetKindExample {
    /// Human-readable example title.
    pub name: String,
    /// Stable identifier used to reference this example.
    pub slug: String,
    /// One-line "use this when..." hint that helps callers choose
    /// between examples for the same target kind.
    pub use_when: String,
    /// Host platforms whose toolchain the starter's resolver needs to load.
    /// Empty means the starter loads on any platform. Named platforms use
    /// [`std::env::consts::OS`] values such as `macos`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platforms: Vec<String>,
    /// Where the starter file tree lives. The wire schema omits this so
    /// discovery remains independent from local package layout.
    #[serde(skip_serializing)]
    pub source: TargetKindExampleSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetKindExampleSource {
    pub root: TargetKindExampleRoot,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetKindExampleRoot {
    BuiltInPrelude,
    Workspace { root: PathBuf },
}

/// A materialized starter workspace for a target kind.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TargetKindExampleBundle {
    /// Human-readable example title.
    pub name: String,
    /// Stable identifier used to reference this example.
    pub slug: String,
    /// One-line "use this when..." hint that helps callers choose
    /// between examples for the same target kind.
    pub use_when: String,
    /// Every file in the example bundle, sorted by path.
    pub files: Vec<TargetKindExampleFile>,
}

/// A single file inside a [`TargetKindExample`].
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TargetKindExampleFile {
    /// Path relative to the example workspace root.
    pub path: String,
    /// File contents as a UTF-8 string.
    pub contents: String,
    /// Binary file contents encoded as Base64. This is absent for text files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contents_base64: Option<String>,
}

impl TargetKindExampleFile {
    pub fn decoded_contents(&self) -> std::result::Result<Vec<u8>, String> {
        use base64::Engine as _;

        match &self.contents_base64 {
            Some(contents) => base64::engine::general_purpose::STANDARD
                .decode(contents)
                .map_err(|error| format!("invalid Base64 contents for `{}`: {error}", self.path)),
            None => Ok(self.contents.as_bytes().to_vec()),
        }
    }
}

/// Attribute metadata exposed by a target kind schema.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AttrSchema {
    /// Attribute name under `[target.attrs]`.
    pub name: String,
    /// Human-readable type name.
    pub ty: String,
    /// Whether the attribute must be present.
    pub required: bool,
    /// Default value rendered as target kind metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Human-readable attribute description.
    pub docs: String,
    /// Whether the value can vary by configuration.
    pub configurable: bool,
    /// Whether the target kind currently implements the attribute.
    pub implemented: bool,
    /// Accepted string values. An empty list accepts every value of the type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_values: Vec<String>,
    /// Rejected string values. Surrounding whitespace is ignored during validation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disallowed_values: Vec<String>,
}

/// Dependency metadata exposed by a target kind schema.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DepSchema {
    /// Dependency attribute name.
    pub name: String,
    /// Providers accepted by this dependency edge.
    pub expected_providers: Vec<String>,
    /// Human-readable dependency description.
    pub docs: String,
    /// Minimum number of dependencies accepted by this edge.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub min_count: usize,
    /// Maximum number of dependencies accepted by this edge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_count: Option<usize>,
}

pub fn load_graph_workspace(root: &Path) -> Result<Vec<GraphTarget>> {
    let schemas = target_kind_schemas_for_workspace(root)?;
    load_graph_workspace_with_targets_and_schemas(root, load_workspace(root)?, &schemas)
}

/// Load the graph using an explicit configuration, layered over the
/// workspace-declared one. Mirrors [`load_graph_workspace`] for callers
/// that carry invocation-time overrides.
pub fn load_graph_workspace_with_configuration(
    root: &Path,
    configuration: &BuildConfiguration,
) -> Result<Vec<GraphTarget>> {
    let schemas = target_kind_schemas_for_workspace(root)?;
    load_graph_workspace_with_targets_and_schemas(
        root,
        load_workspace_with_configuration(root, configuration)?,
        &schemas,
    )
}

pub(crate) fn load_graph_workspace_with_compiled_schemas(
    root: &Path,
    compiled_schemas: &[TargetKindSchema],
    options: &crate::analysis::AnalysisOptions,
) -> Result<Vec<GraphTarget>> {
    load_graph_workspace_with_compiled_schemas_and_targets(
        root,
        compiled_schemas,
        load_workspace(root)?,
        options,
    )
}

pub(crate) fn load_graph_workspace_with_compiled_schemas_and_targets(
    root: &Path,
    compiled_schemas: &[TargetKindSchema],
    targets: Vec<Target>,
    options: &crate::analysis::AnalysisOptions,
) -> Result<Vec<GraphTarget>> {
    Ok(
        load_graph_workspace_with_compiled_schemas_and_targets_recorded(
            root,
            compiled_schemas,
            targets,
            options,
        )?
        .0,
    )
}

pub(crate) fn load_graph_workspace_with_compiled_schemas_and_targets_recorded(
    root: &Path,
    compiled_schemas: &[TargetKindSchema],
    targets: Vec<Target>,
    options: &crate::analysis::AnalysisOptions,
) -> Result<(Vec<GraphTarget>, crate::resolution::ResolutionRecord)> {
    let schemas = target_kind_schemas_for_workspace_from_compiled(root, compiled_schemas)?;
    load_graph_workspace_recorded(root, targets, &schemas, options)
}

pub(crate) fn load_graph_workspace_with_targets_and_schemas(
    root: &Path,
    targets: Vec<Target>,
    schemas: &[TargetKindSchema],
) -> Result<Vec<GraphTarget>> {
    Ok(load_graph_workspace_recorded(
        root,
        targets,
        schemas,
        &crate::analysis::AnalysisOptions::default(),
    )?
    .0)
}

/// Load the graph and report what deriving it read, so a caller that can tell
/// whether any of it moved may skip deriving it next time.
pub(crate) fn load_graph_workspace_recorded(
    root: &Path,
    targets: Vec<Target>,
    schemas: &[TargetKindSchema],
    options: &crate::analysis::AnalysisOptions,
) -> Result<(Vec<GraphTarget>, crate::resolution::ResolutionRecord)> {
    let mut expanded =
        crate::resolution::expand_workspace_targets(root, targets, schemas, options)?;
    let record = std::mem::take(&mut expanded.record);
    let mut graph = graph_from_owned_targets_with_schemas(expanded.targets, schemas);
    for target in &mut graph {
        let Some(diagnostics) = expanded.diagnostics.remove(&target.label.id) else {
            continue;
        };
        for diagnostic in diagnostics {
            if !target.diagnostics.contains(&diagnostic) {
                target.diagnostics.push(diagnostic);
            }
        }
    }
    Ok((graph, record))
}

#[must_use]
pub fn graph_from_targets(targets: &[Target]) -> Vec<GraphTarget> {
    let schemas = built_in_target_kind_schemas();
    graph_from_targets_with_schemas(targets, &schemas)
}

pub fn graph_from_targets_result(targets: &[Target]) -> Result<Vec<GraphTarget>> {
    let schemas = built_in_target_kind_schemas_result()?;
    Ok(graph_from_targets_with_schemas(targets, &schemas))
}

fn graph_from_targets_with_schemas(
    targets: &[Target],
    schemas: &[TargetKindSchema],
) -> Vec<GraphTarget> {
    targets
        .iter()
        .map(|target| graph_target_from_schema(target, schemas))
        .collect()
}

fn graph_from_owned_targets_with_schemas(
    targets: Vec<Target>,
    schemas: &[TargetKindSchema],
) -> Vec<GraphTarget> {
    targets
        .into_iter()
        .map(|target| graph_target_from_owned_schema(target, schemas))
        .collect()
}

#[must_use]
pub fn built_in_target_kind_schemas() -> Vec<TargetKindSchema> {
    let Ok(schemas) = built_in_target_kind_schemas_result() else {
        return vec![script_schema()];
    };
    schemas
}

pub fn built_in_target_kind_schemas_result() -> Result<Vec<TargetKindSchema>> {
    let mut schemas = starlark_prelude_target_kind_schemas()?;
    append_script_schema(&mut schemas)?;
    Ok(schemas)
}

pub fn target_kind_schemas_for_workspace(root: &Path) -> Result<Vec<TargetKindSchema>> {
    let mut schemas = starlark_prelude_target_kind_schemas()?;
    schemas.extend(workspace_module_target_kind_schemas(root)?);
    validate_unique_target_kinds(&schemas)
        .map_err(|message| prelude_message(crate::modules::COMBINED_MODULE_PATH, &message))?;
    append_script_schema(&mut schemas)?;
    Ok(schemas)
}

fn target_kind_schemas_for_workspace_from_compiled(
    root: &Path,
    compiled_schemas: &[TargetKindSchema],
) -> Result<Vec<TargetKindSchema>> {
    let workspace_schemas = workspace_module_target_kind_schemas(root)?;
    let workspace_kinds = workspace_schemas
        .iter()
        .map(|schema| schema.kind.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut schemas = compiled_schemas
        .iter()
        .filter(|schema| !workspace_kinds.contains(schema.kind.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    schemas.extend(workspace_schemas);
    validate_unique_target_kinds(&schemas)
        .map_err(|message| prelude_message(crate::modules::COMBINED_MODULE_PATH, &message))?;
    append_script_schema(&mut schemas)?;
    Ok(schemas)
}

fn workspace_module_target_kind_schemas(root: &Path) -> Result<Vec<TargetKindSchema>> {
    let mut schemas = Vec::new();
    let common = crate::modules::common_module_source();
    for module_file in crate::modules::load_module_files(root)? {
        let source_context = TargetKindSchemaSource::workspace(root, &module_file.display_path);
        schemas.extend(parse_target_kind_schemas_with_context(
            &module_file.display_path,
            &format!("{common}\n{}", module_file.source),
            &source_context,
        )?);
    }
    Ok(schemas)
}

pub fn validate_module_source(
    root: &Path,
    path: &str,
    source: &str,
) -> Result<Vec<TargetKindSchema>> {
    let common = crate::modules::common_module_source();
    parse_target_kind_schemas_with_context(
        path,
        &format!("{common}\n{source}"),
        &TargetKindSchemaSource::workspace(root, path),
    )
}

#[must_use]
pub fn built_in_target_kind_schema(kind: &str) -> Option<TargetKindSchema> {
    built_in_target_kind_schemas()
        .into_iter()
        .find(|schema| schema.kind == kind)
}

impl From<&Target> for GraphTarget {
    fn from(target: &Target) -> Self {
        graph_target_from_schema(target, &built_in_target_kind_schemas())
    }
}

pub(crate) fn graph_target_from_schema(
    target: &Target,
    schemas: &[TargetKindSchema],
) -> GraphTarget {
    let schema = schemas.iter().find(|schema| schema.kind == target.kind);
    let target_id = target.id();
    let mut diagnostics = if schema.is_some() {
        Vec::new()
    } else {
        vec![Diagnostic::new(
            "unknown_target_kind",
            format!("target kind `{}` has no target kind schema", target.kind),
        )
        .with_target(target_id.as_str())]
    };
    let attrs = graph_attrs(target);
    if let Some(schema) = schema {
        for attr_schema in &schema.attrs {
            if attr_schema.configurable {
                continue;
            }
            if let Some(value) = attrs.get(&attr_schema.name) {
                if select_branches(value).is_some() {
                    diagnostics.push(
                        Diagnostic::new(
                            "select_on_non_configurable_attr",
                            format!(
                                "attribute `{}` is not configurable but uses `select()`",
                                attr_schema.name
                            ),
                        )
                        .with_target(target_id.as_str())
                        .with_attribute(attr_schema.name.as_str()),
                    );
                }
            }
        }
    }
    let tools = graph_tools_from_parts(&target.kind, &attrs, schema);
    GraphTarget {
        label: TargetLabel {
            package: target.package.clone(),
            name: target.name.clone(),
            id: target_id,
        },
        kind: target.kind.clone(),
        deps: target.deps.clone(),
        dependency_edges: target.dependency_edges.clone(),
        srcs: target.srcs.clone(),
        visibility: target.visibility.clone(),
        attrs,
        capabilities: schema
            .as_ref()
            .map_or_else(Vec::new, |schema| schema.capabilities.clone()),
        providers: schema.map_or_else(Vec::new, |schema| schema.providers.clone()),
        tools,
        diagnostics,
    }
}

fn graph_target_from_owned_schema(target: Target, schemas: &[TargetKindSchema]) -> GraphTarget {
    let Target {
        // Only meaningful while gathering resolver inputs, which has already
        // happened by the time a target becomes a graph target.
        resolver_input_exclude: _,
        package,
        kind,
        name,
        deps,
        dependency_edges,
        srcs,
        visibility,
        attrs,
        typed_attrs,
    } = target;
    let schema = schemas.iter().find(|schema| schema.kind == kind);
    let target_id = crate::target_ref::target_id(&package, &name);
    let mut diagnostics = if schema.is_some() {
        Vec::new()
    } else {
        vec![Diagnostic::new(
            "unknown_target_kind",
            format!("target kind `{kind}` has no target kind schema"),
        )
        .with_target(target_id.as_str())]
    };
    let attrs = graph_attrs_from_parts(attrs, typed_attrs);
    if let Some(schema) = schema {
        for attr_schema in &schema.attrs {
            if attr_schema.configurable {
                continue;
            }
            if let Some(value) = attrs.get(&attr_schema.name) {
                if select_branches(value).is_some() {
                    diagnostics.push(
                        Diagnostic::new(
                            "select_on_non_configurable_attr",
                            format!(
                                "attribute `{}` is not configurable but uses `select()`",
                                attr_schema.name
                            ),
                        )
                        .with_target(target_id.as_str())
                        .with_attribute(attr_schema.name.as_str()),
                    );
                }
            }
        }
    }
    let tools = graph_tools_from_parts(&kind, &attrs, schema);
    GraphTarget {
        label: TargetLabel {
            package,
            name,
            id: target_id,
        },
        kind,
        deps,
        dependency_edges,
        srcs,
        visibility,
        attrs,
        capabilities: schema
            .as_ref()
            .map_or_else(Vec::new, |schema| schema.capabilities.clone()),
        providers: schema.map_or_else(Vec::new, |schema| schema.providers.clone()),
        tools,
        diagnostics,
    }
}

fn graph_tools_from_parts(
    kind: &str,
    attrs: &BTreeMap<String, AttrValue>,
    schema: Option<&TargetKindSchema>,
) -> Vec<ToolRequirement> {
    let mut tools = schema.map_or_else(Vec::new, |schema| schema.tools.clone());
    if kind == "script" {
        if let Some(AttrValue::String(runtime)) = attrs.get("script_runtime") {
            if !runtime.contains('/') && !runtime.contains('\\') {
                tools.push(ToolRequirement {
                    name: runtime.clone(),
                    executables: vec![runtime.clone()],
                });
            }
        }
    }
    tools
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

fn graph_attrs_from_parts(
    attrs: BTreeMap<String, String>,
    typed_attrs: BTreeMap<String, AttrValue>,
) -> BTreeMap<String, AttrValue> {
    if !typed_attrs.is_empty() {
        return typed_attrs;
    }
    attrs
        .into_iter()
        .map(|(key, value)| (key, AttrValue::String(value)))
        .collect()
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(value: &usize) -> bool {
    *value == 0
}

#[cfg(test)]
mod tests;
