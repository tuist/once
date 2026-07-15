use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

use futures::future::try_join_all;
use tokio::process::Command;

use super::{
    managed_mise,
    mise_runtime::{managed_mise_cache_dir, managed_mise_config_dir, managed_mise_data_dir},
    path::build_action_path,
    select_extra_env, tool_env, MANAGED_MISE_VERSION,
};

/// Resolve `tool` through the workspace's mise config when one exists.
///
/// Workspaces without `mise.toml` keep the historical behavior and
/// return the tool name so the runner resolves it through the declared
/// action `PATH`.
pub async fn workspace_tool(workspace: &Path, tool: &str) -> Result<String, ToolEnvError> {
    workspace_executable(workspace, tool, &[tool]).await
}

/// Whether `workspace` pins a mise toolchain via `mise.toml`.
///
/// Callers that resolve declared tools use this to skip mise entirely
/// for workspaces that rely on the host toolchain, keeping the historical
/// host-`PATH` resolution intact.
pub fn workspace_has_mise_config(workspace: &Path) -> bool {
    has_mise_config(workspace)
}

/// Resolve an executable while activating the declared workspace tools.
pub async fn workspace_executable(
    workspace: &Path,
    executable: &str,
    tools: &[&str],
) -> Result<String, ToolEnvError> {
    if !has_mise_config(workspace) {
        return Ok(executable.to_string());
    }
    workspace_tool_without_prepare(workspace, executable, tools).await
}

/// Install requested workspace tools before cacheable action execution.
pub async fn workspace_prepare_tools(workspace: &Path, tools: &[&str]) -> Result<(), ToolEnvError> {
    ensure_workspace_tools(workspace, tools).await
}

/// Build a mise execution prefix for a configured workspace.
pub async fn workspace_mise_command(workspace: &Path) -> Result<Option<Vec<String>>, ToolEnvError> {
    if !has_mise_config(workspace) {
        return Ok(None);
    }
    let mut argv = vec![
        managed_mise().await?.display().to_string(),
        "exec".to_string(),
        "-C".to_string(),
        workspace.display().to_string(),
    ];
    if has_mise_lock(workspace) {
        argv.push("--locked".to_string());
    }
    argv.push("--".to_string());
    Ok(Some(argv))
}

/// Environment required by managed mise during action execution.
pub fn workspace_mise_env(workspace: &Path, tools: &[&str]) -> BTreeMap<String, String> {
    if !has_mise_config(workspace) {
        return BTreeMap::new();
    }
    let mut selected = BTreeMap::new();
    // The action environment is built from scratch rather than inherited,
    // so mise's home directory has to be forwarded explicitly. The
    // variable that names it is platform specific: `HOME` on Unix,
    // `USERPROFILE` on Windows.
    for key in ["HOME", "USERPROFILE"] {
        if let Some(value) = env::var_os(key) {
            selected.insert(key.into(), value.to_string_lossy().into_owned());
        }
    }
    for (key, value) in managed_mise_isolation(workspace) {
        selected.insert(key.into(), value);
    }
    selected.insert("MISE_ENABLE_TOOLS".into(), tools.join(","));
    selected.insert("MISE_AUTO_INSTALL".into(), "0".into());
    selected.insert("MISE_EXEC_AUTO_INSTALL".into(), "0".into());
    selected
}

/// Build an argv prefix that runs `tool` in the workspace's mise environment.
///
/// The managed mise runtime prepares the requested tool before the action is
/// created. Execution then disables mise's implicit installer, keeping network
/// access and tool mutation outside the cacheable action.
pub async fn workspace_tool_command(
    workspace: &Path,
    tool: &str,
) -> Result<Vec<String>, ToolEnvError> {
    if !has_mise_config(workspace) {
        return Ok(vec![tool.to_string()]);
    }
    ensure_workspace_tools(workspace, &[tool]).await?;
    let mut argv = workspace_mise_command(workspace)
        .await?
        .expect("mise command exists when mise config exists");
    argv.push(tool.to_string());
    Ok(argv)
}

