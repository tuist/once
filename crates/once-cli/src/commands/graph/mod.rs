//! Graph capability commands for build, lint, run, and test.
//!
//! This module owns command orchestration: resolving a target from the
//! workspace graph, checking the requested capability, executing actions
//! declared by target kinds or generic fallback actions, and rendering the result. The legacy
//! capability fallback lives in [`action`].

mod action;
mod analysis;
mod build_receipt;
mod capability;
mod contract;
mod lint;

pub use contract::{validate_action_contracts, ActionContractValidation};

use std::path::Path;
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use once_cas::{CacheProvider, Digest};
use once_core::{EvidenceCacheState, LintSeverity, ResourceLimits, SandboxMode};
use once_frontend::analysis::AnalysisOptions;
use once_frontend::GraphTarget;

pub use self::capability::load_graph_for_capability;
use self::capability::{
    build_target, ensure_capability, record_capability_run, run_target_capability, write_record,
    CapabilityRunRecord,
};
use self::lint::{
    lint_provider_output_path, persist_lint_results, validate_lint_provider, write_lint_results,
};
use crate::cli::Output;

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
        input_fingerprint: outcome.input_fingerprint,
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
            input_fingerprint,
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
            input_fingerprint,
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
            input_fingerprint,
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
            input_fingerprint,
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
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::capability::{graph_supports, render_human};
    use super::*;
    use once_cas::ActionResult;
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
            input_fingerprint: None,
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
            input_fingerprint: None,
            cache_state: EvidenceCacheState::Hit,
            result: action_result(),
        };

        let rendered = render_human(&record);

        assert!(rendered.contains("outputs: none"));
        assert!(!rendered.contains("requires:"));
        assert!(!rendered.contains("paths:"));
    }
}
