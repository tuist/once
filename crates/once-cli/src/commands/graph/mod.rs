//! Graph capability commands for build, lint, run, and test.
//!
//! This module owns command orchestration: resolving a target from the
//! workspace graph, checking the requested capability, executing actions
//! declared by target kinds or generic fallback actions, and rendering the result. The legacy
//! capability fallback lives in [`action`].

mod action;
mod analysis;
mod build_receipt;
mod contract;

pub use contract::{validate_action_contracts, ActionContractValidation};

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use once_cas::{ActionResult, CacheProvider, Digest};
use once_core::{
    EvidenceCacheState, EvidenceSubject, LintResults, LintSeverity, ResourceLimits, RunOpts,
    SandboxMode, WorkspacePath,
};
use once_frontend::analysis::AnalysisOptions;
use once_frontend::GraphTarget;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::cli::{Format, Output};
use crate::commands::util::cache_tag;
use crate::render;

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CapabilityRunRecord {
    target: String,
    kind: String,
    capability: String,
    status: String,
    action_digest: String,
    cache: String,
    output_groups: Vec<String>,
    required_outputs: Vec<String>,
    outputs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    test_results: Option<String>,
    #[serde(skip, default)]
    input_digest: Option<Digest>,
    #[serde(skip, default = "default_cache_state")]
    cache_state: EvidenceCacheState,
    #[serde(skip, default = "default_action_result")]
    result: ActionResult,
}

fn default_cache_state() -> EvidenceCacheState {
    EvidenceCacheState::Hit
}

fn default_action_result() -> ActionResult {
    ActionResult {
        exit_code: 0,
        stdout: None,
        stderr: None,
        outputs: BTreeMap::new(),
    }
}

#[derive(Debug, Clone, Default)]
pub struct GraphRunOptions {
    pub visible: bool,
    pub arguments: Vec<String>,
}