async fn workspace_tool_without_prepare(
    workspace: &Path,
    tool: &str,
    enabled_tools: &[&str],
) -> Result<String, ToolEnvError> {
    let mise = managed_mise().await?;
    let mut command = Command::new(&mise);
    configure_mise_command(&mut command, workspace);
    enable_mise_tools(&mut command, enabled_tools);
    let output = command
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

    ensure_workspace_tools(workspace, tools).await?;
    let mise_env = mise_env(workspace).await?;
    let mut selected = select_extra_env(&mise_env, extra_keys);
    let path = workspace_path(workspace, tools, &mise_env).await?;
    selected.insert("PATH".into(), path);
    selected.extend(workspace_mise_env(workspace, tools));
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

fn has_mise_lock(workspace: &Path) -> bool {
    workspace.join("mise.lock").is_file()
}

async fn ensure_workspace_tools(workspace: &Path, tools: &[&str]) -> Result<(), ToolEnvError> {
    if !has_mise_config(workspace) || tools.is_empty() {
        return Ok(());
    }
    let mise = managed_mise().await?;
    let mut command = Command::new(&mise);
    configure_mise_command(&mut command, workspace);
    command.args(["install", "-C"]).arg(workspace);
    if has_mise_lock(workspace) {
        command.arg("--locked");
    }
    command
        .args(["--yes", "--quiet"])
        .args(tools)
        .env("MISE_ENABLE_TOOLS", tools.join(","));
    let output = command
        .output()
        .await
        .map_err(|source| ToolEnvError::SpawnMise { source })?;
    if !output.status.success() {
        return Err(ToolEnvError::MiseFailed {
            command: format!("mise install {}", tools.join(" ")),
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(())
}

async fn mise_env(workspace: &Path) -> Result<BTreeMap<String, String>, ToolEnvError> {
    let mise = managed_mise().await?;
    let mut command = Command::new(&mise);
    configure_mise_command(&mut command, workspace);
    let output = command
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

fn configure_mise_command(command: &mut Command, workspace: &Path) {
    for (key, value) in managed_mise_isolation(workspace) {
        command.env(key, value);
    }
}

/// Directories that isolate managed mise from the user's global mise
/// installation.
///
/// The analysis-time command configuration and the execution-time action
/// environment must agree on these, otherwise a tool installed during
/// analysis would not be found at execution. Defining them once keeps the
/// two paths in lockstep.
fn managed_mise_isolation(workspace: &Path) -> [(&'static str, String); 4] {
    [
        (
            "MISE_DATA_DIR",
            managed_mise_data_dir().display().to_string(),
        ),
        (
            "MISE_CONFIG_DIR",
            managed_mise_config_dir().display().to_string(),
        ),
        (
            "MISE_CACHE_DIR",
            managed_mise_cache_dir().display().to_string(),
        ),
        ("MISE_TRUSTED_CONFIG_PATHS", workspace.display().to_string()),
    ]
}

fn enable_mise_tools(command: &mut Command, tools: &[&str]) {
    if !tools.is_empty() {
        command.env("MISE_ENABLE_TOOLS", tools.join(","));
    }
}

async fn workspace_path(
    workspace: &Path,
    tools: &[&str],
    mise_env: &BTreeMap<String, String>,
) -> Result<String, ToolEnvError> {
    let tool_paths = try_join_all(tools.iter().map(|tool| async move {
        workspace_tool_without_prepare(workspace, tool, tools)
            .await
            .map(PathBuf::from)
    }))
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
    #[error("mise {MANAGED_MISE_VERSION} is unavailable for {operating_system}-{architecture}")]
    UnsupportedMisePlatform {
        operating_system: &'static str,
        architecture: &'static str,
    },
    #[error("failed to download managed mise: {source}")]
    DownloadMise {
        #[source]
        source: reqwest::Error,
    },
    #[error("managed mise download returned status {status}")]
    MiseDownloadStatus { status: reqwest::StatusCode },
    #[error("managed mise asset {asset} failed checksum verification: expected {expected}, got {actual}")]
    MiseChecksum {
        asset: &'static str,
        expected: &'static str,
        actual: String,
    },
    #[error("failed to {action} for managed mise at {}: {source}", path.display())]
    InstallMiseIo {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
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

    #[test]
    fn configured_workspace_mise_env_isolated_and_non_installing() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("mise.toml"), "[tools]\nrust = '1.96.0'\n").unwrap();

        let selected = workspace_mise_env(tmp.path(), &["rust"]);

        assert_eq!(
            selected.get("MISE_AUTO_INSTALL").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            selected.get("MISE_EXEC_AUTO_INSTALL").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            selected.get("MISE_ENABLE_TOOLS").map(String::as_str),
            Some("rust")
        );
        assert_eq!(
            selected
                .get("MISE_TRUSTED_CONFIG_PATHS")
                .map(String::as_str),
            Some(tmp.path().to_string_lossy().as_ref())
        );
        assert!(selected.contains_key("MISE_DATA_DIR"));
        assert!(selected.contains_key("MISE_CONFIG_DIR"));
        assert!(selected.contains_key("MISE_CACHE_DIR"));
    }
}
