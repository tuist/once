use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::remote::archive::file_error;
use crate::remote::archive::limits::check_entries;
use crate::remote::http::check_size;
use crate::remote::path::Directory;
use crate::{RemoteIoLimits, Result, WorkspacePath};

pub(super) struct InputPlan {
    pub(super) workspace: Directory,
    pub(super) entries: BTreeMap<PathBuf, PathBuf>,
    pub(super) declared_roots: Vec<PathBuf>,
    pub(super) archive_bytes: u64,
    metadata_bytes: u64,
}

impl InputPlan {
    pub(super) fn collect(
        workspace: &Path,
        inputs: &[WorkspacePath],
        provider: &'static str,
        limits: RemoteIoLimits,
    ) -> Result<Self> {
        let mut plan = Self {
            workspace: Directory::open(workspace)
                .map_err(|source| file_error("open_remote_input_root", workspace, source))?,
            entries: BTreeMap::new(),
            declared_roots: Vec::new(),
            archive_bytes: 1024,
            metadata_bytes: 0,
        };
        check_size(
            plan.archive_bytes,
            limits.max_archive_bytes,
            provider,
            "archive bytes",
        )?;
        for input in inputs {
            let relative = Path::new(input.as_str());
            plan.metadata_bytes = plan
                .metadata_bytes
                .saturating_add(relative.as_os_str().len() as u64)
                .saturating_add(64);
            check_size(
                plan.metadata_bytes,
                limits.max_metadata_bytes,
                provider,
                "archive metadata bytes",
            )?;
            plan.declared_roots.push(relative.to_path_buf());
            plan.collect_entry(&input.resolve(workspace), relative, provider, limits)?;
        }
        Ok(plan)
    }

    fn collect_entry(
        &mut self,
        source: &Path,
        relative: &Path,
        provider: &'static str,
        limits: RemoteIoLimits,
    ) -> Result<()> {
        if self.entries.contains_key(relative) {
            return Ok(());
        }
        let metadata = self
            .workspace
            .metadata(relative)
            .map_err(|source_error| file_error("read_remote_input", relative, source_error))?;
        check_entries(
            self.entries.len().saturating_add(1),
            limits.max_archive_entries,
            provider,
        )?;
        self.metadata_bytes = self
            .metadata_bytes
            .saturating_add(source.as_os_str().len() as u64)
            .saturating_add(relative.as_os_str().len() as u64)
            .saturating_add(256);
        check_size(
            self.metadata_bytes,
            limits.max_metadata_bytes,
            provider,
            "archive metadata bytes",
        )?;
        let link_bytes = if metadata.is_symlink() {
            self.workspace
                .read_link(relative)
                .map_err(|source_error| {
                    file_error("read_remote_input_link", relative, source_error)
                })?
                .as_os_str()
                .len() as u64
        } else {
            0
        };
        let data_bytes = if metadata.is_file() { metadata.len } else { 0 };
        if !relative.as_os_str().is_empty() {
            self.archive_bytes = self
                .archive_bytes
                .saturating_add(1536)
                .saturating_add(padded(data_bytes))
                .saturating_add(padded(
                    (relative.as_os_str().len() as u64).saturating_add(1),
                ))
                .saturating_add(padded(link_bytes.saturating_add(1)));
            check_size(
                self.archive_bytes,
                limits.max_archive_bytes,
                provider,
                "archive bytes",
            )?;
        }
        self.entries
            .insert(relative.to_path_buf(), source.to_path_buf());
        if !metadata.is_dir() {
            return Ok(());
        }
        let directory = self
            .workspace
            .descend(relative, false)
            .map_err(|source_error| {
                file_error("read_remote_input_directory", relative, source_error)
            })?;
        let children = directory.entries().map_err(|source_error| {
            file_error("read_remote_input_directory", relative, source_error)
        })?;
        for child in children {
            let child = child.map_err(|source_error| {
                file_error("read_remote_input_directory", relative, source_error)
            })?;
            self.collect_entry(
                &source.join(&child),
                &relative.join(&child),
                provider,
                limits,
            )?;
        }
        Ok(())
    }
}

fn padded(bytes: u64) -> u64 {
    bytes.div_ceil(512).saturating_mul(512)
}

#[cfg(all(test, any(target_os = "linux", target_vendor = "apple")))]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn rejects_parent_and_leaf_symlink_swaps_after_planning() {
        for swap_parent in [true, false] {
            let workspace = tempfile::tempdir().unwrap();
            let sibling = tempfile::tempdir().unwrap();
            std::fs::create_dir(workspace.path().join("input")).unwrap();
            std::fs::write(workspace.path().join("input/secret"), b"declared").unwrap();
            std::fs::write(sibling.path().join("secret"), b"coordinator only").unwrap();
            let plan = InputPlan::collect(
                workspace.path(),
                &[WorkspacePath::try_from("input/secret").unwrap()],
                "test",
                RemoteIoLimits::default(),
            )
            .unwrap();
            if swap_parent {
                std::fs::remove_dir_all(workspace.path().join("input")).unwrap();
                std::os::unix::fs::symlink(sibling.path(), workspace.path().join("input")).unwrap();
            } else {
                std::fs::remove_file(workspace.path().join("input/secret")).unwrap();
                std::os::unix::fs::symlink(
                    sibling.path().join("secret"),
                    workspace.path().join("input/secret"),
                )
                .unwrap();
            }
            assert!(super::super::build::archive(workspace.path(), plan, "test").is_err());
        }
    }

    #[test]
    fn tiny_inputs_reserve_their_planned_size_not_the_archive_cap() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("input"), b"small").unwrap();
        let plan = InputPlan::collect(
            workspace.path(),
            &[WorkspacePath::try_from("input").unwrap()],
            "test",
            RemoteIoLimits::default(),
        )
        .unwrap();
        assert!(plan.archive_bytes < 8192);
        assert!(plan.metadata_bytes < 1024);
    }

    #[test]
    fn changed_file_size_cannot_exceed_the_preparation_grant() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("input"), b"small").unwrap();
        let plan = InputPlan::collect(
            workspace.path(),
            &[WorkspacePath::try_from("input").unwrap()],
            "test",
            RemoteIoLimits::default(),
        )
        .unwrap();
        std::fs::write(workspace.path().join("input"), vec![0; 16384]).unwrap();
        let error = super::super::build::archive(workspace.path(), plan, "test")
            .err()
            .unwrap();
        assert!(matches!(error, crate::Error::ExecutorResourceLimit { .. }));
    }
}
