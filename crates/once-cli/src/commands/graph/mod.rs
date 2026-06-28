//! Graph capability commands for build, run, and test.
//!
//! This module owns command orchestration: resolving a target from the
//! workspace graph, checking the requested capability, executing actions
//! declared by target kinds or generic fallback actions, and rendering the result. The legacy
//! capability fallback lives in [`action`].

mod action;
mod analysis;

use std::path::Path;
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use once_cas::{ActionResult, CacheProvider, Digest};
use once_core::{EvidenceCacheState, EvidenceSubject, RunOpts};
use once_frontend::analysis::AnalysisOptions;
use once_frontend::GraphTarget;
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::cli::{Format, Output};
use crate::commands::util::cache_tag;
use crate::render;

#[derive(Debug, Serialize)]
struct CapabilityRunRecord {
    target: String,
    kind: String,
    capability: String,
    status: &'static str,
    action_digest: String,
    cache: &'static str,
    output_groups: Vec<String>,
    required_outputs: Vec<String>,
    outputs: Vec<String>,
    #[serde(skip)]
    input_digest: Option<Digest>,
    #[serde(skip)]
    cache_state: EvidenceCacheState,
    #[serde(skip)]
    result: ActionResult,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GraphRunOptions {
    pub visible: bool,
}

pub async fn build(
    workspace: &Path,
    cache: &CacheProvider,
    output: Output,
    target_id: &str,
) -> Result<ExitCode> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    let target = require_target(&graph, target_id)?;
    let session = analysis::BuildSession::new(workspace, cache, &graph)?;
    let record = build_target(workspace, cache, &target, &session).await?;
    record_capability_run(workspace, &record).await;
    write_record(output, &record).await?;
    Ok(ExitCode::SUCCESS)
}

pub async fn test(
    workspace: &Path,
    cache: &CacheProvider,
    output: Output,
    target_id: &str,
) -> Result<ExitCode> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    let target = require_target(&graph, target_id)?;
    let test_capability = ensure_capability(&target, "test")?;
    let session = analysis::BuildSession::new(workspace, cache, &graph)?;
    if !test_capability.requires_outputs.is_empty()
        && target
            .capabilities
            .iter()
            .any(|capability| capability.name == "build")
    {
        let build_record = build_target(workspace, cache, &target, &session).await?;
        record_capability_run(workspace, &build_record).await;
    }
    let record = if let Some(outcome) = session.run_with_analysis(&target, "test").await? {
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
            capability: test_capability.name.clone(),
            status: "completed",
            action_digest: action_digest.to_string(),
            cache: cache_tag,
            output_groups: test_capability.output_groups.clone(),
            required_outputs: test_capability.requires_outputs.clone(),
            outputs,
            input_digest,
            cache_state,
            result,
        }
    } else {
        run_target_capability(workspace, cache, &target, "test").await?
    };
    record_capability_run(workspace, &record).await;
    write_record(output, &record).await?;
    Ok(ExitCode::SUCCESS)
}

pub async fn run(
    workspace: &Path,
    cache: &CacheProvider,
    output: Output,
    target_id: &str,
    options: GraphRunOptions,
) -> Result<ExitCode> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    let target = require_target(&graph, target_id)?;
    let run_capability = ensure_capability(&target, "run")?;
    let session = analysis::BuildSession::new_with_options(
        workspace,
        cache,
        &graph,
        AnalysisOptions {
            run_visible: options.visible,
        },
    )?;
    if !run_capability.requires_outputs.is_empty()
        && target
            .capabilities
            .iter()
            .any(|capability| capability.name == "build")
    {
        let build_record = build_target(workspace, cache, &target, &session).await?;
        record_capability_run(workspace, &build_record).await;
    }
    let record = if let Some(outcome) = session.run_with_analysis(&target, "run").await? {
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
            status: "completed",
            action_digest: action_digest.to_string(),
            cache: cache_tag,
            output_groups: run_capability.output_groups.clone(),
            required_outputs: run_capability.requires_outputs.clone(),
            outputs,
            input_digest,
            cache_state,
            result,
        }
    } else {
        run_target_capability(workspace, cache, &target, "run").await?
    };
    record_capability_run(workspace, &record).await;
    write_record(output, &record).await?;
    Ok(ExitCode::SUCCESS)
}

fn require_target(graph: &[GraphTarget], target_id: &str) -> Result<GraphTarget> {
    graph
        .iter()
        .find(|target| target.label.id == target_id)
        .cloned()
        .with_context(|| format!("no target matches `{target_id}`"))
}

/// Build a target, walking deps first. If the target kind has an `impl`
/// callable, execute the actions the impl declares; otherwise fall back to the
/// generic marker action in [`action`].
async fn build_target(
    workspace: &Path,
    cache: &CacheProvider,
    target: &GraphTarget,
    session: &analysis::BuildSession,
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
            status: "completed",
            action_digest: action_digest.to_string(),
            cache: cache_tag,
            output_groups: capability.output_groups.clone(),
            required_outputs: capability.requires_outputs.clone(),
            outputs,
            input_digest,
            cache_state,
            result,
        })
    } else {
        run_target_capability(workspace, cache, target, "build").await
    }
}

pub fn supports(workspace: &Path, target_id: &str, capability: &str) -> Result<bool> {
    let Some(target) = find_graph_target(workspace, target_id)? else {
        return Ok(false);
    };
    Ok(target
        .capabilities
        .iter()
        .any(|candidate| candidate.name == capability))
}

async fn run_target_capability(
    workspace: &Path,
    cache: &CacheProvider,
    target: &GraphTarget,
    capability_name: &str,
) -> Result<CapabilityRunRecord> {
    let capability = ensure_capability(target, capability_name)?;
    let outputs = action::output_paths(target, capability_name)?;
    let action = action::action_for(target, capability_name, &outputs)?;
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
    let cache = cache_tag(outcome.cache);
    let cache_state = EvidenceCacheState::from(outcome.cache);
    let result = outcome.result;
    Ok(CapabilityRunRecord {
        target: target.label.id.clone(),
        kind: target.kind.clone(),
        capability: capability.name.clone(),
        status: "completed",
        action_digest: outcome.action.to_string(),
        cache,
        output_groups: capability.output_groups.clone(),
        required_outputs: capability.requires_outputs.clone(),
        outputs: outputs
            .into_iter()
            .map(|output| output.as_str().to_string())
            .collect(),
        input_digest: action.input_digest(),
        cache_state,
        result,
    })
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

fn find_graph_target(workspace: &Path, target_id: &str) -> Result<Option<GraphTarget>> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    Ok(graph
        .into_iter()
        .find(|target| target.label.id == target_id))
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
    fn ensure_capability_returns_matching_capability() {
        let target = graph_target("apple_application", &["build", "run"]);
        let capability = ensure_capability(&target, "run").unwrap();
        assert_eq!(capability.name, "run");
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
            status: "completed",
            action_digest: "deadbeef".to_string(),
            cache: "miss",
            output_groups: vec!["default".to_string()],
            required_outputs: vec!["bundle".to_string()],
            outputs: vec![".once/out/apps/ios/App/run".to_string()],
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
            status: "completed",
            action_digest: "deadbeef".to_string(),
            cache: "hit",
            output_groups: Vec::new(),
            required_outputs: Vec::new(),
            outputs: Vec::new(),
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
