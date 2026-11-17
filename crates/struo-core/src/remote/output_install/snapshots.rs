//! Remote snapshots are captured before publication and restored through the
//! same checked staging path, never recaptured from coordinator files.

use std::collections::BTreeMap;
use std::io::{Seek, SeekFrom};
use std::path::Path;

use struo_cas::{CacheProvider, Digest};

use super::{host_error, OutputStaging};
use crate::remote::path::validate_output_tree;
use crate::{outputs, RemoteIoLimits, Result, WorkspacePath};

impl OutputStaging {
    pub(in crate::remote) async fn capture(
        &mut self,
        roots: &[WorkspacePath],
        outputs: &[WorkspacePath],
        cache: &CacheProvider,
    ) -> Result<BTreeMap<String, Digest>> {
        validate_output_tree(
            &self.files_dir,
            roots,
            self.limits,
            0,
            Some(&self.workspace_dir),
        )?;
        let captured = outputs::capture_remote(outputs, self.files(), cache).await?;
        self.captured = true;
        Ok(captured)
    }
}

pub(crate) async fn restore(
    snapshots: Vec<outputs::PrefetchedOutput>,
    workspace: &Path,
    lease: Option<crate::ExecutionPermit>,
) -> Result<()> {
    let workspace = workspace.to_path_buf();
    let limits = lease
        .as_ref()
        .map_or_else(RemoteIoLimits::default, crate::ExecutionPermit::io_limits);
    let mut readers = Vec::new();
    let mut bytes = 0_u64;
    for snapshot in snapshots {
        let mut reader = snapshot.blob.into_reader()?;
        let size = reader.seek(SeekFrom::End(0)).map_err(|source| {
            host_error("size_remote_snapshot", Path::new(&snapshot.rel), source)
        })?;
        reader.rewind().map_err(|source| {
            host_error("rewind_remote_snapshot", Path::new(&snapshot.rel), source)
        })?;
        bytes = bytes.saturating_add(size);
        crate::remote::http::check_size(
            bytes,
            limits.max_archive_bytes,
            "remote",
            "output snapshot bytes",
        )?;
        readers.push((snapshot.rel, reader));
    }
    let phase = crate::remote::phase_permit(lease.as_ref(), bytes.saturating_mul(2)).await?;
    // Keep the materialization budget alive through physical restoration.
    let owner = crate::resource_scope::current();
    tokio::task::spawn_blocking(move || {
        let _owner = owner;
        let _phase = phase;
        let staging = OutputStaging::create_limited(&workspace, limits)?;
        let mut roots: Vec<WorkspacePath> = Vec::new();
        // Prefetch retains the BTreeMap's parent-before-descendant ordering.
        // Restore descendants too: a later action may have replaced their bytes.
        for (rel, reader) in readers {
            let output = WorkspacePath::try_from(rel.as_str())?;
            staging
                .files_dir
                .parent(Path::new(output.as_str()), true)
                .and_then(|destination| destination.remove())
                .map_err(|source| {
                    host_error("stage_remote_snapshot", Path::new(output.as_str()), source)
                })?;
            outputs::restore_prefetched_output(
                output.as_str(),
                &staging.files.join(output.as_str()),
                reader,
                true,
            )?;
            if !roots
                .iter()
                .any(|root| Path::new(output.as_str()).starts_with(root.as_str()))
            {
                roots.push(output);
            }
        }
        super::lock::with_output_lock(&staging.workspace_dir, || {
            super::publish::install_outputs(
                &staging.files_dir,
                &staging.backup_dir,
                &staging.workspace_dir,
                &roots,
                staging.limits,
                &staging.directory_modes,
                true,
            )
        })
    })
    .await
    .map_err(|source| {
        host_error(
            "restore_remote_snapshot",
            Path::new(""),
            std::io::Error::other(source.to_string()),
        )
    })?
}
