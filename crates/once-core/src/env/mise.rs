use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

use futures::future::try_join_all;
use tokio::process::Command;

use super::{path::build_action_path, select_extra_env, tool_env};

/// Resolve `tool` through the workspace's mise config when one exists.
///
/// Workspaces without `mise.toml` keep the historical behavior and
/// return the tool name so the runner resolves it through the declared
/// action `PATH`.
pub async fn workspace_tool(workspace: &Path, tool: &str) -> Result<String, ToolEnvError> {
    if !has_mise_config(workspace) {
        return Ok(tool.to_string());
    }
    let output = Command::new("mise")
        .args(["which", "-C"])
        .arg(workspace)
        .arg(tool)
        .output()
        .await
        .map_err(|source| ToolEnvError::SpawnMise { source })?;
    if !output.status.success() {
        return Err(ToolEnvError::MiseFailed {
            command: format!("mise which {tool}"),
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Build an action environment from the workspace's pinned mise
/// toolchain when `mise.toml` is present.
///
/// The returned `PATH` is not copied from the parent shell. It contains
/// only the directories for tools Once explicitly needs plus stable
/// system directories for linker/shell helpers. Workspaces without
/// `mise.toml` fall back to the historical allowlist policy.
pub async fn workspace_tool_env(
    workspace: &Path,
    tools: &[&str],
    extra_keys: &[&str],
) -> Result<BTreeMap<String, String>, ToolEnvError> {
    if !has_mise_config(workspace) {
        return Ok(tool_env(extra_keys));
    }

    let mise_env = mise_env(workspace).await?;
    let mut selected = select_extra_env(&mise_env, extra_keys);
    let path = workspace_path(workspace, tools, &mise_env).await?;
    selected.insert("PATH".into(), path);
    Ok(selected)
}

/// Read one variable from the workspace's pinned toolchain environment.
///
/// This is used for cache-key material such as `RUSTUP_TOOLCHAIN`, so
/// invoking Once outside `mise exec` still keys actions on the same
/// toolchain that execution will use.
pub async fn workspace_tool_var(
    workspace: &Path,
    key: &str,
) -> Result<Option<String>, ToolEnvError> {
    if !has_mise_config(workspace) {
        return Ok(env::var(key).ok());
    }
    Ok(mise_env(workspace).await?.remove(key))
}

fn has_mise_config(workspace: &Path) -> bool {
    workspace.join("mise.toml").is_file()
}

async fn mise_env(workspace: &Path) -> Result<BTreeMap<String, String>, ToolEnvError> {
    let output = Command::new("mise")
        .args(["env", "--json", "-C"])
        .arg(workspace)
        .output()
        .await
        .map_err(|source| ToolEnvError::SpawnMise { source })?;
    if !output.status.success() {
        return Err(ToolEnvError::MiseFailed {
            command: "mise env --json".to_string(),
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    serde_json::from_slice(&output.stdout).map_err(|source| ToolEnvError::ParseMiseEnv { source })
}

async fn workspace_path(
    workspace: &Path,
    tools: &[&str],
    mise_env: &BTreeMap<String, String>,
) -> Result<String, ToolEnvError> {
    let tool_paths = try_join_all(
        tools
            .iter()
            .map(|tool| async move { workspace_tool(workspace, tool).await.map(PathBuf::from) }),
    )
    .await?;
    build_action_path(&tool_paths, mise_env)
}

#[derive(Debug, thiserror::Error)]
pub enum ToolEnvError {
    #[error("failed to spawn mise: {source}")]
    SpawnMise {
        #[source]
        source: std::io::Error,
    },
    #[error("{command} failed with exit {status}: {stderr}")]
    MiseFailed {
        command: String,
        status: i32,
        stderr: String,
    },
    #[error("failed to parse mise environment JSON: {source}")]
    ParseMiseEnv {
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to build action PATH: {source}")]
    JoinPath {
        #[source]
        source: env::JoinPathsError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn workspace_without_mise_keeps_legacy_env_policy() {
        let tmp = tempfile::TempDir::new().unwrap();
        let env = workspace_tool_env(tmp.path(), &["rustc"], &["RUSTUP_TOOLCHAIN"])
            .await
            .unwrap();
        assert_eq!(env, tool_env(&["RUSTUP_TOOLCHAIN"]));
        assert_eq!(workspace_tool(tmp.path(), "rustc").await.unwrap(), "rustc");
    }
}
