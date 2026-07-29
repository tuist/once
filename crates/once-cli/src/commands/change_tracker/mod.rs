#[cfg(unix)]
mod client;
mod protocol;
#[cfg(unix)]
mod server;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use once_cas::Digest;
use once_core::Xdg;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ChangePosition {
    pub instance_id: String,
    pub source_generation: u64,
    pub output_generation: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ChangeSnapshot {
    pub position: ChangePosition,
    pub source_changes: Option<Vec<String>>,
    pub output_changes: Option<Vec<String>>,
}

#[cfg(unix)]
pub async fn snapshot(
    workspace: &Path,
    xdg: &Xdg,
    outputs: &[String],
    since: Option<&ChangePosition>,
) -> Option<ChangeSnapshot> {
    client::snapshot(workspace, &socket_path(workspace, xdg), outputs, since).await
}

#[cfg(not(unix))]
pub async fn snapshot(
    _workspace: &Path,
    _xdg: &Xdg,
    _outputs: &[String],
    _since: Option<&ChangePosition>,
) -> Option<ChangeSnapshot> {
    None
}

#[cfg(unix)]
pub async fn serve(workspace: &Path, xdg: &Xdg) -> Result<ExitCode> {
    server::serve(workspace, &socket_path(workspace, xdg)).await?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(not(unix))]
pub async fn serve(_workspace: &Path, _xdg: &Xdg) -> Result<ExitCode> {
    anyhow::bail!("the filesystem change tracker is not supported on this platform")
}

fn socket_path(workspace: &Path, xdg: &Xdg) -> PathBuf {
    let workspace = std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let workspace_id = Digest::of_bytes(workspace.to_string_lossy().as_bytes());
    let socket = xdg
        .once_runtime()
        .join("change-tracker")
        .join(format!("{workspace_id}.sock"));
    if socket.as_os_str().len() < 100 {
        return socket;
    }
    let runtime_id = Digest::of_bytes(xdg.once_runtime().to_string_lossy().as_bytes()).to_string();
    std::env::temp_dir().join(format!(
        "once-ct-{}-{}.sock",
        &runtime_id[..12],
        &workspace_id.to_string()[..24]
    ))
}
