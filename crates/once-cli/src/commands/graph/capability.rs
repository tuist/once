//! Running one capability against one graph target, and reporting it.
//!
//! Holds the record shape a capability run produces plus the helpers that
//! resolve, execute, and render it. Split out of the graph command
//! dispatch so `mod.rs` stays a table of contents.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use once_cas::{ActionResult, CacheProvider, Digest};
use once_core::{
    EvidenceCacheState, EvidenceSubject, InputFingerprintManifest, RunOpts, SandboxMode,
};
use once_frontend::GraphTarget;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use super::{action, analysis};
use crate::cli::{Format, Output};
use crate::commands::util::cache_tag;
use crate::render;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct CapabilityRunRecord {
    pub target: String,
    pub kind: String,
    pub capability: String,
    pub status: String,
    pub action_digest: String,
    pub cache: String,
    pub output_groups: Vec<String>,
    pub required_outputs: Vec<String>,
    pub outputs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_results: Option<String>,
    #[serde(skip, default)]
    pub input_digest: Option<Digest>,
    #[serde(skip, default)]
    pub input_fingerprint: Option<InputFingerprintManifest>,
    #[serde(skip, default = "default_cache_state")]
    pub cache_state: EvidenceCacheState,
    #[serde(skip, default = "default_action_result")]
    pub result: ActionResult,
}

pub(super) fn default_cache_state() -> EvidenceCacheState {
    EvidenceCacheState::Hit
}

pub(super) fn default_action_result() -> ActionResult {
    ActionResult {
        exit_code: 0,
        stdout: None,
        stderr: None,
        outputs: BTreeMap::new(),
    }
}

pub(super) async fn build_target(
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
            input_fingerprint,
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
            input_fingerprint,
            cache_state,
            result,
        })
    } else {
        run_target_capability(workspace, cache, target, "build", sandbox).await
    }
}

pub fn load_graph_for_capability_with_configuration(
    workspace: &Path,
    target_id: &str,
    capability: &str,
    resolved: &super::ResolvedConfiguration,
) -> Result<Option<Vec<GraphTarget>>> {
    let graph =
        once_frontend::load_graph_workspace_with_configuration(workspace, &resolved.configuration)
            .context("loading graph")?;
    Ok(graph_supports(&graph, target_id, capability).then_some(graph))
}

pub(super) fn graph_supports(graph: &[GraphTarget], target_id: &str, capability: &str) -> bool {
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

pub(super) async fn run_target_capability(
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
        input_fingerprint: None,
        cache_state,
        result,
    })
}

pub(super) fn set_sandbox(action: &mut once_core::Action, sandbox_mode: SandboxMode) {
    if let once_core::Action::RunCommand { sandbox, .. } = action {
        *sandbox = (*sandbox).stronger(sandbox_mode);
    }
}

pub(super) async fn record_capability_run(workspace: &Path, record: &CapabilityRunRecord) {
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
        record.input_fingerprint.clone(),
        record.cache_state,
        &record.result,
    )
    .await;
}

pub(super) fn ensure_capability<'a>(
    target: &'a GraphTarget,
    capability: &str,
) -> Result<&'a once_frontend::Capability> {
    target
        .capabilities
        .iter()
        .find(|candidate| candidate.name == capability)
        .ok_or_else(|| unsupported_capability(target, capability))
}

pub(super) fn unsupported_capability(target: &GraphTarget, capability: &str) -> anyhow::Error {
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

pub(super) async fn write_record(output: Output, record: &CapabilityRunRecord) -> Result<()> {
    let body = match output.format {
        Format::Human => render_human(record),
        Format::Json | Format::Toon => render::structured(output.format, record)?,
    };
    let mut out = tokio::io::stdout();
    out.write_all(body.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}

pub(super) fn render_human(record: &CapabilityRunRecord) -> String {
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
