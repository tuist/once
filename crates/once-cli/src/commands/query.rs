//! `once query` - inspect the typed build graph.

mod expression;
mod external_source;
mod script;
pub(crate) mod test_plan;

pub(crate) use script::script_validation_value;

use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use once_core::{
    EvidenceRecord, EvidenceStore, InputDigestBuilder, TestBatchAttempt, TestManifest,
    TestTimingStore, TestUnit, WorkspacePath,
};
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::cli::{Format, Output};
use crate::render;

#[derive(Debug, PartialEq, Eq, Serialize)]
struct TargetRecord {
    id: String,
    package: String,
    name: String,
    kind: String,
    deps: Vec<String>,
    dependency_edges: std::collections::BTreeMap<String, Vec<String>>,
    capabilities: Vec<String>,
    tools: Vec<once_frontend::ToolRequirement>,
}

#[derive(Debug, Serialize)]
struct TestTargetRecord {
    id: String,
    kind: String,
    deps: Vec<String>,
    runner: Option<String>,
    labels: Vec<String>,
    results_path: Option<String>,
}

#[derive(Debug, Serialize)]
struct WorkspaceValidation {
    valid: bool,
    target_count: usize,
    diagnostics: Vec<once_frontend::Diagnostic>,
}

#[derive(Debug, Serialize)]
struct ModuleValidation {
    valid: bool,
    path: String,
    target_kinds: Vec<once_frontend::TargetKindSchema>,
    diagnostics: Vec<ModuleDiagnostic>,
}

#[derive(Debug, Serialize)]
struct ModuleDiagnostic {
    code: &'static str,
    message: String,
    repairs: Vec<&'static str>,
}

pub async fn targets(workspace: &Path, output: Output, kind: Option<&str>) -> Result<()> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    let records = target_records(graph, kind);
    write_body(output, || render_targets_human(&records), &records).await
}

pub async fn expression(workspace: &Path, output: Output, query: &str) -> Result<()> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    let result = expression::evaluate(query, &graph)?;
    write_body(output, || expression::render_human(&result), &result).await
}

pub async fn script(workspace: &Path, output: Output, path: &str) -> Result<()> {
    script::inspect(workspace, output, path).await
}

pub async fn validate_workspace(workspace: &Path, output: Output) -> Result<()> {
    let validation = workspace_validation(workspace)?;
    write_body(
        output,
        || render_workspace_validation(&validation),
        &validation,
    )
    .await
}

pub async fn validate_actions(
    workspace: &Path,
    output: Output,
    target: &str,
    capability: &str,
    action: Option<usize>,
) -> Result<()> {
    let xdg = once_core::Xdg::from_env();
    let cache = crate::cache_provider::resolve(workspace, &xdg)?;
    let validation = crate::commands::graph::validate_action_contracts(
        workspace, &cache, target, capability, action,
    )
    .await?;
    write_body(
        output,
        || render_action_validation_human(&validation),
        &validation,
    )
    .await
}

fn render_action_validation_human(
    validation: &crate::commands::graph::ActionContractValidation,
) -> String {
    let mut text = format!(
        "{} {}: {} ({} actions)\n",
        validation.target,
        validation.capability,
        if validation.valid { "valid" } else { "invalid" },
        validation.actions_run
    );
    for diagnostic in &validation.diagnostics {
        let _ = writeln!(text, "  {}: {}", diagnostic.code, diagnostic.message);
        for repair in &diagnostic.repairs {
            let _ = writeln!(text, "    repair: {repair}");
        }
    }
    for limitation in &validation.limitations {
        let _ = writeln!(text, "  limitation: {limitation}");
    }
    text
}

pub async fn module_contract(output: Output) -> Result<()> {
    let contract = once_frontend::module_authoring_contract();
    write_body(
        output,
        || "project module authoring contract\n".to_string(),
        &contract,
    )
    .await
}

pub(crate) fn module_contract_value() -> Result<serde_json::Value> {
    Ok(serde_json::to_value(
        once_frontend::module_authoring_contract(),
    )?)
}

pub async fn external_source(output: Output, url: &str, max_bytes: usize) -> Result<()> {
    let url = url.to_string();
    let source = tokio::task::spawn_blocking(move || external_source::fetch(&url, max_bytes))
        .await
        .context("joining external source fetch")??;
    write_body(
        output,
        || {
            format!(
                "external source: {} bytes{}\n{}",
                source.byte_count,
                if source.truncated { " (truncated)" } else { "" },
                source.content
            )
        },
        &source,
    )
    .await
}

pub(crate) fn external_source_value(url: &str, max_bytes: usize) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(external_source::fetch(
        url, max_bytes,
    )?)?)
}

pub async fn validate_module(workspace: &Path, output: Output, path: &str) -> Result<()> {
    let validation = module_validation(workspace, path)?;
    write_body(
        output,
        || render_module_validation(&validation),
        &validation,
    )
    .await
}

pub(crate) fn module_validation_value(workspace: &Path, path: &str) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(module_validation(workspace, path)?)?)
}

fn module_validation(workspace: &Path, path: &str) -> Result<ModuleValidation> {
    let relative = once_core::WorkspacePath::try_from(path)
        .map_err(|error| anyhow!("invalid module path: {error}"))?;
    if relative.as_str().is_empty() {
        anyhow::bail!("module path must name a Starlark file");
    }
    let source_path = relative.resolve(workspace);
    let source = std::fs::read_to_string(&source_path)
        .with_context(|| format!("reading module `{}`", source_path.display()))?;
    match once_frontend::validate_module_source(workspace, relative.as_str(), &source) {
        Ok(target_kinds) => Ok(ModuleValidation {
            valid: true,
            path: relative.to_string(),
            target_kinds,
            diagnostics: Vec::new(),
        }),
        Err(error) => Ok(ModuleValidation {
            valid: false,
            path: relative.to_string(),
            target_kinds: Vec::new(),
            diagnostics: vec![ModuleDiagnostic {
                code: "invalid_module",
                message: error.to_string(),
                repairs: vec![
                    "Compare the module with once query module-contract.",
                    "Fix the reported Starlark or target kind contract error and validate again.",
                ],
            }],
        }),
    }
}

