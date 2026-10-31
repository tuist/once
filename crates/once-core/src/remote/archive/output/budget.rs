use std::path::{Path, PathBuf};

use super::{archive_error, check_entries, Directory};
use crate::{RemoteIoLimits, Result};

// Validation and staging cleanup recurse through the extracted tree.
const MAX_OUTPUT_DEPTH: usize = 64;

pub(super) struct ExtractionBudget {
    entries: usize,
    metadata_bytes: u64,
    limits: RemoteIoLimits,
    provider: &'static str,
}

impl ExtractionBudget {
    pub(super) fn new(limits: RemoteIoLimits, provider: &'static str) -> Self {
        Self {
            entries: 0,
            metadata_bytes: 0,
            limits,
            provider,
        }
    }

    pub(super) fn charge(
        &mut self,
        root: &Directory,
        path: &Path,
        retain_mode: bool,
        link_target: Option<&Path>,
    ) -> Result<()> {
        let depth = path.components().count();
        check_entries(depth, self.limits.max_archive_entries, self.provider)?;
        crate::remote::http::check_size(
            depth as u64,
            MAX_OUTPUT_DEPTH as u64,
            self.provider,
            "output path depth",
        )?;
        let mut entries = self.entries;
        let mut metadata_bytes = self.metadata_bytes;
        let mut prefix = PathBuf::new();
        let mut missing = false;
        // The staged filesystem accounts for aliases without a second retained path index.
        for component in path.components() {
            prefix.push(component);
            if !missing {
                match root.metadata(&prefix) {
                    Ok(metadata) if prefix == path || metadata.is_dir() => continue,
                    Ok(_) => {
                        return Err(archive_error(
                            self.provider,
                            format!(
                                "remote output ancestor `{}` is not a directory",
                                prefix.display()
                            ),
                        ));
                    }
                    Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                        missing = true;
                    }
                    Err(source) => {
                        return Err(archive_error(self.provider, source.to_string()));
                    }
                }
            }
            entries = entries.saturating_add(1);
            check_entries(entries, self.limits.max_archive_entries, self.provider)?;
            metadata_bytes = metadata_bytes
                .saturating_add(prefix.as_os_str().len() as u64)
                .saturating_add(256);
        }
        if retain_mode {
            metadata_bytes = metadata_bytes
                .saturating_add(path.as_os_str().len() as u64)
                .saturating_add(64);
        }
        if let Some(target) = link_target {
            metadata_bytes = metadata_bytes.saturating_add(target.as_os_str().len() as u64);
        }
        crate::remote::http::check_size(
            metadata_bytes,
            self.limits.max_metadata_bytes,
            self.provider,
            "output metadata bytes",
        )?;
        self.entries = entries;
        self.metadata_bytes = metadata_bytes;
        Ok(())
    }
}
