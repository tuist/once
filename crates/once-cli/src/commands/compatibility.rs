use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{bail, Context, Result};
use once_core::{ResourceLimits, Xdg};

use crate::cli::{CompatibilityInvocation, Output};

pub async fn run(
    workspace: &Path,
    xdg: &Xdg,
    output: Output,
    resource_limits: ResourceLimits,
    invocation: CompatibilityInvocation,
) -> Result<ExitCode> {
    match invocation {
        CompatibilityInvocation::Xcodebuild(argv) => {
            Box::pin(crate::commands::xcodebuild::run(
                workspace,
                xdg,
                output,
                resource_limits,
                argv,
            ))
            .await
        }
        CompatibilityInvocation::Swift(argv) => {
            Box::pin(crate::commands::swift::run(
                workspace,
                xdg,
                output,
                resource_limits,
                argv,
            ))
            .await
        }
        CompatibilityInvocation::Bazel(argv) => {
            Box::pin(crate::commands::bazel::run(
                workspace,
                xdg,
                output,
                resource_limits,
                argv,
            ))
            .await
        }
        CompatibilityInvocation::Cargo(argv) => {
            Box::pin(crate::commands::cargo::run(
                workspace,
                xdg,
                output,
                resource_limits,
                argv,
            ))
            .await
        }
    }
}

pub(crate) fn run_passthrough(
    argv: &[String],
    executable_name: &str,
    override_variable: &str,
) -> Result<ExitCode> {
    let executable = system_executable(executable_name, override_variable)?;
    let status = Command::new(&executable)
        .args(argv)
        .status()
        .with_context(|| format!("starting {}", executable.display()))?;
    Ok(ExitCode::from(
        status
            .code()
            .and_then(|code| u8::try_from(code).ok())
            .unwrap_or(255),
    ))
}

pub(crate) fn system_executable(executable_name: &str, override_variable: &str) -> Result<PathBuf> {
    if let Some(path) = env::var_os(override_variable).map(PathBuf::from) {
        if executable(&path) {
            return Ok(path);
        }
        bail!(
            "{override_variable} does not name an executable: {}",
            path.display()
        );
    }
    let current = env::current_exe()
        .ok()
        .and_then(|path| path.canonicalize().ok());
    for directory in env::var_os("PATH").iter().flat_map(env::split_paths) {
        let candidate = directory.join(executable_name);
        if !executable(&candidate) || is_current_executable(&candidate, current.as_deref()) {
            continue;
        }
        if is_command_shim(&candidate) {
            continue;
        }
        return Ok(candidate);
    }
    if let Some(path) = xcrun_executable(executable_name, current.as_deref()) {
        return Ok(path);
    }
    bail!("could not find a system {executable_name} outside the Once compatibility wrapper")
}

#[cfg(target_os = "macos")]
fn xcrun_executable(executable_name: &str, current: Option<&Path>) -> Option<PathBuf> {
    let output = Command::new("xcrun")
        .args(["--find", executable_name])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    (executable(&path) && !is_current_executable(&path, current)).then_some(path)
}

#[cfg(not(target_os = "macos"))]
fn xcrun_executable(_executable_name: &str, _current: Option<&Path>) -> Option<PathBuf> {
    None
}

fn is_current_executable(candidate: &Path, current: Option<&Path>) -> bool {
    current.is_some_and(|current| candidate.canonicalize().is_ok_and(|path| path == current))
}

fn is_command_shim(candidate: &Path) -> bool {
    candidate
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == "shims")
}

fn executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        path.metadata()
            .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_executable_files() {
        let directory = tempfile::tempdir().unwrap();
        let executable_path = directory.path().join("tool");
        std::fs::write(&executable_path, "").unwrap();
        assert!(!executable(&executable_path));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = std::fs::metadata(&executable_path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&executable_path, permissions).unwrap();
            assert!(executable(&executable_path));
        }
    }

    #[test]
    fn skips_command_shims() {
        assert!(is_command_shim(Path::new("/tools/shims/tool")));
        assert!(!is_command_shim(Path::new("/tools/bin/tool")));
    }
}