fn render_module_validation(validation: &ModuleValidation) -> String {
    if validation.valid {
        return format!(
            "module valid: {} ({} target kinds)\n",
            validation.path,
            validation.target_kinds.len()
        );
    }
    format!(
        "module invalid: {}\n{}\n",
        validation.path, validation.diagnostics[0].message
    )
}

pub(crate) fn workspace_validation_value(workspace: &Path) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(workspace_validation(workspace)?)?)
}

fn workspace_validation(workspace: &Path) -> Result<WorkspaceValidation> {
    let target_count = once_frontend::load_graph_workspace(workspace)?.len();
    let diagnostics = once_frontend::validate_workspace(workspace)?;
    Ok(WorkspaceValidation {
        valid: diagnostics.is_empty(),
        target_count,
        diagnostics,
    })
}

fn render_workspace_validation(validation: &WorkspaceValidation) -> String {
    if validation.valid {
        return format!("workspace valid: {} targets\n", validation.target_count);
    }
    let mut out = format!(
        "workspace invalid: {} diagnostics across {} targets\n",
        validation.diagnostics.len(),
        validation.target_count
    );
    for diagnostic in &validation.diagnostics {
        let _ = writeln!(out, "{}: {}", diagnostic.code, diagnostic.message);
    }
    out
}

fn target_records(graph: Vec<once_frontend::GraphTarget>, kind: Option<&str>) -> Vec<TargetRecord> {
    graph
        .into_iter()
        .filter(|target| kind.is_none_or(|kind| target.kind == kind))
        .map(|target| TargetRecord {
            id: target.label.id,
            package: target.label.package,
            name: target.label.name,
            kind: target.kind,
            deps: target.deps,
            dependency_edges: target.dependency_edges,
            capabilities: target
                .capabilities
                .into_iter()
                .map(|capability| capability.name)
                .collect(),
            tools: target.tools,
        })
        .collect::<Vec<_>>()
}

pub async fn capabilities(workspace: &Path, output: Output, target_id: &str) -> Result<()> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    let target = graph
        .into_iter()
        .find(|target| target.label.id == target_id)
        .with_context(|| format!("no target matches `{target_id}`"))?;
    write_body(output, || render_capabilities_human(&target), &target).await
}

pub async fn schema(workspace: &Path, output: Output, kind: &str) -> Result<()> {
    let schema = once_frontend::target_kind_schemas_for_workspace(workspace)?
        .into_iter()
        .find(|schema| schema.kind == kind)
        .with_context(|| format!("no target kind schema matches `{kind}`"))?;
    write_body(output, || render_schema_human(&schema), &schema).await
}

pub async fn example(workspace: &Path, output: Output, kind: &str, slug: &str) -> Result<()> {
    let schema = once_frontend::target_kind_schemas_for_workspace(workspace)?
        .into_iter()
        .find(|schema| schema.kind == kind)
        .with_context(|| format!("no target kind schema matches `{kind}`"))?;
    let example = once_frontend::load_target_kind_example(&schema, slug)?;
    write_body(output, || render_example_human(&example), &example).await
}

#[derive(Debug, Serialize)]
struct TargetKindSummary {
    kind: String,
    docs: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    source_references: Vec<once_frontend::SourceReference>,
    examples: Vec<TargetKindExampleSummary>,
}

#[derive(Debug, Serialize)]
struct TargetKindExampleSummary {
    slug: String,
    name: String,
    use_when: String,
}

impl From<once_frontend::TargetKindSchema> for TargetKindSummary {
    fn from(schema: once_frontend::TargetKindSchema) -> Self {
        Self {
            kind: schema.kind,
            docs: schema.docs,
            source_references: schema.source_references,
            examples: schema
                .examples
                .into_iter()
                .map(|example| TargetKindExampleSummary {
                    slug: example.slug,
                    name: example.name,
                    use_when: example.use_when,
                })
                .collect(),
        }
    }
}

pub async fn target_kinds(workspace: &Path, output: Output, query: Option<&str>) -> Result<()> {
    let schemas = matching_target_kind_schemas(workspace, query)?;
    let summaries: Vec<TargetKindSummary> =
        schemas.into_iter().map(TargetKindSummary::from).collect();
    write_body(output, || render_target_kinds_human(&summaries), &summaries).await
}

