use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use microsandbox::sandbox::{FsSetAttrs, SandboxFs};

use crate::{Error, Result, WorkspacePath};

use super::microsandbox_error;
use super::path::guest_path;
use crate::remote::path::relative_link_stays_within;

enum HostEntry {
    Directory {
        relative: PathBuf,
        mode: u32,
    },
    File {
        source: PathBuf,
        relative: PathBuf,
        mode: u32,
    },
    Symlink {
        relative: PathBuf,
        target: PathBuf,
    },
}

pub(super) async fn stage_inputs(
    fs: &SandboxFs,
    workspace_root: &Path,
    guest_root: &str,
    inputs: &[WorkspacePath],
) -> Result<()> {
    let (entries, declared_roots) = plan_inputs(workspace_root, inputs)?;
    fs.mkdir(guest_root)
        .await
        .map_err(|source| microsandbox_error(&source))?;

    let mut directory_modes = Vec::new();
    for entry in entries.into_values() {
        match entry {
            HostEntry::Directory { relative, mode } => {
                let guest = guest_for_relative(guest_root, &relative)?;
                fs.mkdir(&guest)
                    .await
                    .map_err(|source| microsandbox_error(&source))?;
                directory_modes.push((guest, mode));
            }
            HostEntry::File {
                source,
                relative,
                mode,
            } => {
                let guest = guest_for_relative(guest_root, &relative)?;
                ensure_guest_parent(fs, &guest).await?;
                fs.copy_from_host(&source, &guest)
                    .await
                    .map_err(|source| microsandbox_error(&source))?;
                set_mode(fs, &guest, mode).await?;
            }
            HostEntry::Symlink { relative, target } => {
                if !relative_link_stays_within(&relative, &target, &declared_roots) {
                    return Err(transfer_error(
                        &relative,
                        format!(
                            "symbolic link target `{}` escapes the declared input trees",
                            target.display()
                        ),
                    ));
                }
                let guest = guest_for_relative(guest_root, &relative)?;
                ensure_guest_parent(fs, &guest).await?;
                let target = target.to_str().ok_or_else(|| {
                    transfer_error(&relative, "symbolic link target is not valid UTF-8")
                })?;
                fs.symlink(target, &guest)
                    .await
                    .map_err(|source| microsandbox_error(&source))?;
            }
        }
    }

    directory_modes.sort_by_key(|(path, _)| std::cmp::Reverse(path.matches('/').count()));
    for (path, mode) in directory_modes {
        set_mode(fs, &path, mode).await?;
    }
    Ok(())
}

fn plan_inputs(
    workspace_root: &Path,
    inputs: &[WorkspacePath],
) -> Result<(BTreeMap<PathBuf, HostEntry>, Vec<PathBuf>)> {
    let declared_roots = inputs
        .iter()
        .map(|input| PathBuf::from(input.as_str()))
        .collect::<Vec<_>>();
    let mut entries = BTreeMap::new();
    for input in inputs {
        let relative = PathBuf::from(input.as_str());
        collect_entry(&input.resolve(workspace_root), &relative, &mut entries)?;
    }
    Ok((entries, declared_roots))
}

fn collect_entry(
    source: &Path,
    relative: &Path,
    entries: &mut BTreeMap<PathBuf, HostEntry>,
) -> Result<()> {
    let metadata = std::fs::symlink_metadata(source).map_err(|source_error| Error::FileAction {
        action: "read_remote_input",
        path: relative.display().to_string(),
        source: source_error,
    })?;
    let mode = metadata_mode(&metadata);
    if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(source).map_err(|source_error| Error::FileAction {
            action: "read_remote_input_link",
            path: relative.display().to_string(),
            source: source_error,
        })?;
        entries.insert(
            relative.to_path_buf(),
            HostEntry::Symlink {
                relative: relative.to_path_buf(),
                target,
            },
        );
        return Ok(());
    }
    if metadata.is_file() {
        entries.insert(
            relative.to_path_buf(),
            HostEntry::File {
                source: source.to_path_buf(),
                relative: relative.to_path_buf(),
                mode,
            },
        );
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(transfer_error(
            relative,
            "only files, directories, and symbolic links can be remote inputs",
        ));
    }

    entries.insert(
        relative.to_path_buf(),
        HostEntry::Directory {
            relative: relative.to_path_buf(),
            mode,
        },
    );
    let mut children = std::fs::read_dir(source)
        .map_err(|source_error| Error::FileAction {
            action: "read_remote_input_directory",
            path: relative.display().to_string(),
            source: source_error,
        })?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|source_error| Error::FileAction {
            action: "read_remote_input_directory",
            path: relative.display().to_string(),
            source: source_error,
        })?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let child_relative = relative.join(child.file_name());
        collect_entry(&child.path(), &child_relative, entries)?;
    }
    Ok(())
}

async fn ensure_guest_parent(fs: &SandboxFs, path: &str) -> Result<()> {
    let parent = Path::new(path)
        .parent()
        .and_then(Path::to_str)
        .ok_or_else(|| transfer_error(Path::new(path), "input has no parent directory"))?;
    fs.mkdir(parent)
        .await
        .map_err(|source| microsandbox_error(&source))
}

async fn set_mode(fs: &SandboxFs, path: &str, mode: u32) -> Result<()> {
    fs.set_stat(
        path,
        false,
        FsSetAttrs {
            mode: Some(mode & 0o7777),
            ..FsSetAttrs::default()
        },
    )
    .await
    .map_err(|source| microsandbox_error(&source))
}

fn guest_for_relative(root: &str, relative: &Path) -> Result<String> {
    let relative = relative
        .to_str()
        .ok_or_else(|| transfer_error(relative, "input path is not valid UTF-8"))?;
    let path = WorkspacePath::try_from(relative)?;
    Ok(guest_path(root, &path))
}

#[cfg(unix)]
fn metadata_mode(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode()
}

fn transfer_error(path: &Path, message: impl Into<String>) -> Error {
    Error::RemoteProviderApi {
        provider: "microsandbox".to_string(),
        message: format!(
            "cannot stage input `{}`: {}",
            path.display(),
            message.into()
        ),
    }
}
