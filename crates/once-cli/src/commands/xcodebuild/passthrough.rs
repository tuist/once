use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{bail, Context, Result};

pub(super) fn run(argv: &[String]) -> Result<ExitCode> {
    let executable = real_xcodebuild()?;
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

fn real_xcodebuild() -> Result<PathBuf> {
    if let Some(path) = env::var_os("ONCE_XCODEBUILD_PATH").map(PathBuf::from) {
        if executable(&path) {
            return Ok(path);
        }
        bail!(
            "ONCE_XCODEBUILD_PATH does not name an executable: {}",
            path.display()
        );
    }
    let current = env::current_exe()
        .ok()
        .and_then(|path| path.canonicalize().ok());
    for directory in env::var_os("PATH").iter().flat_map(env::split_paths) {
        let candidate = directory.join("xcodebuild");
        if !executable(&candidate) || is_current_executable(&candidate, current.as_deref()) {
            continue;
        }
        return Ok(candidate);
    }
    bail!("could not find a system xcodebuild outside the Once compatibility wrapper")
}

fn is_current_executable(candidate: &Path, current: Option<&Path>) -> bool {
    current.is_some_and(|current| candidate.canonicalize().is_ok_and(|path| path == current))
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
        let executable_path = directory.path().join("xcodebuild");
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
}
