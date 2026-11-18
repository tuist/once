use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

use super::ToolEnvError;

pub(super) fn build_action_path(
    tool_paths: &[PathBuf],
    mise_env: &BTreeMap<String, String>,
) -> Result<String, ToolEnvError> {
    let mut dirs = Vec::<PathBuf>::new();
    for tool_path in tool_paths {
        if let Some(parent) = tool_path.parent() {
            push_unique(&mut dirs, parent.to_path_buf());
        }
    }
    if let Some(cargo_home) = mise_env.get("CARGO_HOME") {
        push_unique(&mut dirs, Path::new(cargo_home).join("bin"));
    }
    for dir in stable_system_path() {
        push_unique(&mut dirs, dir);
    }
    env::join_paths(&dirs)
        .map_err(|source| ToolEnvError::JoinPath { source })
        .map(|path| path.to_string_lossy().into_owned())
}

pub fn is_mise_shim(path: &Path) -> bool {
    path.file_stem().is_some_and(|name| name != "mise")
        && path
            .canonicalize()
            .is_ok_and(|resolved| resolved.file_stem().is_some_and(|name| name == "mise"))
}

fn push_unique(dirs: &mut Vec<PathBuf>, dir: PathBuf) {
    if !dirs.iter().any(|existing| existing == &dir) {
        dirs.push(dir);
    }
}

fn stable_system_path() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let mut dirs = Vec::new();
        if let Ok(root) = env::var("SystemRoot") {
            dirs.push(Path::new(&root).join("System32"));
            dirs.push(PathBuf::from(&root));
            dirs.push(Path::new(&root).join("System32").join("Wbem"));
        }
        dirs
    }
    #[cfg(target_os = "macos")]
    {
        [
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
            "/Library/Apple/usr/bin",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
            .into_iter()
            .map(PathBuf::from)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn shim_detection_distinguishes_mise_from_real_tool_symlinks() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let mise = directory.path().join("mise");
        let real = directory.path().join("real-tool");
        std::fs::write(&mise, b"mise").unwrap();
        std::fs::write(&real, b"tool").unwrap();
        let shim = directory.path().join("probe");
        let tool = directory.path().join("linked-tool");
        symlink(&mise, &shim).unwrap();
        symlink(&real, &tool).unwrap();

        assert!(is_mise_shim(&shim));
        assert!(!is_mise_shim(&mise));
        assert!(!is_mise_shim(&tool));
        assert!(!is_mise_shim(&directory.path().join("missing")));
    }

    #[test]
    fn action_path_omits_parent_path_from_mise_env() {
        let env = BTreeMap::from([
            ("CARGO_HOME".to_string(), "/cache/cargo".to_string()),
            ("PATH".to_string(), "/tmp/global/bin".to_string()),
        ]);
        let path = build_action_path(&[PathBuf::from("/tools/rust/bin/rustc")], &env).unwrap();
        let entries: Vec<_> = env::split_paths(&path).collect();
        assert!(entries.contains(&PathBuf::from("/tools/rust/bin")));
        assert!(entries.contains(&PathBuf::from("/cache/cargo/bin")));
        assert!(!entries.contains(&PathBuf::from("/tmp/global/bin")));
    }
}
