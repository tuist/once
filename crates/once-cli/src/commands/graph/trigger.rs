use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use once_cas::{CacheProvider, Digest};
use once_core::{ResourcePool, SandboxMode};
use once_frontend::{Diagnostic, TriggerInvocation};
use serde_json::{json, Value};

use super::analysis::{BuildSession, KnownChanges};
use super::capability::{ensure_capability, record_capability_run, CapabilityRunRecord};
use super::configuration::ResolvedConfiguration;

pub(crate) struct TriggerBatch {
    sessions: Vec<BuildSession>,
    members: Vec<(usize, TriggerInvocation)>,
    pub resolution: Value,
}

impl TriggerBatch {
    pub(crate) async fn prepare(
        workspace: &Path,
        cache: &CacheProvider,
        invocations: &[TriggerInvocation],
        resources: Arc<ResourcePool>,
    ) -> Result<Self> {
        let mut sessions = Vec::new();
        let mut configurations = BTreeMap::new();
        let mut members = Vec::new();
        let mut resolution = Vec::new();
        for invocation in invocations {
            let configuration = invocation.configuration.resolve()?;
            let digest = Digest::of_bytes(&configuration.canonical_bytes());
            let index = if let Some(index) = configurations.get(&digest.to_string()) {
                *index
            } else {
                let resolved = ResolvedConfiguration {
                    configuration,
                    digest,
                    path_suffix: format!("/cfg-{}", &digest.to_hex()[..16]),
                    native_targets: None,
                    execution: None,
                };
                let session = BuildSession::load_workspace_with_configuration(
                    workspace,
                    cache,
                    SandboxMode::Inputs,
                    &resolved,
                    &KnownChanges::Unknown,
                )
                .await?
                .with_resource_pool(Arc::clone(&resources));
                let index = sessions.len();
                configurations.insert(digest.to_string(), index);
                sessions.push(session);
                index
            };
            let session = &sessions[index];
            let target = session.target(&invocation.target)?;
            ensure_capability(target, &invocation.capability)?;
            if !session.has_implementation(target) {
                bail!(
                    "trigger target `{}` needs an analysis-backed target kind",
                    invocation.target
                );
            }
            resolution.push(json!({
                "invocation": invocation,
                "configuration": invocation.configuration.resolve()?,
                "kind": target.kind,
                "resolution": session.trigger_resolution()?,
            }));
            members.push((index, invocation.clone()));
        }
        Ok(Self {
            sessions,
            members,
            resolution: json!(resolution),
        })
    }

    pub(crate) async fn execute(&self, workspace: &Path) -> Vec<Value> {
        let mut records = Vec::new();
        // Roots may share uncacheable dependencies and output directories.
        // Only the session's dependency/action scheduler runs concurrently.
        for (index, invocation) in &self.members {
            let result = execute_member(workspace, &self.sessions[*index], invocation).await;
            records.push(match result {
                Ok(record) => json!({"invocation": invocation, "status": "passed", "result": record}),
                Err(error) => json!({
                    "invocation": invocation, "status": "failed", "error": format!("{error:#}"),
                    "diagnostics": [Diagnostic::new("trigger_invocation_failed", format!("{error:#}"))
                        .with_target(&invocation.target).with_attribute("capability")
                        .with_repair(format!("Inspect once query capabilities {} and repair the failing {} capability", invocation.target, invocation.capability))],
                }),
            });
        }
        records
    }
}

async fn execute_member(
    workspace: &Path,
    session: &BuildSession,
    invocation: &TriggerInvocation,
) -> Result<CapabilityRunRecord> {
    let target = session.target(&invocation.target)?;
    let capability = ensure_capability(target, &invocation.capability)?;
    if invocation.capability != "build"
        && !capability.requires_outputs.is_empty()
        && target
            .capabilities
            .iter()
            .any(|entry| entry.name == "build")
    {
        let outcome = session
            .build_with_analysis(target)
            .await?
            .context("trigger prerequisite has no analysis implementation")?;
        let record = record_from_outcome(target, "build", outcome)?;
        record_capability_run(workspace, &record).await;
    }
    let outcome = if invocation.capability == "build" {
        session.build_with_analysis(target).await?
    } else {
        session
            .run_with_analysis(target, &invocation.capability)
            .await?
    }
    .context("trigger capability has no analysis implementation")?;
    let record = record_from_outcome(target, &invocation.capability, outcome)?;
    record_capability_run(workspace, &record).await;
    Ok(record)
}

fn record_from_outcome(
    target: &once_frontend::GraphTarget,
    capability: &str,
    outcome: super::analysis::BuildOutcome,
) -> Result<CapabilityRunRecord> {
    let declared = ensure_capability(target, capability)?;
    Ok(CapabilityRunRecord {
        target: target.label.id.clone(),
        kind: target.kind.clone(),
        capability: capability.to_string(),
        status: "completed".to_string(),
        action_digest: outcome.action_digest.to_string(),
        cache: outcome.cache_tag.to_string(),
        output_groups: declared.output_groups.clone(),
        required_outputs: declared.requires_outputs.clone(),
        outputs: outcome.outputs,
        test_results: outcome
            .provider
            .pointer("/test_info/outputs/results")
            .and_then(Value::as_str)
            .map(str::to_string),
        input_digest: outcome.input_digest,
        input_fingerprint: outcome.input_fingerprint,
        cache_state: outcome.cache_state,
        result: outcome.result,
    })
}
