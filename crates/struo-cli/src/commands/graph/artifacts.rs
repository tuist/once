use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use struo_cas::CacheProvider;
use struo_core::artifacts::{export_artifacts, ArtifactExport, ArtifactInfo};
use struo_core::{ResourceLimits, ResourcePool, ResourceRequest, SandboxMode};

use super::{analysis, capability, ResolvedConfiguration};

pub async fn export(
    workspace: &Path,
    cache: &CacheProvider,
    target_id: &str,
    destination: &Path,
    sandbox: SandboxMode,
    resource_limits: ResourceLimits,
    resolved: &ResolvedConfiguration,
) -> Result<ArtifactExport> {
    let session = analysis::BuildSession::load_workspace_with_configuration(
        workspace,
        cache,
        sandbox,
        resolved,
        &analysis::known_changes(None, None),
    )
    .await?
    .with_resource_limits(resource_limits.clone());
    let target = session.target(target_id)?;
    capability::ensure_capability(target, "artifact")?;
    let outcome = session
        .run_with_analysis_and_provider_validation(target, "artifact", validate_provider)
        .await?
        .context("artifact capability requires a Starlark implementation")?;
    let info = parse_provider(target, &outcome.provider)?;
    let pool = Arc::new(ResourcePool::new(resource_limits));
    let permit = pool
        .acquire_required(ResourceRequest::new(1, 4 * 1024 * 1024))
        .await?;
    export_artifacts(workspace, &info, &outcome.outputs, destination, &permit)
        .await
        .map_err(|error| provider_error(target, &error.to_string()))
}

fn parse_provider(target: &struo_frontend::GraphTarget, provider: &Value) -> Result<ArtifactInfo> {
    let result = serde_json::from_value(
        provider
            .get("artifact_info")
            .cloned()
            .unwrap_or(Value::Null),
    );
    result.map_err(|error| provider_error(target, &error.to_string()))
}

fn validate_provider(target: &struo_frontend::GraphTarget, provider: &Value) -> Result<()> {
    let info = parse_provider(target, provider)?;
    let outputs = info
        .artifacts
        .iter()
        .flat_map(|artifact| {
            artifact
                .payloads
                .values()
                .map(|payload| payload.path.clone())
        })
        .collect::<Vec<_>>();
    info.validate(&outputs)
        .map_err(|error| provider_error(target, &error.to_string()))
}

fn provider_error(target: &struo_frontend::GraphTarget, message: &str) -> anyhow::Error {
    anyhow!(struo_frontend::analysis::AnalysisFailure {
        diagnostic: struo_frontend::Diagnostic::new("invalid_artifact_provider", message)
            .with_target(&target.label.id)
            .with_attribute("artifact_info")
            .with_repair("Return struo.artifact_info.v1 with uniquely named artifacts and declared file payloads; inspect struo query artifacts <target>"),
    })
}

pub fn query(workspace: &Path, target_id: &str) -> Result<Value> {
    let graph = struo_frontend::load_graph_workspace(workspace)?;
    let target_id = struo_frontend::normalize_cli_target_from(workspace, workspace, target_id)?;
    let target = graph.iter().find(|target| target.label.id == target_id)
        .with_context(|| {
            let nearby = graph.iter().take(5).map(|target| format!("`{}`", target.label.id)).collect::<Vec<_>>().join(", ");
            format!("target `{target_id}` not found; use a valid target selector: {nearby}. Run `struo query targets` to list targets")
        })?;
    Ok(json!({
        "schema": "struo.artifact_contract.v1",
        "target": target.label.id,
        "supported": target.capabilities.iter().any(|capability| capability.name == "artifact"),
        "provider": "struo_artifact_info",
        "capability": "artifact",
        "declaration": {
            "artifact_info": {
                "schema": "struo.artifact_info.v1",
                "artifacts": [{
                    "name": "product",
                    "payloads": {"content": {"path": "<declared-output-file>", "media_type": "application/octet-stream"}},
                    "interfaces": {},
                    "selectors": {},
                }],
            },
        },
        "constraints": {
            "metadata_max_bytes": 1024 * 1024,
            "artifacts_max": 128,
            "payloads_per_artifact_max": 128,
            "interfaces_per_artifact_max": 32,
            "interface_shape": {"payload": "<payload-name>", "spec": {}},
            "optional_payloads": "A payload declared with \"optional\": true is left out of the export when its output file is empty, together with interface spec keys ending in _payload whose value names it; an interface's own payload must be required",
            "unknown_interfaces": "Preserved without execution; consumption requires a trusted compatible handler",
            "paths": "Exact declared output files inside the workspace, including after symlink resolution",
        },
        "publish": {"cli": format!("struo artifact publish {}", target.label.id), "mcp": "struo_publish_artifacts"},
    }))
}
