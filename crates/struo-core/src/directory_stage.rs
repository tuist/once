use super::{restore_streamed_directory_symlink, DirectoryEntryKind, StreamedDirectoryEntry};
use crate::{Error, Result};
use std::io::{self, Read};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub(super) struct Tree {
    _container: tempfile::TempDir,
    root: PathBuf,
    directory: Option<std::fs::File>,
}

impl Tree {
    pub(super) fn begin(logical_path: &str, destination: &Path) -> Result<Option<Self>> {
        Self::begin_with(logical_path, destination, rename_new)
    }

    fn begin_with(
        logical_path: &str,
        destination: &Path,
        rename: impl Fn(&Path, &Path) -> rustix::io::Result<()>,
    ) -> Result<Option<Self>> {
        match std::fs::symlink_metadata(destination) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            _ => return Ok(None),
        }
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).map_err(|source| failed(logical_path, source))?;
        let container = tempfile::Builder::new()
            .prefix(".struo-tree-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(parent)
            .map_err(|source| failed(logical_path, source))?;
        let root = container.path().join("tree");
        std::fs::create_dir(&root).map_err(|source| failed(logical_path, source))?;
        let probe = container.path().join("rename-probe");
        match rename(&root, &probe) {
            Ok(()) => {
                rename(&probe, &root).map_err(|source| failed(logical_path, source.into()))?;
            }
            Err(
                rustix::io::Errno::NOSYS | rustix::io::Errno::NOTSUP | rustix::io::Errno::INVAL,
            ) => {
                tracing::debug!(
                    path = logical_path,
                    "private tree publication unsupported; using existing restoration"
                );
                return Ok(None);
            }
            Err(source) => return Err(failed(logical_path, source.into())),
        }
        tracing::debug!(
            path = logical_path,
            "materializing fresh directory snapshot privately"
        );
        Ok(Some(Self {
            _container: container,
            directory: std::fs::File::open(&root).ok(),
            root,
        }))
    }

    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    pub(super) fn restore_entry(
        &self,
        logical_path: &str,
        entry: &StreamedDirectoryEntry,
        reader: &mut impl Read,
    ) -> Result<()> {
        self.prepare_parent(&entry.destination)
            .map_err(|source| failed(logical_path, source))?;
        match entry.kind {
            DirectoryEntryKind::File => {
                let mut file = self
                    .create_file(entry)
                    .map_err(|source| failed(logical_path, source))?;
                crate::restore_file::write_private_content(
                    &mut file,
                    reader.take(entry.content_len),
                    entry.mode.max(0o400),
                    Some(entry.content_len),
                )
                .map_err(|source| {
                    if source.kind() == io::ErrorKind::UnexpectedEof {
                        super::invalid_directory(logical_path, "truncated entry content")
                    } else {
                        failed(logical_path, source)
                    }
                })
            }
            DirectoryEntryKind::SymlinkFile | DirectoryEntryKind::SymlinkDir => {
                restore_streamed_directory_symlink(logical_path, entry, reader)
            }
        }
    }

    fn create_file(&self, entry: &StreamedDirectoryEntry) -> io::Result<std::fs::File> {
        if let Some(directory) = self
            .directory
            .as_ref()
            .filter(|_| entry.destination.parent() == Some(self.root.as_path()))
        {
            let name = entry
                .destination
                .file_name()
                .ok_or_else(|| io::Error::other("missing directory entry filename"))?;
            let mode = entry.mode.max(0o400) & 0o7777;
            #[cfg(target_vendor = "apple")]
            let mode = u16::try_from(mode).map_err(io::Error::other)?;
            return rustix::fs::openat(
                directory,
                name,
                rustix::fs::OFlags::WRONLY
                    | rustix::fs::OFlags::CREATE
                    | rustix::fs::OFlags::EXCL
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::from_bits_truncate(mode),
            )
            .map(std::fs::File::from)
            .map_err(io::Error::from);
        }
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(entry.mode.max(0o400))
            .open(&entry.destination)
    }

    fn prepare_parent(&self, path: &Path) -> io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("missing directory entry parent"))?;
        let relative = parent.strip_prefix(&self.root).map_err(io::Error::other)?;
        let mut current = self.root.clone();
        for component in relative.components() {
            current.push(component);
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    std::fs::create_dir(&current)?;
                }
                Ok(_) => {
                    return Err(io::Error::other(
                        "directory entry parent is not a directory",
                    ))
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub(super) fn publish(self, logical_path: &str, destination: &Path) -> Result<()> {
        rename_new(&self.root, destination).map_err(|source| failed(logical_path, source.into()))
    }
}

fn rename_new(source: &Path, destination: &Path) -> rustix::io::Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        source,
        rustix::fs::CWD,
        destination,
        rustix::fs::RenameFlags::NOREPLACE,
    )
}

fn failed(path: &str, source: io::Error) -> Error {
    Error::RestoreOutput {
        path: path.to_string(),
        source,
    }
}

#[cfg(test)]
#[path = "directory_stage_tests.rs"]
mod tests;
