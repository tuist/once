use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use fabrik_frontend::DependencyEntry;

use super::report::ecosystem_name;

pub(in crate::commands::deps) fn entry_path(
    workspace: &Path,
    entry: &DependencyEntry,
    path: &str,
) -> PathBuf {
    workspace.join(&entry.package).join(path)
}

pub(in crate::commands::deps) fn graph_output_path(
    workspace: &Path,
    entry: &DependencyEntry,
    default_name: &str,
) -> PathBuf {
    entry.output.as_ref().map_or_else(
        || {
            workspace
                .join("vendor")
                .join(&entry.name)
                .join(default_name)
        },
        |output| entry_path(workspace, entry, output),
    )
}

pub(in crate::commands::deps) fn required_lockfile(
    workspace: &Path,
    entry: &DependencyEntry,
) -> Result<PathBuf> {
    let lockfile = entry.lockfile.as_deref().ok_or_else(|| {
        anyhow!(
            "dependency entry `{}` for {} must declare `lockfile`",
            entry.name,
            ecosystem_name(entry.ecosystem)
        )
    })?;
    Ok(entry_path(workspace, entry, lockfile))
}
