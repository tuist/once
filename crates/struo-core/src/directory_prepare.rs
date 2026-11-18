use std::io::Read;
use std::path::Path;

use super::{stage, DIRECTORY_BLOB_MAGIC, DIRECTORY_BLOB_V1_MAGIC};
use crate::{Error, Result};

pub(crate) enum Preparation<R, G> {
    Unavailable(R),
    Ready(PreparedDirectory<R, G>),
}

pub(crate) struct PreparedDirectory<R, G> {
    tree: stage::Tree,
    input: R,
    _owner: G,
}

impl<R, G> PreparedDirectory<R, G> {
    pub(crate) fn publish(self, logical_path: &str, destination: &Path) -> Result<()> {
        self.tree.publish(logical_path, destination)
    }
}

pub(crate) async fn prepare<R: Read + Send + 'static, G: Send + 'static>(
    logical_path: &str,
    destination: &Path,
    input: R,
    owner: G,
) -> Result<Preparation<R, G>> {
    let Some(tree) = stage::Tree::begin(logical_path, destination)? else {
        return Ok(Preparation::Unavailable(input));
    };
    let path = logical_path.to_string();
    let error_path = path.clone();
    tokio::task::spawn_blocking(move || {
        let mut prepared = PreparedDirectory {
            tree,
            input,
            _owner: owner,
        };
        let mut magic = vec![0_u8; DIRECTORY_BLOB_MAGIC.len()];
        super::read_directory_bytes(
            &path,
            &mut prepared.input,
            &mut magic,
            "directory blob magic",
        )?;
        let version_one = magic == DIRECTORY_BLOB_V1_MAGIC;
        if !version_one && magic != DIRECTORY_BLOB_MAGIC {
            return Err(super::invalid_directory(
                &path,
                "missing directory blob magic",
            ));
        }
        while let Some(entry) = super::read_streamed_directory_entry(
            &path,
            prepared.tree.root(),
            version_one,
            &mut prepared.input,
        )? {
            prepared
                .tree
                .restore_entry(&path, &entry, &mut prepared.input)?;
        }
        Ok(Preparation::Ready(prepared))
    })
    .await
    .map_err(|source| Error::RestoreOutput {
        path: error_path,
        source: std::io::Error::other(source.to_string()),
    })?
}

#[cfg(test)]
#[path = "directory_prepare_tests.rs"]
mod tests;
