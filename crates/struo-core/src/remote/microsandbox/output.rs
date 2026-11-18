use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use microsandbox::sandbox::{FsEntryKind, SandboxFs};
use tokio::io::AsyncWriteExt;

use crate::{Error, Result, WorkspacePath};

use super::path::{guest_child, guest_path};
use crate::remote::output_install::OutputStaging;
use crate::remote::path::relative_link_stays_within;

pub(super) async fn retrieve_outputs(
    fs: &SandboxFs,
    workspace_root: &Path,
    guest_root: &str,
    outputs: &[WorkspacePath],
) -> Result<()> {
    let staging = OutputStaging::create(workspace_root)?;
    let declared_roots = outputs
        .iter()
        .map(|output| PathBuf::from(output.as_str()))
        .collect::<Vec<_>>();

    for output in outputs {
        let guest = guest_path(guest_root, output);
        let relative = PathBuf::from(output.as_str());
        retrieve_tree(fs, &guest, &relative, staging.files(), &declared_roots).await?;
    }
    staging.install(outputs, workspace_root).await
}

async fn retrieve_tree(
    fs: &SandboxFs,
    root_guest: &str,
    root_relative: &Path,
    staging_root: &Path,
    declared_roots: &[PathBuf],
) -> Result<()> {
    let mut queue = VecDeque::from([(root_guest.to_string(), root_relative.to_path_buf())]);
    let mut directory_modes = Vec::new();

    while let Some((guest, relative)) = queue.pop_front() {
        let metadata = fs
            .stat_with_follow(&guest, false)
            .await
            .map_err(|source| output_error(&relative, source.to_string()))?;
        let host = staging_root.join(&relative);
        match metadata.kind {
            FsEntryKind::File => {
                if let Some(parent) = host.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|source| {
                        host_error("create_remote_output_parent", &relative, source)
                    })?;
                }
                let mut reader = fs
                    .read_stream(&guest)
                    .await
                    .map_err(|source| output_error(&relative, source.to_string()))?;
                let mut writer = tokio::fs::File::create(&host)
                    .await
                    .map_err(|source| host_error("create_remote_output", &relative, source))?;
                while let Some(chunk) = reader
                    .recv()
                    .await
                    .map_err(|source| output_error(&relative, source.to_string()))?
                {
                    writer
                        .write_all(&chunk)
                        .await
                        .map_err(|source| host_error("write_remote_output", &relative, source))?;
                }
                writer
                    .flush()
                    .await
                    .map_err(|source| host_error("flush_remote_output", &relative, source))?;
                set_host_mode(&host, metadata.mode)?;
            }
            FsEntryKind::Directory => {
                tokio::fs::create_dir_all(&host).await.map_err(|source| {
                    host_error("create_remote_output_directory", &relative, source)
                })?;
                directory_modes.push((host, metadata.mode));
                let mut children = fs
                    .list(&guest)
                    .await
                    .map_err(|source| output_error(&relative, source.to_string()))?;
                children.sort_by(|left, right| left.path.cmp(&right.path));
                for child in children {
                    let name = immediate_child_name(&guest, &child.path).ok_or_else(|| {
                        output_error(
                            &relative,
                            format!("provider returned invalid child path `{}`", child.path),
                        )
                    })?;
                    queue.push_back((guest_child(&guest, name), relative.join(name)));
                }
            }
            FsEntryKind::Symlink => {
                let target = fs
                    .read_link(&guest)
                    .await
                    .map_err(|source| output_error(&relative, source.to_string()))?;
                let target_path = Path::new(&target);
                if !relative_link_stays_within(&relative, target_path, declared_roots) {
                    return Err(output_error(
                        &relative,
                        format!("symbolic link target `{target}` escapes declared output trees"),
                    ));
                }
                if let Some(parent) = host.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|source| {
                        host_error("create_remote_output_parent", &relative, source)
                    })?;
                }
                create_host_symlink(target_path, &host)
                    .map_err(|source| host_error("create_remote_output_link", &relative, source))?;
            }
            FsEntryKind::Other => {
                return Err(output_error(
                    &relative,
                    "provider returned an unsupported filesystem entry",
                ));
            }
        }
    }

    directory_modes.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    for (path, mode) in directory_modes {
        set_host_mode(&path, mode)?;
    }
    Ok(())
}

fn immediate_child_name<'a>(parent: &str, child: &'a str) -> Option<&'a str> {
    let child_path = Path::new(child);
    if child_path.parent()? != Path::new(parent) {
        return None;
    }
    child_path.file_name()?.to_str()
}

#[cfg(unix)]
fn set_host_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o7777))
        .map_err(|source| host_error("set_remote_output_mode", path, source))
}

#[cfg(unix)]
fn create_host_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

fn output_error(path: &Path, message: impl Into<String>) -> Error {
    Error::RemoteProviderApi {
        provider: "microsandbox".to_string(),
        message: format!(
            "cannot retrieve output `{}`: {}",
            path.display(),
            message.into()
        ),
    }
}

fn host_error(action: &'static str, path: &Path, source: std::io::Error) -> Error {
    Error::FileAction {
        action,
        path: path.display().to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_immediate_guest_children() {
        assert_eq!(
            immediate_child_name("/workspace/out", "/workspace/out/result.txt"),
            Some("result.txt")
        );
        assert_eq!(
            immediate_child_name("/workspace/out", "/workspace/other/result.txt"),
            None
        );
    }
}