pub(crate) fn matching_target_kind_schemas(
    workspace: &Path,
    query: Option<&str>,
) -> Result<Vec<once_frontend::TargetKindSchema>> {
    let mut schemas = once_frontend::target_kind_schemas_for_workspace(workspace)?;
    let terms = query
        .into_iter()
        .flat_map(|query| query.split(|character: char| !character.is_alphanumeric()))
        .map(str::to_lowercase)
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();
    if terms.is_empty() {
        return Ok(schemas);
    }

    let families = terms
        .iter()
        .filter(|term| {
            schemas
                .iter()
                .any(|schema| target_kind_family(schema) == term.as_str())
        })
        .collect::<Vec<_>>();
    if !families.is_empty() {
        let specific_terms = terms
            .iter()
            .filter(|term| {
                !generic_target_kind_query_term(term)
                    && !families
                        .iter()
                        .any(|family| family.as_str() == term.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        schemas.retain(|schema| {
            families
                .iter()
                .any(|family| target_kind_family(schema) == family.as_str())
                || target_kind_matches_terms(schema, &specific_terms)
        });
        return Ok(schemas);
    }

    if let Some(segment) = terms.iter().find(|term| {
        schemas
            .iter()
            .any(|schema| target_kind_segments(schema).any(|candidate| candidate == term.as_str()))
    }) {
        schemas.retain(|schema| {
            target_kind_segments(schema).any(|candidate| candidate == segment.as_str())
        });
        return Ok(schemas);
    }

    schemas.retain(|schema| target_kind_matches_terms(schema, &terms));
    Ok(schemas)
}

fn generic_target_kind_query_term(term: &str) -> bool {
    matches!(
        term,
        "app"
            | "application"
            | "binary"
            | "build"
            | "existing"
            | "executable"
            | "kind"
            | "kinds"
            | "libraries"
            | "library"
            | "mixed"
            | "native"
            | "repository"
            | "runner"
            | "runners"
            | "target"
            | "targets"
            | "test"
            | "tests"
            | "typed"
    )
}

fn target_kind_family(schema: &once_frontend::TargetKindSchema) -> &str {
    schema.kind.split('_').next().unwrap_or(&schema.kind)
}

fn target_kind_segments(schema: &once_frontend::TargetKindSchema) -> impl Iterator<Item = &str> {
    schema.kind.split('_')
}

fn target_kind_matches_terms(schema: &once_frontend::TargetKindSchema, terms: &[String]) -> bool {
    let mut searchable = format!("{} {}", schema.kind, schema.docs);
    for example in &schema.examples {
        searchable.push(' ');
        searchable.push_str(&example.slug);
        searchable.push(' ');
        searchable.push_str(&example.name);
        searchable.push(' ');
        searchable.push_str(&example.use_when);
    }
    for reference in &schema.source_references {
        searchable.push(' ');
        searchable.push_str(&reference.system);
        searchable.push(' ');
        searchable.push_str(&reference.symbol);
        searchable.push(' ');
        searchable.push_str(&reference.use_when);
    }
    let searchable_terms = searchable
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    // Match a term against whole tokens, but allow it to be a substring of a token
    // so a partial query still resolves (for example `spec` finding the `rspec`
    // runner) while staying scoped to individual words rather than the whole blob.
    terms.iter().any(|term| {
        searchable_terms
            .iter()
            .any(|candidate| candidate.contains(term.as_str()))
    })
}

pub async fn target(workspace: &Path, output: Output, target_id: &str) -> Result<()> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    let target = graph
        .into_iter()
        .find(|target| target.label.id == target_id)
        .with_context(|| format!("no target matches `{target_id}`"))?;
    write_body(output, || render_target_human(&target), &target).await
}

pub async fn tests(workspace: &Path, output: Output) -> Result<()> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    let records = test_records(workspace, &graph);
    write_body(output, || render_tests_human(&records), &records).await
}

pub(crate) fn tests_value(workspace: &Path) -> Result<serde_json::Value> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    Ok(serde_json::to_value(test_records(workspace, &graph))?)
}

pub async fn affected_tests(
    workspace: &Path,
    output: Output,
    changed_paths: &[String],
) -> Result<()> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    let records = test_plan::affected_tests(workspace, &graph, changed_paths)?;
    write_body(output, || render_affected_tests_human(&records), &records).await
}

pub(crate) fn affected_tests_value(
    workspace: &Path,
    changed_paths: &[String],
) -> Result<serde_json::Value> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    Ok(serde_json::to_value(test_plan::affected_tests(
        workspace,
        &graph,
        changed_paths,
    )?)?)
}

pub async fn test_plan_request(
    workspace: &Path,
    output: Output,
    changed_paths: &[String],
    target: Option<&str>,
    test_unit: Option<&str>,
) -> Result<()> {
    let plan = match (target, test_unit) {
        (Some(target), Some(test_unit)) => explicit_test_unit_plan(workspace, target, test_unit)?,
        (Some(target), None) => explicit_test_plan(workspace, &[target.to_string()])?,
        (None, None) => test_plan_for_paths(workspace, changed_paths)?,
        (None, Some(_)) => anyhow::bail!("a test unit requires an explicit target"),
    };
    write_body(output, || render_test_plan_human(&plan), &plan).await
}

pub(crate) fn test_plan_for_paths(
    workspace: &Path,
    changed_paths: &[String],
) -> Result<test_plan::TestPlan> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    test_plan_for_paths_with_graph(workspace, &graph, changed_paths)
}

pub(crate) fn test_plan_for_paths_with_graph(
    workspace: &Path,
    graph: &[once_frontend::GraphTarget],
    changed_paths: &[String],
) -> Result<test_plan::TestPlan> {
    test_plan::plan(workspace, graph, changed_paths)
}

pub(crate) fn explicit_test_plan(
    workspace: &Path,
    targets: &[String],
) -> Result<test_plan::TestPlan> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    explicit_test_plan_with_graph(workspace, &graph, targets)
}

pub(crate) fn explicit_test_plan_with_graph(
    workspace: &Path,
    graph: &[once_frontend::GraphTarget],
    targets: &[String],
) -> Result<test_plan::TestPlan> {
    test_plan::explicit_plan(workspace, graph, targets)
}

pub(crate) fn explicit_test_unit_plan(
    workspace: &Path,
    target: &str,
    test_unit: &str,
) -> Result<test_plan::TestPlan> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    explicit_test_unit_plan_with_graph(workspace, &graph, target, test_unit)
}

pub(crate) fn explicit_test_unit_plan_with_graph(
    workspace: &Path,
    graph: &[once_frontend::GraphTarget],
    target: &str,
    test_unit: &str,
) -> Result<test_plan::TestPlan> {
    let manifest = test_manifest_record(workspace, target)?;
    if !test_manifest_is_current(workspace, target, &manifest) {
        anyhow::bail!(
            "the test manifest for `{target}` is stale; run the whole target to refresh discovery"
        );
    }
    validate_test_unit(&manifest, target, test_unit)?;
    test_plan::explicit_unit_plan(graph, target, test_unit)
}

pub(crate) fn validate_test_unit(
    manifest: &TestManifest,
    target: &str,
    test_unit: &str,
) -> Result<()> {
    if manifest.case_filtering != "runner_args" {
        anyhow::bail!("target `{target}` does not support explicit test-unit filtering");
    }
    if !manifest.units.iter().any(|unit| unit.id == test_unit) {
        anyhow::bail!(
            "test unit `{test_unit}` is not present in the current manifest for `{target}`; run the whole target to refresh discovery"
        );
    }
    Ok(())
}

