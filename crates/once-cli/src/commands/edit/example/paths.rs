use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};

pub(super) fn resolve_example_path(
    workspace: &Path,
    destination: &Path,
    relative: &str,
) -> Result<PathBuf> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::CurDir
                    | Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
    {
        anyhow::bail!("example file path must stay inside the destination");
    }
    let path = destination.join(relative);
    if !path.starts_with(workspace) {
        anyhow::bail!("example file path must stay inside the workspace");
    }
    Ok(path)
}

pub(super) fn unsafe_path_reason(workspace: &Path, path: &Path) -> Result<Option<String>> {
    let relative = path
        .strip_prefix(workspace)
        .context("example path is outside the workspace")?;
    let mut current = workspace.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Ok(Some("the path traverses a symbolic link".to_string()));
            }
            Ok(metadata) if current != path && !metadata.is_dir() => {
                return Ok(Some("a parent path is not a directory".to_string()));
            }
            Ok(metadata) if current == path && metadata.is_dir() => {
                return Ok(Some(
                    "an existing directory occupies the file path".to_string(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(None)
}

pub(super) fn normalize_relative_path(path: &str) -> String {
    Path::new(path)
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub(super) fn join_display_path(prefix: &str, path: &str) -> String {
    if prefix.is_empty() {
        path.to_string()
    } else {
        format!("{prefix}/{path}")
    }
}

pub(super) fn example_manifest_packages<'a>(
    destination: &str,
    paths: impl Iterator<Item = &'a String>,
) -> BTreeSet<String> {
    paths
        .filter_map(|path| {
            let path = Path::new(path);
            (path.file_name()?.to_str()? == once_frontend::TOML_BUILD_FILE_NAME)
                .then(|| path.parent().unwrap_or_else(|| Path::new("")))
        })
        .map(|path| normalize_relative_path(path.to_string_lossy().as_ref()))
        .map(|package| {
            if destination.is_empty() || package.starts_with(destination) {
                package
            } else {
                join_display_path(destination, &package)
            }
        })
        .collect()
}
