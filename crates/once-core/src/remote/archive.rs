use std::path::Path;

use tempfile::TempPath;

use crate::{Error, Result};

mod input;
mod output;

pub(super) use input::create_input_archive;
pub(super) use output::install_output_archive;

pub(super) struct TempArchive {
    path: TempPath,
    len: u64,
}

impl TempArchive {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn len(&self) -> u64 {
        self.len
    }
}

pub(super) fn output_archive_file(workspace_root: &Path) -> Result<tempfile::NamedTempFile> {
    let parent = workspace_root.join(".once/tmp");
    std::fs::create_dir_all(&parent).map_err(|source| Error::FileAction {
        action: "create_remote_archive_directory",
        path: ".once/tmp".to_string(),
        source,
    })?;
    tempfile::NamedTempFile::new_in(parent).map_err(|source| Error::FileAction {
        action: "create_remote_output_archive",
        path: ".once/tmp".to_string(),
        source,
    })
}

fn archive_error(provider: &'static str, message: String) -> Error {
    Error::RemoteProviderApi {
        provider: provider.to_string(),
        message,
    }
}

fn file_error(action: &'static str, path: &Path, source: std::io::Error) -> Error {
    Error::FileAction {
        action,
        path: path.display().to_string(),
        source,
    }
}