pub async fn build(
    workspace: &Path,
    cache: &CacheProvider,
    output: Output,
    target_id: &str,
    sandbox: SandboxMode,
    resource_limits: ResourceLimits,
) -> Result<ExitCode> {
    let xdg = once_core::Xdg::from_env();
    let stored_receipt = build_receipt::read(workspace, target_id, sandbox).await;
    let prior_position = stored_receipt.as_ref().map(build_receipt::position);
    let initial_snapshot =
        crate::commands::change_tracker::snapshot(workspace, &xdg, &[], prior_position).await;
    if let Some(record) = build_receipt::load(
        workspace,
        target_id,
        sandbox,
        initial_snapshot.as_ref(),
        stored_receipt,
    )
    .await
    {
        write_record(output, &record).await?;
        return Ok(ExitCode::SUCCESS);
    }
    let session = analysis::BuildSession::load_workspace(workspace, cache, sandbox)
        .await?
        .with_resource_limits(resource_limits);
    let target = session.target(target_id)?;
    let record = build_target(workspace, cache, target, &session, sandbox).await?;
    record_capability_run(workspace, &record).await;
    // An uncacheable build (any declared action with `cacheable = false`, which
    // several target kinds such as Dockerfile, Go, and Android use) must run on
    // every invocation. Never persist a receipt for it, and drop any stale one,
    // so the fast path can never skip its mandatory work.
    if record.cache_state == EvidenceCacheState::Bypass {
        build_receipt::clear(workspace, target_id, sandbox).await;
        write_record(output, &record).await?;
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(final_snapshot) = crate::commands::change_tracker::snapshot(
        workspace,
        &xdg,
        &record.outputs,
        initial_snapshot.as_ref().map(|snapshot| &snapshot.position),
    )
    .await
    {
        if initial_snapshot.as_ref().is_some_and(|initial| {
            initial.position.instance_id == final_snapshot.position.instance_id
                && (initial.position.source_generation == final_snapshot.position.source_generation
                    || session.source_changes_match(final_snapshot.source_changes.as_deref()))
        }) {
            let environment = session.observed_environment();
            let host_paths = session.observed_host_paths();
            let source_digests =
                session.observed_source_digests(final_snapshot.source_changes.as_deref());
            build_receipt::store(
                workspace,
                target_id,
                sandbox,
                &final_snapshot,
                build_receipt::Observations {
                    environment: &environment,
                    host_paths: &host_paths,
                    source_digests: &source_digests,
                },
                &record,
            )
            .await;
        }
    }
    write_record(output, &record).await?;
    Ok(ExitCode::SUCCESS)
}

pub async fn lint(
    workspace: &Path,
    cache: &CacheProvider,
    output: Output,
    target_id: &str,
    sandbox: SandboxMode,
    fail_on: LintSeverity,
    resource_limits: ResourceLimits,
) -> Result<ExitCode> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    let session = analysis::BuildSession::new(workspace, cache, graph, sandbox)
        .await?
        .with_resource_limits(resource_limits);
    let target = session.target(target_id)?;
    let capability = ensure_capability(target, "lint")?;
    let outcome = session
        .run_with_analysis_and_provider_validation(target, "lint", validate_lint_provider)
        .await?
        .ok_or_else(|| anyhow!("{} does not implement its lint capability", target.label.id))?;
    let report_path = lint_provider_output_path(target, &outcome.provider, "sarif")?;
    let results_path = lint_provider_output_path(target, &outcome.provider, "results")?;
    let results = once_core::read_sarif_results(target.label.id.as_str(), report_path, workspace)?;
    persist_lint_results(workspace, results_path, &results).await?;
    let record = CapabilityRunRecord {
        target: target.label.id.clone(),
        kind: target.kind.clone(),
        capability: capability.name.clone(),
        status: "completed".to_string(),
        action_digest: outcome.action_digest.to_string(),
        cache: outcome.cache_tag.to_string(),
        output_groups: capability.output_groups.clone(),
        required_outputs: capability.requires_outputs.clone(),
        outputs: outcome.outputs,
        test_results: None,
        input_digest: outcome.input_digest,
        cache_state: outcome.cache_state,
        result: outcome.result,
    };
    record_capability_run(workspace, &record).await;
    write_lint_results(output, &results).await?;
    Ok(if results.fails_at(fail_on) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

fn validate_lint_provider(target: &GraphTarget, provider: &serde_json::Value) -> Result<()> {
    lint_provider_output_path(target, provider, "sarif")?;
    lint_provider_output_path(target, provider, "results")?;
    Ok(())
}

fn lint_provider_output_path<'a>(
    target: &GraphTarget,
    provider: &'a serde_json::Value,
    output_name: &str,
) -> Result<&'a str> {
    let attribute = format!("lint_info.outputs.{output_name}");
    let pointer = format!("/lint_info/outputs/{output_name}");
    let path = provider
        .pointer(&pointer)
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.is_empty());
    if let Some(path) = path {
        if WorkspacePath::try_from(path).is_ok() {
            return Ok(path);
        }
    }
    let diagnostic = once_frontend::Diagnostic::new(
        "invalid_lint_provider_output",
        format!(
            "lint provider for `{}` must return a non-empty workspace-relative path at `{attribute}`",
            target.label.id
        ),
    )
    .with_target(&target.label.id)
    .with_attribute(&attribute)
    .with_repair(format!(
        "Return the declared {output_name} output path at `{attribute}`"
    ));
    Err(anyhow::Error::new(
        once_frontend::analysis::AnalysisFailure { diagnostic },
    ))
}

async fn persist_lint_results(workspace: &Path, path: &str, results: &LintResults) -> Result<()> {
    let absolute = workspace.join(path);
    if let Some(parent) = absolute.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let bytes = serde_json::to_vec_pretty(results)?;
    tokio::fs::write(&absolute, bytes)
        .await
        .with_context(|| format!("writing normalized lint results `{}`", absolute.display()))
}

