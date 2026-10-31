use std::path::Path;

use super::{archive_error, TempArchive};
use crate::remote::{phase_permit, PhasePermit};
use crate::{ExecutionPermit, RemoteIoLimits, Result, WorkspacePath};

mod build;
mod plan;
#[cfg(all(test, any(target_os = "linux", target_vendor = "apple")))]
mod tests;

pub(in crate::remote) async fn create_input_archive_limited(
    workspace_root: &Path,
    inputs: &[WorkspacePath],
    provider: &'static str,
    limits: RemoteIoLimits,
    lease: Option<&ExecutionPermit>,
) -> Result<(TempArchive, PhasePermit)> {
    let workspace = workspace_root.to_path_buf();
    let inputs = inputs.to_vec();
    let planning_lease = lease.cloned();
    let planned = tokio::task::spawn_blocking(move || {
        let _lease = planning_lease;
        plan::InputPlan::collect(&workspace, &inputs, provider, limits)
    })
    .await
    .map_err(|source| archive_error(provider, format!("input planning task failed: {source}")))??;
    let phase = phase_permit(lease, planned.archive_bytes).await?;
    let building_lease = phase.clone();
    let workspace = workspace_root.to_path_buf();
    let archive = tokio::task::spawn_blocking(move || {
        let _lease = building_lease;
        build::archive(&workspace, planned, provider)
    })
    .await
    .map_err(|source| archive_error(provider, format!("input archive task failed: {source}")))??;
    Ok((archive, phase))
}
