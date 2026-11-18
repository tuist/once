use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;
use struo_cas::Digest;
use struo_core::{ReproducibilityTrial, ResourceLimits, SandboxMode};

use super::{analysis, capability, ResolvedConfiguration};
use crate::cli::Output;

pub async fn verify_build(
    workspace: &Path,
    target_id: &str,
    sandbox: SandboxMode,
    resource_limits: ResourceLimits,
    resolved: &ResolvedConfiguration,
    output: Output,
) -> Result<ExitCode> {
    let target_id = target_id.to_string();
    let subject = format!("build {target_id}");
    let configuration = resolved.clone();
    let limits = resource_limits.clone();
    let report = crate::commands::verification::run(
        workspace,
        subject,
        Vec::new(),
        resource_limits,
        move |workspace, cache| {
            let configuration = configuration.clone();
            let limits = limits.clone();
            let target_id = target_id.clone();
            async move {
                let changes = analysis::known_changes(None, None);
                let session = analysis::BuildSession::load_workspace_with_configuration(
                    &workspace,
                    &cache,
                    sandbox,
                    &configuration,
                    &changes,
                )
                .await?
                .with_resource_limits(limits);
                let target = session.target(&target_id)?;
                let record =
                    capability::build_target(&workspace, &cache, target, &session, sandbox).await?;
                Ok(ReproducibilityTrial {
                    action_digest: Digest::from_hex(&record.action_digest),
                    result: Some(record.result),
                    error: None,
                    log_paths: session.log_paths(),
                })
            }
        },
    )
    .await?;
    crate::commands::verification::write(&report, output).await
}