async fn write_lint_results(output: Output, results: &LintResults) -> Result<()> {
    let body = match output.format {
        Format::Human => render_lint_results(results),
        Format::Json | Format::Toon => render::structured(output.format, results)?,
    };
    let mut out = tokio::io::stdout();
    out.write_all(body.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}

fn render_lint_results(results: &LintResults) -> String {
    let mut out = format!(
        "once: lint {} complete, {} errors, {} warnings, {} notes\n",
        results.target, results.summary.errors, results.summary.warnings, results.summary.notes
    );
    for finding in &results.findings {
        if let Some(location) = &finding.location {
            if let Some(path) = &location.path {
                out.push_str(path);
                if let Some(line) = location.line {
                    out.push(':');
                    out.push_str(&line.to_string());
                    if let Some(column) = location.column {
                        out.push(':');
                        out.push_str(&column.to_string());
                    }
                }
                out.push_str(": ");
            }
        }
        out.push_str(&format!("{:?}", finding.severity).to_lowercase());
        if let Some(rule_id) = &finding.rule_id {
            out.push('[');
            out.push_str(rule_id);
            out.push(']');
        }
        out.push_str(": ");
        out.push_str(&finding.message);
        out.push('\n');
    }
    out
}

pub async fn test(
    workspace: &Path,
    cache: &CacheProvider,
    output: Output,
    target_id: &str,
    sandbox: SandboxMode,
    resource_limits: ResourceLimits,
) -> Result<ExitCode> {
    test_with_filters(
        workspace,
        cache,
        output,
        target_id,
        sandbox,
        &[],
        None,
        resource_limits,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn test_with_filters(
    workspace: &Path,
    cache: &CacheProvider,
    output: Output,
    target_id: &str,
    sandbox: SandboxMode,
    test_filters: &[String],
    test_batch_id: Option<&str>,
    resource_limits: ResourceLimits,
) -> Result<ExitCode> {
    if let Some(batch_id) = test_batch_id {
        if Digest::from_hex(batch_id).is_none() {
            anyhow::bail!("invalid internal test batch identifier");
        }
    }
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    if !test_filters.is_empty() {
        let manifest =
            crate::commands::query::test_manifest_record_with_graph(workspace, target_id, &graph)?;
        for test_filter in test_filters {
            crate::commands::query::validate_test_unit(&manifest, target_id, test_filter)?;
        }
    }
    let session = analysis::BuildSession::new_with_options(
        workspace,
        cache,
        graph,
        AnalysisOptions {
            test_filters: test_filters.to_vec(),
            test_batch_id: test_batch_id.map(str::to_string),
            ..AnalysisOptions::default()
        },
        sandbox,
    )
    .await?
    .with_resource_limits(resource_limits);
    let target = session.target(target_id)?;
    let test_capability = ensure_capability(target, "test")?;
    if !test_capability.requires_outputs.is_empty()
        && target
            .capabilities
            .iter()
            .any(|capability| capability.name == "build")
    {
        let build_record = build_target(workspace, cache, target, &session, sandbox).await?;
        record_capability_run(workspace, &build_record).await;
    }
    let record = if let Some(outcome) = session.run_with_analysis(target, "test").await? {
        let analysis::BuildOutcome {
            provider,
            action_digest,
            input_digest,
            outputs,
            cache_tag,
            cache_state,
            result,
            ..
        } = outcome;
        let test_results = provider
            .pointer("/test_info/outputs/results")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        CapabilityRunRecord {
            target: target.label.id.clone(),
            kind: target.kind.clone(),
            capability: test_capability.name.clone(),
            status: "completed".to_string(),
            action_digest: action_digest.to_string(),
            cache: cache_tag.to_string(),
            output_groups: test_capability.output_groups.clone(),
            required_outputs: test_capability.requires_outputs.clone(),
            outputs,
            test_results,
            input_digest,
            cache_state,
            result,
        }
    } else {
        run_target_capability(workspace, cache, target, "test", sandbox).await?
    };
    record_capability_run(workspace, &record).await;
    if test_filters.is_empty() {
        crate::commands::query::refresh_test_manifest_for_target(workspace, target)
            .context("persisting test manifest")?;
    }
    write_record(output, &record).await?;
    Ok(ExitCode::SUCCESS)
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    workspace: &Path,
    cache: &CacheProvider,
    graph: Vec<GraphTarget>,
    output: Output,
    target_id: &str,
    options: GraphRunOptions,
    sandbox: SandboxMode,
    resource_limits: ResourceLimits,
) -> Result<ExitCode> {
    let session = analysis::BuildSession::new_with_options(
        workspace,
        cache,
        graph,
        AnalysisOptions {
            run_visible: options.visible,
            run_arguments: options.arguments,
            ..AnalysisOptions::default()
        },
        sandbox,
    )
    .await?
    .with_resource_limits(resource_limits);
    let target = session.target(target_id)?;
    let run_capability = ensure_capability(target, "run")?;
    if !run_capability.requires_outputs.is_empty()
        && target
            .capabilities
            .iter()
            .any(|capability| capability.name == "build")
    {
        let build_record = build_target(workspace, cache, target, &session, sandbox).await?;
        record_capability_run(workspace, &build_record).await;
    }
    let record = if let Some(outcome) = session.run_with_analysis(target, "run").await? {
        let analysis::BuildOutcome {
            action_digest,
            input_digest,
            outputs,
            cache_tag,
            cache_state,
            result,
            ..
        } = outcome;
        CapabilityRunRecord {
            target: target.label.id.clone(),
            kind: target.kind.clone(),
            capability: run_capability.name.clone(),
            status: "completed".to_string(),
            action_digest: action_digest.to_string(),
            cache: cache_tag.to_string(),
            output_groups: run_capability.output_groups.clone(),
            required_outputs: run_capability.requires_outputs.clone(),
            outputs,
            test_results: None,
            input_digest,
            cache_state,
            result,
        }
    } else {
        run_target_capability(workspace, cache, target, "run", sandbox).await?
    };
    record_capability_run(workspace, &record).await;
    write_record(output, &record).await?;
    Ok(ExitCode::SUCCESS)
}

/// Build a target, walking deps first. If the target kind has an `impl`
/// callable, execute the actions the impl declares; otherwise fall back to the
/// generic marker action in [`action`].
async fn build_target(
    workspace: &Path,
    cache: &CacheProvider,
    target: &GraphTarget,
    session: &analysis::BuildSession,
    sandbox: SandboxMode,
) -> Result<CapabilityRunRecord> {
    let capability = ensure_capability(target, "build")?;
    if let Some(outcome) = session.build_with_analysis(target).await? {
        // Destructure the outcome so `outputs` moves into the record
        // instead of being cloned. `action_digest` is `Copy`,
        // `cache_tag` is `&'static str`, and `provider` is dropped on
        // this path because the run record doesn't surface it yet.
        let analysis::BuildOutcome {
            action_digest,
            input_digest,
            outputs,
            cache_state,
            result,
            cache_tag,
            ..
        } = outcome;
        Ok(CapabilityRunRecord {
            target: target.label.id.clone(),
            kind: target.kind.clone(),
            capability: capability.name.clone(),
            status: "completed".to_string(),
            action_digest: action_digest.to_string(),
            cache: cache_tag.to_string(),
            output_groups: capability.output_groups.clone(),
            required_outputs: capability.requires_outputs.clone(),
            outputs,
            test_results: None,
            input_digest,
            cache_state,
            result,
        })
    } else {
        run_target_capability(workspace, cache, target, "build", sandbox).await
    }
}

pub fn load_graph_for_capability(
    workspace: &Path,
    target_id: &str,
    capability: &str,
) -> Result<Option<Vec<GraphTarget>>> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    Ok(graph_supports(&graph, target_id, capability).then_some(graph))
}

fn graph_supports(graph: &[GraphTarget], target_id: &str, capability: &str) -> bool {
    graph
        .iter()
        .find(|target| target.label.id == target_id)
        .is_some_and(|target| {
            target
                .capabilities
                .iter()
                .any(|candidate| candidate.name == capability)
        })
}

async fn run_target_capability(
    workspace: &Path,
    cache: &CacheProvider,
    target: &GraphTarget,
    capability_name: &str,
    sandbox: SandboxMode,
) -> Result<CapabilityRunRecord> {
    let capability = ensure_capability(target, capability_name)?;
    let outputs = action::output_paths(target, capability_name)?;
    let mut action = action::action_for(target, capability_name, &outputs)?;
    set_sandbox(&mut action, sandbox);
    let outcome = once_core::run_with_cache(&action, workspace, cache, RunOpts::default())
        .await
        .with_context(|| format!("executing {capability_name} for {}", target.label.id))?;
    if outcome.result.exit_code != 0 {
        crate::commands::evidence::record_outcome(
            workspace,
            EvidenceSubject::target(target.label.id.as_str(), capability_name),
            &action,
            &outcome,
        )
        .await;
        anyhow::bail!(
            "{} failed for {} with exit code {}",
            capability_name,
            target.label.id,
            outcome.result.exit_code
        );
    }
    let cache = cache_tag(outcome.cache).to_string();
    let cache_state = EvidenceCacheState::from(outcome.cache);
    let result = outcome.result;
    Ok(CapabilityRunRecord {
        target: target.label.id.clone(),
        kind: target.kind.clone(),
        capability: capability.name.clone(),
        status: "completed".to_string(),
        action_digest: outcome.action.to_string(),
        cache,
        output_groups: capability.output_groups.clone(),
        required_outputs: capability.requires_outputs.clone(),
        outputs: outputs
            .into_iter()
            .map(|output| output.as_str().to_string())
            .collect(),
        test_results: None,
        input_digest: action.input_digest(),
        cache_state,
        result,
    })
}

fn set_sandbox(action: &mut once_core::Action, sandbox_mode: SandboxMode) {
    if let once_core::Action::RunCommand { sandbox, .. } = action {
        *sandbox = (*sandbox).stronger(sandbox_mode);
    }
}

async fn record_capability_run(workspace: &Path, record: &CapabilityRunRecord) {
    let Some(action_digest) = Digest::from_hex(&record.action_digest) else {
        tracing::warn!(
            target = %record.target,
            capability = %record.capability,
            action_digest = %record.action_digest,
            "skipping evidence for invalid action digest"
        );
        return;
    };
    crate::commands::evidence::record_action_result(
        workspace,
        EvidenceSubject::target(record.target.as_str(), record.capability.as_str()),
        action_digest,
        record.input_digest,
        record.cache_state,
        &record.result,
    )
    .await;
}

fn ensure_capability<'a>(
    target: &'a GraphTarget,
    capability: &str,
) -> Result<&'a once_frontend::Capability> {
    target
        .capabilities
        .iter()
        .find(|candidate| candidate.name == capability)
        .ok_or_else(|| unsupported_capability(target, capability))
}