pub async fn test_results(workspace: &Path, output: Output, target_id: &str) -> Result<()> {
    let value = test_results_value(workspace, target_id)?;
    write_body(output, || render_test_results_human(&value), &value).await
}

pub async fn test_manifest(workspace: &Path, output: Output, target_id: &str) -> Result<()> {
    let manifest = test_manifest_record(workspace, target_id)?;
    write_body(output, || render_test_manifest_human(&manifest), &manifest).await
}

pub(crate) fn test_manifest_value(workspace: &Path, target_id: &str) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(test_manifest_record(
        workspace, target_id,
    )?)?)
}

pub(crate) fn test_manifest_record(workspace: &Path, target_id: &str) -> Result<TestManifest> {
    if let Some(manifest) = stored_test_manifest_record(workspace, target_id)? {
        return Ok(manifest);
    }
    // Reading a manifest must not mutate the workspace. Persistence happens only
    // through refresh_test_manifest, which runs after an actual whole-target test.
    derive_test_manifest(workspace, target_id)
}

pub(crate) fn stored_test_manifest_record(
    workspace: &Path,
    target_id: &str,
) -> Result<Option<TestManifest>> {
    let manifest_path = test_manifest_path(target_id)?;
    let stored_path = workspace.join(&manifest_path);
    if stored_path.is_file() {
        let raw = std::fs::read_to_string(&stored_path)
            .with_context(|| format!("reading `{}`", stored_path.display()))?;
        return serde_json::from_str(&raw)
            .with_context(|| format!("parsing `{}`", stored_path.display()))
            .map(Some);
    }
    Ok(None)
}

pub(crate) fn refresh_test_manifest(workspace: &Path, target_id: &str) -> Result<TestManifest> {
    let manifest = derive_test_manifest(workspace, target_id)?;
    write_test_manifest(workspace, &manifest)?;
    Ok(manifest)
}

fn write_test_manifest(workspace: &Path, manifest: &TestManifest) -> Result<()> {
    let path = workspace.join(test_manifest_path(&manifest.target)?);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating `{}`", parent.display()))?;
    }
    let raw = serde_json::to_vec_pretty(&manifest).context("serializing test manifest")?;
    std::fs::write(&path, raw).with_context(|| format!("writing `{}`", path.display()))?;
    Ok(())
}

