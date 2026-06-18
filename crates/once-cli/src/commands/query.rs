//! `once query` - inspect the typed build graph.

mod expression;

use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
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
    capabilities: Vec<String>,
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
struct AffectedTestRecord {
    id: String,
    kind: String,
    reasons: Vec<String>,
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
            capabilities: target
                .capabilities
                .into_iter()
                .map(|capability| capability.name)
                .collect(),
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

pub async fn target_kinds(workspace: &Path, output: Output) -> Result<()> {
    let schemas = once_frontend::target_kind_schemas_for_workspace(workspace)?;
    let summaries: Vec<TargetKindSummary> =
        schemas.into_iter().map(TargetKindSummary::from).collect();
    write_body(output, || render_target_kinds_human(&summaries), &summaries).await
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
    let records = affected_test_records(workspace, &graph, changed_paths);
    write_body(output, || render_affected_tests_human(&records), &records).await
}

pub(crate) fn affected_tests_value(
    workspace: &Path,
    changed_paths: &[String],
) -> Result<serde_json::Value> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    Ok(serde_json::to_value(affected_test_records(
        workspace,
        &graph,
        changed_paths,
    ))?)
}

pub async fn test_results(workspace: &Path, output: Output, target_id: &str) -> Result<()> {
    let path = workspace.join(test_results_path(target_id)?);
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading `{}`", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).with_context(|| format!("parsing `{}`", path.display()))?;
    write_body(output, || render_test_results_human(&value), &value).await
}

pub(crate) fn test_results_value(workspace: &Path, target_id: &str) -> Result<serde_json::Value> {
    let path = workspace.join(test_results_path(target_id)?);
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading `{}`", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parsing `{}`", path.display()))
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

fn affected_test_records(
    workspace: &Path,
    graph: &[once_frontend::GraphTarget],
    changed_paths: &[String],
) -> Vec<AffectedTestRecord> {
    let changed: Vec<String> = changed_paths
        .iter()
        .map(|path| path.trim_start_matches("./").to_string())
        .collect();
    let targets = graph
        .iter()
        .map(|target| (target.label.id.as_str(), target))
        .collect::<std::collections::BTreeMap<_, _>>();
    graph
        .iter()
        .filter(|target| has_capability(target, "test"))
        .filter_map(|target| {
            let mut reasons = Vec::new();
            if changed.is_empty() {
                reasons.push("no changed paths supplied; include test target".to_string());
            }
            for path in &changed {
                if target_owns_path(workspace, target, path) {
                    reasons.push(format!("changed test input `{path}`"));
                }
                for dep_id in &target.deps {
                    if let Some(owner) = dependency_owning_path(
                        workspace,
                        &targets,
                        dep_id,
                        path,
                        &mut std::collections::BTreeSet::new(),
                    ) {
                        reasons.push(format!("changed dependency `{owner}` input `{path}`"));
                    }
                }
            }
            reasons.sort();
            reasons.dedup();
            (!reasons.is_empty()).then(|| AffectedTestRecord {
                id: target.label.id.clone(),
                kind: target.kind.clone(),
                reasons,
            })
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

fn target_owns_path(workspace: &Path, target: &once_frontend::GraphTarget, changed: &str) -> bool {
    expanded_target_inputs(workspace, target)
        .iter()
        .any(|input| input == changed)
}

fn dependency_owning_path(
    workspace: &Path,
    targets: &std::collections::BTreeMap<&str, &once_frontend::GraphTarget>,
    target_id: &str,
    changed: &str,
    visited: &mut std::collections::BTreeSet<String>,
) -> Option<String> {
    if !visited.insert(target_id.to_string()) {
        return None;
    }
    let target = targets.get(target_id)?;
    if target_owns_path(workspace, target, changed) {
        return Some(target.label.id.clone());
    }
    for dep_id in &target.deps {
        if let Some(owner) = dependency_owning_path(workspace, targets, dep_id, changed, visited) {
            return Some(owner);
        }
    }
    None
}

fn expanded_target_inputs(workspace: &Path, target: &once_frontend::GraphTarget) -> Vec<String> {
    let mut inputs = Vec::new();
    let package_dir = if target.label.package.is_empty() {
        workspace.to_path_buf()
    } else {
        workspace.join(&target.label.package)
    };
    for pattern in &target.srcs {
        let abs_pattern = package_dir.join(pattern);
        let Some(pattern) = abs_pattern.to_str() else {
            continue;
        };
        let Ok(entries) = glob::glob(pattern) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.is_file() {
                continue;
            }
            if let Ok(relative) = entry.strip_prefix(workspace) {
                inputs.push(workspace_relative_path_string(relative));
            }
        }
    }
    if let Some(provider) = metadata_provider(workspace, target) {
        inputs.extend(provider_string_list(&provider, "affected_inputs"));
    }
    inputs.sort();
    inputs.dedup();
    inputs
}

fn workspace_relative_path_string(path: &Path) -> String {
    let path = path.to_string_lossy();
    if std::path::MAIN_SEPARATOR == '/' {
        path.into_owned()
    } else {
        path.replace(std::path::MAIN_SEPARATOR, "/")
    }
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

fn provider_string_list(provider: &serde_json::Value, key: &str) -> Vec<String> {
    provider
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
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

fn render_affected_tests_human(records: &[AffectedTestRecord]) -> String {
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

fn render_target_human(target: &once_frontend::GraphTarget) -> String {
    let mut out = format!("{} ({})\n", target.label.id, target.kind);
    if !target.srcs.is_empty() {
        writeln!(out, "srcs: {}", target.srcs.join(", ")).expect("writing to string cannot fail");
    }
    if !target.deps.is_empty() {
        writeln!(out, "deps: {}", target.deps.join(", ")).expect("writing to string cannot fail");
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
            capabilities: vec!["build".to_string(), "run".to_string()],
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
                capabilities: vec!["build".to_string(), "run".to_string()],
            }]
        );
    }

    #[test]
    fn workspace_relative_path_string_preserves_slash_separated_paths() {
        assert_eq!(
            workspace_relative_path_string(Path::new("apps/ios/App.swift")),
            "apps/ios/App.swift"
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
}