fn unsupported_capability(target: &GraphTarget, capability: &str) -> anyhow::Error {
    let available = target
        .capabilities
        .iter()
        .map(|capability| capability.name.as_str())
        .collect::<Vec<_>>();
    if available.is_empty() {
        return anyhow!(
            "{} ({}) does not expose any capabilities",
            target.label.id,
            target.kind
        );
    }
    anyhow!(
        "{} ({}) does not expose `{}`. Available capabilities: {}",
        target.label.id,
        target.kind,
        capability,
        available.join(", ")
    )
}

async fn write_record(output: Output, record: &CapabilityRunRecord) -> Result<()> {
    let body = match output.format {
        Format::Human => render_human(record),
        Format::Json | Format::Toon => render::structured(output.format, record)?,
    };
    let mut out = tokio::io::stdout();
    out.write_all(body.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}

fn render_human(record: &CapabilityRunRecord) -> String {
    let groups = if record.output_groups.is_empty() {
        "none".to_string()
    } else {
        record.output_groups.join(", ")
    };
    let mut out = format!(
        "once: {} {} ({}) cache {}, exit=0\noutputs: {}\n",
        record.capability, record.target, record.kind, record.cache, groups
    );
    if !record.required_outputs.is_empty() {
        out.push_str("requires: ");
        out.push_str(&record.required_outputs.join(", "));
        out.push('\n');
    }
    if !record.outputs.is_empty() {
        out.push_str("paths:\n");
        for path in &record.outputs {
            out.push_str("  ");
            out.push_str(path);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use once_frontend::{Capability, TargetLabel};

    fn action_result() -> ActionResult {
        ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs: BTreeMap::new(),
        }
    }

    fn graph_target(kind: &str, capabilities: &[&str]) -> GraphTarget {
        GraphTarget {
            label: TargetLabel {
                package: "apps/ios".to_string(),
                name: "App".to_string(),
                id: "apps/ios/App".to_string(),
            },
            kind: kind.to_string(),
            deps: Vec::new(),
            dependency_edges: BTreeMap::new(),
            srcs: Vec::new(),
            visibility: Vec::new(),
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
    fn lint_provider_requires_structured_output_paths() {
        let target = graph_target("custom_lint", &["lint"]);
        let provider = serde_json::json!({
            "lint_info": {
                "outputs": {
                    "sarif": ".once/out/lint/report.sarif"
                }
            }
        });

        let error = validate_lint_provider(&target, &provider).unwrap_err();
        let failure = error
            .downcast_ref::<once_frontend::analysis::AnalysisFailure>()
            .expect("structured analysis failure");

        assert_eq!(failure.diagnostic.code, "invalid_lint_provider_output");
        assert_eq!(failure.diagnostic.target.as_deref(), Some("apps/ios/App"));
        assert_eq!(
            failure.diagnostic.attribute.as_deref(),
            Some("lint_info.outputs.results")
        );
        assert!(!failure.diagnostic.repairs.is_empty());
    }

    #[test]
    fn ensure_capability_returns_matching_capability() {
        let target = graph_target("apple_application", &["build", "run"]);
        let capability = ensure_capability(&target, "run").unwrap();
        assert_eq!(capability.name, "run");
    }

    #[test]
    fn graph_supports_finds_a_capability_in_a_preloaded_graph() {
        let graph = vec![graph_target("apple_application", &["build", "run"])];

        assert!(graph_supports(&graph, "apps/ios/App", "run"));
        assert!(!graph_supports(&graph, "apps/ios/App", "test"));
        assert!(!graph_supports(&graph, "apps/ios/Missing", "run"));
    }

    #[test]
    fn unsupported_capability_lists_available_capabilities() {
        let target = graph_target("apple_application", &["build", "run"]);
        let err = ensure_capability(&target, "test").unwrap_err().to_string();
        assert!(err.contains("does not expose `test`"));
        assert!(err.contains("Available capabilities: build, run"));
    }

    #[test]
    fn unsupported_capability_reports_when_none_declared() {
        let target = graph_target("mystery", &[]);
        let err = ensure_capability(&target, "build").unwrap_err().to_string();
        assert!(err.contains("does not expose any capabilities"));
    }

    #[test]
    fn render_human_includes_requires_and_paths() {
        let record = CapabilityRunRecord {
            target: "apps/ios/App".to_string(),
            kind: "apple_application".to_string(),
            capability: "run".to_string(),
            status: "completed".to_string(),
            action_digest: "deadbeef".to_string(),
            cache: "miss".to_string(),
            output_groups: vec!["default".to_string()],
            required_outputs: vec!["bundle".to_string()],
            outputs: vec![".once/out/apps/ios/App/run".to_string()],
            test_results: None,
            input_digest: None,
            cache_state: EvidenceCacheState::Miss,
            result: action_result(),
        };

        let rendered = render_human(&record);

        assert!(rendered.contains("once: run apps/ios/App (apple_application) cache miss, exit=0"));
        assert!(rendered.contains("outputs: default"));
        assert!(rendered.contains("requires: bundle"));
        assert!(rendered.contains("  .once/out/apps/ios/App/run"));
    }

    #[test]
    fn render_human_reports_no_output_groups() {
        let record = CapabilityRunRecord {
            target: "apps/ios/App".to_string(),
            kind: "apple_application".to_string(),
            capability: "build".to_string(),
            status: "completed".to_string(),
            action_digest: "deadbeef".to_string(),
            cache: "hit".to_string(),
            output_groups: Vec::new(),
            required_outputs: Vec::new(),
            outputs: Vec::new(),
            test_results: None,
            input_digest: None,
            cache_state: EvidenceCacheState::Hit,
            result: action_result(),
        };

        let rendered = render_human(&record);

        assert!(rendered.contains("outputs: none"));
        assert!(!rendered.contains("requires:"));
        assert!(!rendered.contains("paths:"));
    }
}