fn derive_test_manifest(workspace: &Path, target_id: &str) -> Result<TestManifest> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    let target = graph
        .iter()
        .find(|target| target.label.id == target_id)
        .with_context(|| format!("no target matches `{target_id}`"))?;
    if !has_capability(target, "test") {
        anyhow::bail!("target `{target_id}` does not expose the test capability");
    }
    let provider = metadata_provider(workspace, target);
    let test_info = provider
        .as_ref()
        .and_then(|provider| provider.get("test_info"))
        .cloned();
    let runner = test_info
        .as_ref()
        .and_then(|info| info.pointer("/runner/type"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let listing_supported = test_info
        .as_ref()
        .and_then(|info| info.pointer("/listing/supported"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let case_filtering = test_info
        .as_ref()
        .and_then(|info| info.pointer("/filtering/case_filtering"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unsupported")
        .to_string();
    let sharding = once_core::TestSharding {
        supported: test_info
            .as_ref()
            .and_then(|info| info.pointer("/sharding/supported"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        granularity: test_info
            .as_ref()
            .and_then(|info| info.pointer("/sharding/granularity"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("target")
            .to_string(),
    };
    let path = workspace.join(test_results_path(target_id)?);
    let (source, units) = if path.is_file() {
        let results = test_results_value(workspace, target_id)?;
        (
            "normalized_results",
            test_units_from_results(target_id, &results)?,
        )
    } else {
        ("whole_target_fallback", Vec::new())
    };
    TestManifest::new(
        target_id,
        runner,
        source,
        listing_supported,
        case_filtering,
        sharding,
        units,
    )
    .and_then(|manifest| {
        manifest.with_discovery_fingerprint(
            provider
                .as_ref()
                .map(|provider| test_discovery_fingerprint(workspace, target, provider))
                .transpose()?,
        )
    })
}

pub(crate) fn test_manifest_is_current(
    workspace: &Path,
    target_id: &str,
    manifest: &TestManifest,
) -> bool {
    let Some(expected) = manifest.discovery_fingerprint.as_deref() else {
        return false;
    };
    let Ok(graph) = once_frontend::load_graph_workspace(workspace) else {
        return false;
    };
    let Some(target) = graph.iter().find(|target| target.label.id == target_id) else {
        return false;
    };
    let Some(provider) = metadata_provider(workspace, target) else {
        return false;
    };
    let Ok(actual) = test_discovery_fingerprint(workspace, target, &provider) else {
        return false;
    };
    let current = actual == expected;
    tracing::debug!(
        target = target_id,
        expected_fingerprint = expected,
        actual_fingerprint = actual,
        current,
        "checked test manifest freshness"
    );
    current
}

fn test_discovery_fingerprint(
    workspace: &Path,
    target: &once_frontend::GraphTarget,
    provider: &serde_json::Value,
) -> Result<String> {
    let mut digest = InputDigestBuilder::new(b"once.test-discovery.v1");
    digest.push_bytes(&serde_json::to_vec(target).context("serializing test target")?);
    digest.push_bytes(&serde_json::to_vec(provider).context("serializing test provider")?);
    let mut inputs = provider
        .get("test_discovery_inputs")
        .or_else(|| provider.get("affected_inputs"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    inputs.sort();
    inputs.dedup();
    for input in inputs {
        let path = WorkspacePath::try_from(input.as_str())
            .with_context(|| format!("invalid affected test input `{input}`"))?;
        digest.push_bytes(path.as_str().as_bytes());
        digest
            .push_source(workspace, path.as_str())
            .with_context(|| format!("hashing affected test input `{input}`"))?;
    }
    Ok(digest.finish().to_string())
}

fn test_manifest_path(target_id: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(".once")
        .join("test-manifests")
        .join(target_id_path(target_id)?)
        .join("manifest.json"))
}

fn test_units_from_results(target_id: &str, results: &serde_json::Value) -> Result<Vec<TestUnit>> {
    results
        .get("cases")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .map(|case| {
            let id = case
                .get("id")
                .and_then(serde_json::Value::as_str)
                .context("normalized test case is missing its id")?;
            let name = case
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(id);
            let suite = case
                .get("suite")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(target_id);
            Ok(TestUnit {
                id: id.to_string(),
                name: name.to_string(),
                suite: suite.to_string(),
                file: case
                    .get("file")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect()
}

pub async fn evidence(
    workspace: &Path,
    output: Output,
    subject: Option<&str>,
    limit: Option<usize>,
) -> Result<()> {
    let records = evidence_records(workspace, subject, limit).await?;
    write_body(output, || render_evidence_human(&records), &records).await
}

pub async fn test_attempts(
    workspace: &Path,
    output: Output,
    target: Option<&str>,
    limit: Option<usize>,
) -> Result<()> {
    let records = test_attempt_records(workspace, target, limit).await?;
    write_body(output, || render_test_attempts_human(&records), &records).await
}

pub(crate) async fn test_attempt_records(
    workspace: &Path,
    target: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<TestBatchAttempt>> {
    let store = TestTimingStore::open_workspace(workspace);
    let mut records = store.load().await?;
    if let Some(target) = target {
        records.retain(|record| record.target == target);
    }
    if let Some(limit) = limit {
        if limit == 0 {
            anyhow::bail!("test attempt limit must be greater than zero");
        }
        let remove = records.len().saturating_sub(limit);
        records.drain(..remove);
    }
    Ok(records)
}

pub(crate) async fn evidence_records(
    workspace: &Path,
    subject: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<EvidenceRecord>> {
    let store = EvidenceStore::open_workspace(workspace);
    let mut records = store.load().await?;
    if let Some(subject) = subject {
        records.retain(|record| record.subject.matches(subject));
    }
    if let Some(limit) = limit {
        if limit == 0 {
            anyhow::bail!("evidence limit must be greater than zero");
        }
        let remove = records.len().saturating_sub(limit);
        records.drain(..remove);
    }
    Ok(records)
}

pub(crate) fn test_results_value(workspace: &Path, target_id: &str) -> Result<serde_json::Value> {
    test_results_value_at(workspace, target_id, None, &[])
}

pub(crate) fn test_results_value_at(
    workspace: &Path,
    target_id: &str,
    result_path: Option<&str>,
    expected_units: &[String],
) -> Result<serde_json::Value> {
    let path = match result_path {
        Some(path) => once_core::WorkspacePath::try_from(path)
            .context("test result path must be workspace-relative")?
            .resolve(workspace),
        None => workspace.join(test_results_path(target_id)?),
    };
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading `{}`", path.display()))?;
    let value =
        serde_json::from_str(&raw).with_context(|| format!("parsing `{}`", path.display()))?;
    once_core::validate_test_results_for_units(&value, target_id, expected_units)
        .with_context(|| format!("validating `{}`", path.display()))?;
    Ok(value)
}

pub async fn validate_target(
    workspace: &Path,
    output: Output,
    file: Option<PathBuf>,
) -> Result<()> {
    let raw = read_json_input(file)?;
    let input: ValidateTargetInput = serde_json::from_str(&raw)
        .context("validate-target input is not valid JSON matching `{ \"target\": { ... } }`")?;
    let schemas = once_frontend::target_kind_schemas_for_workspace(workspace)?;
    let diagnostics = once_frontend::validate_target(&input.target, &schemas);
    let result = if diagnostics.is_empty() {
        ValidateResult::Valid { valid: true }
    } else {
        ValidateResult::Invalid {
            valid: false,
            diagnostics,
        }
    };
    write_body(output, || render_validate_human(&result), &result).await
}

#[derive(Debug, serde::Deserialize)]
struct ValidateTargetInput {
    target: once_frontend::TargetSpec,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ValidateResult {
    Valid {
        valid: bool,
    },
    Invalid {
        valid: bool,
        diagnostics: Vec<once_frontend::Diagnostic>,
    },
}

pub(crate) fn read_json_input(file: Option<PathBuf>) -> Result<String> {
    if let Some(path) = file {
        std::fs::read_to_string(&path).with_context(|| format!("reading `{}`", path.display()))
    } else {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("reading JSON from stdin")?;
        Ok(buf)
    }
}

fn render_capabilities_human(target: &once_frontend::GraphTarget) -> String {
    let mut out = format!("{} ({})\n", target.label.id, target.kind);
    if target.capabilities.is_empty() {
        out.push_str("capabilities: none\n");
        return out;
    }
    out.push_str("capabilities:\n");
    for capability in &target.capabilities {
        writeln!(out, "  {}", capability.name).expect("writing to string cannot fail");
        writeln!(out, "    outputs: {}", capability.output_groups.join(", "))
            .expect("writing to string cannot fail");
        if !capability.requires_outputs.is_empty() {
            writeln!(
                out,
                "    requires: {}",
                capability.requires_outputs.join(", ")
            )
            .expect("writing to string cannot fail");
        }
    }
    out
}

fn render_schema_human(schema: &once_frontend::TargetKindSchema) -> String {
    let mut out = format!("{}: {}\n", schema.kind, schema.docs);
    if !schema.attrs.is_empty() {
        out.push_str("attrs:\n");
        for attr in &schema.attrs {
            let required = if attr.required {
                "required"
            } else {
                "optional"
            };
            let configurable = if attr.configurable {
                ", configurable"
            } else {
                ""
            };
            writeln!(
                out,
                "  {}: {} ({required}{configurable})",
                attr.name, attr.ty
            )
            .expect("writing to string cannot fail");
        }
    }
    if !schema.capabilities.is_empty() {
        out.push_str("capabilities:\n");
        for capability in &schema.capabilities {
            let requires = if capability.requires_outputs.is_empty() {
                String::new()
            } else {
                format!(" (requires: {})", capability.requires_outputs.join(", "))
            };
            writeln!(
                out,
                "  {}: {}{}",
                capability.name,
                capability.output_groups.join(", "),
                requires
            )
            .expect("writing to string cannot fail");
        }
    }
    if !schema.tools.is_empty() {
        out.push_str("tools:\n");
        for tool in &schema.tools {
            writeln!(out, "  {}: {}", tool.name, tool.executables.join(", "))
                .expect("writing to string cannot fail");
        }
    }
    if !schema.source_references.is_empty() {
        out.push_str("source references:\n");
        for reference in &schema.source_references {
            writeln!(
                out,
                "  {} {}: {}",
                reference.system, reference.symbol, reference.url
            )
            .expect("writing to string cannot fail");
        }
    }
    out
}

fn render_target_kinds_human(target_kinds: &[TargetKindSummary]) -> String {
    if target_kinds.is_empty() {
        return "target kinds: none\n".to_string();
    }
    let mut out = String::from("target kinds:\n");
    for kind in target_kinds {
        writeln!(out, "  {}: {}", kind.kind, kind.docs).expect("writing to string cannot fail");
        for example in &kind.examples {
            writeln!(out, "    {} - {}", example.slug, example.use_when)
                .expect("writing to string cannot fail");
        }
    }
    out
}

fn render_example_human(example: &once_frontend::TargetKindExampleBundle) -> String {
    let mut out = format!(
        "example {}: {}\nuse when: {}\nfiles:\n",
        example.slug, example.name, example.use_when
    );
    for file in &example.files {
        writeln!(out, "  {}", file.path).expect("writing to string cannot fail");
    }
    out
}

fn render_evidence_human(records: &[EvidenceRecord]) -> String {
    if records.is_empty() {
        return "evidence: none\n".to_string();
    }
    let mut out = String::from("evidence:\n");
    for record in records {
        let _ = writeln!(
            out,
            "  {} {} {} cache={} exit={} action={}",
            record.subject.display(),
            record.kind,
            evidence_status(record),
            evidence_cache(record),
            record.exit_code,
            record.action_digest
        );
    }
    out
}

fn render_test_attempts_human(records: &[TestBatchAttempt]) -> String {
    if records.is_empty() {
        return "test attempts: none\n".to_string();
    }
    let mut out = String::from("test attempts:\n");
    for record in records {
        let _ = writeln!(
            out,
            "  {} {:?} {} ms worker={} cache={}",
            record.target,
            record.status,
            record.duration_ms,
            record.worker,
            record.cache.as_deref().unwrap_or("unknown")
        );
    }
    out
}

fn evidence_status(record: &EvidenceRecord) -> &'static str {
    match record.status {
        once_core::EvidenceStatus::Passed => "passed",
        once_core::EvidenceStatus::Failed => "failed",
    }
}

fn evidence_cache(record: &EvidenceRecord) -> &'static str {
    record.cache.as_str()
}

fn test_records(workspace: &Path, graph: &[once_frontend::GraphTarget]) -> Vec<TestTargetRecord> {
    graph
        .iter()
        .filter(|target| has_capability(target, "test"))
        .map(|target| {
            let test_info = metadata_test_info(workspace, target);
            TestTargetRecord {
                id: target.label.id.clone(),
                kind: target.kind.clone(),
                deps: target.deps.clone(),
                runner: test_info
                    .as_ref()
                    .and_then(|info| info.pointer("/runner/type"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                labels: test_info
                    .as_ref()
                    .and_then(|info| info.get("labels"))
                    .and_then(serde_json::Value::as_array)
                    .map(|labels| {
                        labels
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                results_path: test_info
                    .as_ref()
                    .and_then(|info| info.pointer("/outputs/results"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .or_else(|| test_results_path_string(&target.label.id).ok()),
            }
        })
        .collect()
}

fn metadata_test_info(
    workspace: &Path,
    target: &once_frontend::GraphTarget,
) -> Option<serde_json::Value> {
    if !target
        .providers
        .iter()
        .any(|provider| provider == "once_test_info")
    {
        return None;
    }
    metadata_provider(workspace, target)?
        .get("test_info")
        .cloned()
}

fn metadata_provider(
    workspace: &Path,
    target: &once_frontend::GraphTarget,
) -> Option<serde_json::Value> {
    let analyzer = once_frontend::analysis::AnalysisEngine::for_workspace(workspace).ok()?;
    let analysis = analyzer
        .analyze_target_capability(target, workspace, &[], "metadata")
        .ok()?;
    Some(analysis.provider)
}

fn test_results_path(target_id: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(".once")
        .join("out")
        .join(target_id_path(target_id)?)
        .join("test")
        .join("test_results.json"))
}

fn test_results_path_string(target_id: &str) -> Result<String> {
    Ok(test_results_path(target_id)?
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/"))
}

pub(crate) fn target_id_path(target_id: &str) -> Result<PathBuf> {
    let mut path = PathBuf::new();
    for segment in target_id.split('/') {
        if !is_safe_target_id_segment(segment) {
            return Err(anyhow!("invalid target id `{target_id}`"));
        }
        path.push(segment);
    }
    if path.as_os_str().is_empty() {
        return Err(anyhow!("invalid target id `{target_id}`"));
    }
    Ok(path)
}

fn is_safe_target_id_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

fn has_capability(target: &once_frontend::GraphTarget, name: &str) -> bool {
    target
        .capabilities
        .iter()
        .any(|capability| capability.name == name)
}

fn render_tests_human(records: &[TestTargetRecord]) -> String {
    if records.is_empty() {
        return "tests: none\n".to_string();
    }
    let mut out = String::from("tests:\n");
    for record in records {
        let runner = record.runner.as_deref().unwrap_or("unknown");
        writeln!(out, "  {} ({}) runner={runner}", record.id, record.kind)
            .expect("writing to string cannot fail");
        if let Some(results_path) = &record.results_path {
            writeln!(out, "    results: {results_path}").expect("writing to string cannot fail");
        }
        if !record.labels.is_empty() {
            writeln!(out, "    labels: {}", record.labels.join(", "))
                .expect("writing to string cannot fail");
        }
    }
    out
}

fn render_affected_tests_human(records: &[test_plan::AffectedTestRecord]) -> String {
    if records.is_empty() {
        return "affected tests: none\n".to_string();
    }
    let mut out = String::from("affected tests:\n");
    for record in records {
        writeln!(out, "  {} ({})", record.id, record.kind).expect("writing to string cannot fail");
        for reason in &record.reasons {
            writeln!(out, "    - {reason}").expect("writing to string cannot fail");
        }
    }
    out
}

fn render_test_plan_human(plan: &test_plan::TestPlan) -> String {
    let mut out = format!("test plan: {}\n", plan.id);
    if plan.batches.is_empty() {
        out.push_str("  batches: none\n");
    } else {
        for batch in &plan.batches {
            writeln!(out, "  {}", batch.target).expect("writing to string cannot fail");
        }
    }
    if !plan.selection.unmatched_paths.is_empty() {
        out.push_str("unmatched changed paths:\n");
        for path in &plan.selection.unmatched_paths {
            writeln!(out, "  {path}").expect("writing to string cannot fail");
        }
    }
    out
}

fn render_test_results_human(value: &serde_json::Value) -> String {
    let target = value
        .get("target")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<unknown>");
    let status = value
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let mut out = format!("test results: {target} {status}\n");
    if let Some(summary) = value.get("summary") {
        writeln!(out, "summary: {summary}").expect("writing to string cannot fail");
    }
    if let Some(cases) = value.get("cases").and_then(serde_json::Value::as_array) {
        for case in cases {
            let id = case
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<unknown>");
            let status = case
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            writeln!(out, "  {status} {id}").expect("writing to string cannot fail");
        }
    }
    out
}

fn render_test_manifest_human(manifest: &TestManifest) -> String {
    let mut out = format!(
        "test manifest: {} source={} filtering={}\n",
        manifest.target, manifest.source, manifest.case_filtering
    );
    if manifest.units.is_empty() {
        out.push_str("  units: none; run the whole target to refresh discovery\n");
        return out;
    }
    for unit in &manifest.units {
        writeln!(out, "  {} {}", unit.id, unit.name).expect("writing to string cannot fail");
    }
    out
}

fn render_target_human(target: &once_frontend::GraphTarget) -> String {
    let mut out = format!("{} ({})\n", target.label.id, target.kind);
    if !target.srcs.is_empty() {
        writeln!(out, "srcs: {}", target.srcs.join(", ")).expect("writing to string cannot fail");
    }
    if !target.deps.is_empty() {
        writeln!(out, "deps: {}", target.deps.join(", ")).expect("writing to string cannot fail");
    }
    for (role, dependencies) in &target.dependency_edges {
        writeln!(out, "dependencies.{role}: {}", dependencies.join(", "))
            .expect("writing to string cannot fail");
    }
    if !target.attrs.is_empty() {
        out.push_str("attrs:\n");
        for (key, value) in &target.attrs {
            writeln!(out, "  {key} = {value:?}").expect("writing to string cannot fail");
        }
    }
    if !target.capabilities.is_empty() {
        out.push_str("capabilities: ");
        let names: Vec<&str> = target
            .capabilities
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        out.push_str(&names.join(", "));
        out.push('\n');
    }
    if !target.tools.is_empty() {
        out.push_str("tools:\n");
        for tool in &target.tools {
            writeln!(out, "  {}: {}", tool.name, tool.executables.join(", "))
                .expect("writing to string cannot fail");
        }
    }
    out
}

fn render_validate_human(result: &ValidateResult) -> String {
    match result {
        ValidateResult::Valid { .. } => "valid\n".to_string(),
        ValidateResult::Invalid { diagnostics, .. } => {
            let mut out = String::from("invalid:\n");
            for diagnostic in diagnostics {
                let scope = match (&diagnostic.target, &diagnostic.attribute) {
                    (Some(t), Some(a)) => format!(" [{t}/{a}]"),
                    (Some(t), None) => format!(" [{t}]"),
                    (None, Some(a)) => format!(" [{a}]"),
                    (None, None) => String::new(),
                };
                writeln!(
                    out,
                    "  {} ({}){}: {}",
                    diagnostic.code,
                    scope.trim(),
                    scope,
                    diagnostic.message
                )
                .expect("writing to string cannot fail");
                for repair in &diagnostic.repairs {
                    writeln!(out, "    - {repair}").expect("writing to string cannot fail");
                }
            }
            out
        }
    }
}

fn render_targets_human(records: &[TargetRecord]) -> String {
    if records.is_empty() {
        return "targets: none\n".to_string();
    }
    let mut out = String::from("targets:\n");
    for target in records {
        let capabilities = if target.capabilities.is_empty() {
            "none".to_string()
        } else {
            target.capabilities.join(", ")
        };
        writeln!(out, "  {} ({}) [{}]", target.id, target.kind, capabilities)
            .expect("writing to string cannot fail");
        for tool in &target.tools {
            writeln!(
                out,
                "    tool {}: {}",
                tool.name,
                tool.executables.join(", ")
            )
            .expect("writing to string cannot fail");
        }
    }
    out
}

async fn write_body<T: Serialize>(
    output: Output,
    human: impl FnOnce() -> String,
    value: &T,
) -> Result<()> {
    let body = match output.format {
        Format::Human => human(),
        Format::Json | Format::Toon => render::structured(output.format, value)?,
    };
    let mut out = tokio::io::stdout();
    out.write_all(body.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use once_frontend::{Capability, GraphTarget, TargetLabel};

    use super::*;

    fn target(id: &str, kind: &str, capabilities: &[&str]) -> GraphTarget {
        let (package, name) = id.rsplit_once('/').unwrap_or(("", id));
        GraphTarget {
            label: TargetLabel {
                package: package.to_string(),
                name: name.to_string(),
                id: id.to_string(),
            },
            kind: kind.to_string(),
            deps: Vec::new(),
            dependency_edges: std::collections::BTreeMap::new(),
            srcs: Vec::new(),
            attrs: BTreeMap::new(),
            capabilities: capabilities
                .iter()
                .map(|name| Capability {
                    name: (*name).to_string(),
                    output_groups: Vec::new(),
                    requires_outputs: Vec::new(),
                })
                .collect(),
            providers: Vec::new(),
            tools: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn render_targets_human_reports_empty_and_populated() {
        assert_eq!(render_targets_human(&[]), "targets: none\n");
        let rendered = render_targets_human(&[TargetRecord {
            id: "apps/ios/App".to_string(),
            package: "apps/ios".to_string(),
            name: "App".to_string(),
            kind: "apple_application".to_string(),
            deps: Vec::new(),
            dependency_edges: std::collections::BTreeMap::new(),
            capabilities: vec!["build".to_string(), "run".to_string()],
            tools: Vec::new(),
        }]);
        assert!(rendered.contains("apps/ios/App (apple_application) [build, run]"));
    }

    #[test]
    fn render_capabilities_human_lists_outputs_and_requires() {
        let mut target = target("apps/ios/App", "apple_application", &["build", "run"]);
        target.capabilities[1].output_groups = vec!["default".to_string()];
        target.capabilities[1].requires_outputs = vec!["bundle".to_string()];

        let rendered = render_capabilities_human(&target);

        assert!(rendered.contains("apps/ios/App (apple_application)"));
        assert!(rendered.contains("  run\n    outputs: default\n    requires: bundle"));
    }

    #[test]
    fn render_capabilities_human_reports_none() {
        let target = target("apps/ios/App", "mystery", &[]);
        assert!(render_capabilities_human(&target).contains("capabilities: none"));
    }

    #[test]
    fn render_schema_human_includes_attrs_and_capabilities() {
        let schema = once_frontend::built_in_target_kind_schema("apple_application").unwrap();
        let rendered = render_schema_human(&schema);
        assert!(rendered.starts_with("apple_application: "));
        assert!(rendered.contains("bundle_id: string (required"));
        assert!(rendered.contains("build: default, bundle, dsyms"));
        assert!(rendered.contains("run: default (requires: bundle)"));
    }

    #[test]
    fn render_schema_human_includes_tools() {
        let schema = once_frontend::built_in_target_kind_schema("rust_binary").unwrap();

        let rendered = render_schema_human(&schema);

        assert!(rendered.contains("tools:\n  rust: rustc, cargo"));
    }

    #[test]
    fn target_records_filters_by_kind() {
        let records = target_records(
            vec![
                target("apps/ios/App", "apple_application", &["build", "run"]),
                target("apps/ios/AppTests", "apple_test_bundle", &["build"]),
            ],
            Some("apple_application"),
        );

        assert_eq!(
            records,
            vec![TargetRecord {
                id: "apps/ios/App".to_string(),
                package: "apps/ios".to_string(),
                name: "App".to_string(),
                kind: "apple_application".to_string(),
                deps: Vec::new(),
                dependency_edges: std::collections::BTreeMap::new(),
                capabilities: vec!["build".to_string(), "run".to_string()],
                tools: Vec::new(),
            }]
        );
    }

    #[test]
    fn target_id_path_accepts_only_path_safe_segments() {
        assert_eq!(
            target_id_path("apps/ios/AppTests").unwrap(),
            PathBuf::from("apps").join("ios").join("AppTests")
        );

        for target_id in [
            "",
            ".",
            "..",
            "apps/../Secret",
            "apps//Tests",
            "apps\\Tests",
            "apps/ios/App Tests",
            "apps/ios/App;rm",
        ] {
            assert!(
                target_id_path(target_id).is_err(),
                "{target_id} should fail"
            );
        }
    }

    #[test]
    fn explicit_test_unit_validation_rejects_unsupported_filtering() {
        let manifest = TestManifest::new(
            "suite",
            Some("example".to_string()),
            "normalized_results",
            true,
            "unsupported",
            once_core::TestSharding::default(),
            vec![TestUnit {
                id: "suite::case".to_string(),
                name: "case".to_string(),
                suite: "suite".to_string(),
                file: None,
            }],
        )
        .unwrap();

        let error = validate_test_unit(&manifest, "suite", "suite::case").unwrap_err();

        assert_eq!(
            error.to_string(),
            "target `suite` does not support explicit test-unit filtering"
        );
    }

    #[test]
    fn explicit_test_unit_validation_requires_a_current_manifest_unit() {
        let manifest = TestManifest::new(
            "suite",
            Some("example".to_string()),
            "normalized_results",
            true,
            "runner_args",
            once_core::TestSharding::default(),
            vec![],
        )
        .unwrap();

        let error = validate_test_unit(&manifest, "suite", "suite::missing").unwrap_err();

        assert!(error
            .to_string()
            .contains("run the whole target to refresh discovery"));
    }

    #[test]
    fn discovery_fingerprint_changes_with_declared_discovery_input_content() {
        let workspace = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(workspace.path().join("tests")).unwrap();
        std::fs::write(workspace.path().join("tests/example.py"), "first").unwrap();
        let target = once_frontend::GraphTarget {
            label: once_frontend::TargetLabel {
                package: String::new(),
                name: "tests".to_string(),
                id: "tests".to_string(),
            },
            kind: "example_test".to_string(),
            deps: Vec::new(),
            dependency_edges: std::collections::BTreeMap::new(),
            srcs: vec!["tests/*.py".to_string()],
            attrs: std::collections::BTreeMap::new(),
            capabilities: Vec::new(),
            providers: Vec::new(),
            tools: Vec::new(),
            diagnostics: Vec::new(),
        };
        let provider = serde_json::json!({
            "test_discovery_inputs": ["tests/example.py"],
            "affected_inputs": ["tests/example.py"]
        });

        let first = test_discovery_fingerprint(workspace.path(), &target, &provider).unwrap();
        std::fs::write(workspace.path().join("tests/example.py"), "second").unwrap();
        let second = test_discovery_fingerprint(workspace.path(), &target, &provider).unwrap();

        assert_ne!(first, second);
    }
}
