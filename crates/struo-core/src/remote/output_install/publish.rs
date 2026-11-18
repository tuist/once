use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tokio::sync::{Semaphore, SemaphorePermit};

use super::host_error;
use crate::remote::path::{Directory, Parent};
use crate::{Error, RemoteIoLimits, Result, WorkspacePath};

const MAX_PINNED_PARENTS: usize = 128;
static PINNED_PARENTS: Semaphore = Semaphore::const_new(MAX_PINNED_PARENTS);

struct Parents<'a> {
    roots: [&'a Directory; 3],
    directories: BTreeMap<(usize, PathBuf), (Directory, SemaphorePermit<'static>)>,
    metadata_bytes: u64,
    limits: RemoteIoLimits,
}

impl Parents<'_> {
    fn parent(&mut self, root: usize, path: &Path, create: bool) -> Result<Parent> {
        let key = (root, path.parent().unwrap_or(Path::new("")).to_path_buf());
        if !self.directories.contains_key(&key) {
            self.metadata_bytes = self
                .metadata_bytes
                .saturating_add(key.1.as_os_str().len() as u64)
                .saturating_add(256);
            crate::remote::http::check_size(
                self.metadata_bytes,
                self.limits.max_metadata_bytes,
                "remote",
                "output parent metadata bytes",
            )?;
            let permit =
                PINNED_PARENTS
                    .try_acquire()
                    .map_err(|_| Error::ExecutorResourceLimit {
                        executor: "remote".into(),
                        resource: "pinned output directories",
                        limit: MAX_PINNED_PARENTS as u64,
                    })?;
            let directory = self.roots[root]
                .descend(&key.1, create)
                .map_err(|source| host_error("open_remote_output_parent", path, source))?;
            self.directories.insert(key.clone(), (directory, permit));
        }
        self.directories[&key]
            .0
            .parent(Path::new(path.file_name().unwrap()), false)
            .map_err(|source| host_error("open_remote_output_parent", path, source))
    }
}

pub(super) fn install_outputs(
    files: &Directory,
    backup: &Directory,
    workspace: &Directory,
    outputs: &[WorkspacePath],
    limits: RemoteIoLimits,
    directory_modes: &BTreeMap<PathBuf, u32>,
) -> Result<()> {
    let mode_bytes = directory_modes.keys().fold(0_u64, |bytes, path| {
        bytes
            .saturating_add(path.as_os_str().len() as u64)
            .saturating_add(64)
    });
    crate::remote::path::validate_output_tree(files, outputs, limits, mode_bytes)?;
    for (path, mode) in directory_modes.iter().rev() {
        files
            .set_directory_mode(path, *mode)
            .map_err(|source| host_error("set_remote_output_permissions", path, source))?;
    }
    let mut parents = Parents {
        roots: [files, backup, workspace],
        directories: BTreeMap::new(),
        metadata_bytes: 0,
        limits,
    };
    let mut backed_up = Vec::new();
    let mut installed = Vec::new();
    for output in outputs {
        if let Err(error) = install_output(&mut parents, output, &mut backed_up, &mut installed) {
            rollback_outputs(&installed, &backed_up);
            return Err(error);
        }
    }
    Ok(())
}

fn install_output(
    parents: &mut Parents<'_>,
    output: &WorkspacePath,
    backed_up: &mut Vec<(Parent, Parent)>,
    installed: &mut Vec<(Parent, Parent)>,
) -> Result<()> {
    let path = Path::new(output.as_str());
    let destination = parents.parent(2, path, true)?;
    let saved = parents.parent(1, path, true)?;
    let source = parents.parent(0, path, false)?;
    match destination.metadata() {
        Ok(_) => {
            let original = destination.clone();
            destination
                .rename_to(&saved)
                .map_err(|source| host_error("backup_remote_output", path, source))?;
            backed_up.push((original, saved));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(host_error("backup_remote_output", path, source)),
    }
    source
        .rename_to(&destination)
        .map_err(|source| host_error("install_remote_output", path, source))?;
    installed.push((destination, source));
    Ok(())
}

fn rollback_outputs(installed: &[(Parent, Parent)], backed_up: &[(Parent, Parent)]) {
    for (destination, source) in installed.iter().rev() {
        let _ = destination.rename_to(source);
    }
    for (destination, saved) in backed_up.iter().rev() {
        let _ = saved.rename_to(destination);
    }
}
