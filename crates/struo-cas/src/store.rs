use std::io;
use std::path::{Path, PathBuf};
use tokio::fs::{self, File};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::filesystem::{
    ensure_dir, rename_into_place, write_durably, write_with_durable_contents,
};
use crate::integrity::{self, CorruptBlob};
use crate::{blob, ActionResult, Digest, Error, Result};

mod copy;
mod producer;
mod staged;
mod staging;
mod stream;
pub(crate) use staging::PendingBlob;

pub use staged::{StagedBlob, StagedBlobReader};

const STREAM_CHUNK: usize = 256 * 1024;

/// Local content-addressed store rooted at a workspace `.struo/` directory.
///
/// Blobs live under `cas/<aa>/<rest-of-hex>`, sharded by their BLAKE3 content
/// digest. Stored bodies may be raw bytes or zstd-wrapped representations.
/// Action results live under `actions/<aa>/<rest-of-hex>.json` and reference
/// output digests in the same store.
///
/// [`open`](Self::open) performs no I/O. Directories are created lazily on the
/// first write, so a read-only consumer never changes the store.
#[derive(Debug, Clone)]
pub struct Cas {
    root: PathBuf,
}

impl Cas {
    /// Borrow a CAS rooted at `root`. Does no I/O.
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Filesystem root this store reads and writes under.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn blobs_dir(&self) -> PathBuf {
        self.root.join("cas")
    }

    pub(crate) fn actions_dir(&self) -> PathBuf {
        self.root.join("actions")
    }

    pub(crate) fn scratch_dir(&self) -> PathBuf {
        self.root.join("scratch")
    }

    fn shard_path(base: &Path, digest: &Digest, suffix: &str) -> PathBuf {
        let hex = digest.to_hex();
        let (prefix, rest) = hex.split_at(2);
        base.join(prefix).join(format!("{rest}{suffix}"))
    }

    pub(crate) fn blob_path(&self, digest: &Digest) -> PathBuf {
        Self::shard_path(&self.blobs_dir(), digest, "")
    }

    pub(crate) async fn blob_size(&self, digest: &Digest) -> Result<u64> {
        let path = self.blob_path(digest);
        let mut file = match File::open(&path).await {
            Ok(file) => Ok(file),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(Error::BlobNotFound(*digest)),
            Err(source) => Err(Error::Io {
                path: path.clone(),
                source,
            }),
        }?;
        let metadata = file.metadata().await.map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        let mut header = vec![0_u8; blob::ZSTD_BLOB_HEADER_LEN];
        match file.read_exact(&mut header).await {
            Ok(_) => Ok(blob::raw_size_from_header(&header).unwrap_or(metadata.len())),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(metadata.len()),
            Err(source) => Err(Error::Io { path, source }),
        }
    }

    pub(crate) fn action_path(&self, digest: &Digest) -> PathBuf {
        Self::shard_path(&self.actions_dir(), digest, ".json")
    }

    /// Store a blob; returns its digest. Idempotent - putting the same
    /// bytes twice is safe even from concurrent writers.
    pub async fn put_blob(&self, bytes: &[u8]) -> Result<Digest> {
        let digest = Digest::of_bytes(bytes);
        let path = self.blob_path(&digest);
        if fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(digest);
        }
        let stored = blob::encode_bytes(bytes).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        write_durably(&path, stored.as_ref()).await?;
        Ok(digest)
    }

    pub(crate) async fn mirror_blob(&self, expected: &Digest, bytes: &[u8]) -> Result<Digest> {
        let digest = Digest::of_bytes(bytes);
        if digest != *expected {
            return Ok(digest);
        }
        let path = self.blob_path(&digest);
        if fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(digest);
        }
        let stored = blob::encode_bytes(bytes).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        write_with_durable_contents(&path, stored.as_ref()).await?;
        Ok(digest)
    }

    /// Stream `reader` into the CAS, returning the content's digest.
    ///
    /// Memory use while reading is bounded by `STREAM_CHUNK` regardless
    /// of the input size - this is the path subprocess stdout/stderr go
    /// through, so a multi-GB linker log doesn't OOM the executor. The
    /// stream is hashed, written and compressed as bytes arrive. On completion,
    /// the selected scratch representation is renamed into place or discarded
    /// if the blob already exists. On
    /// returned error or cancellation, temporary files are cleaned up.
    /// A process crash can leave scratch files that future cache invocations ignore.
    pub async fn put_stream<R: AsyncRead + Unpin>(&self, reader: R) -> Result<Digest> {
        let scratch = self.scratch_dir();
        ensure_dir(&scratch).await?;
        let (stored, digest, file) = stream::prepare(&scratch, reader).await?;
        let final_path = self.blob_path(&digest);
        if !fs::try_exists(&final_path).await.unwrap_or(false) {
            File::from_std(file)
                .sync_all()
                .await
                .map_err(|source| Error::Io {
                    path: stored.to_path_buf(),
                    source,
                })?;
            rename_into_place(&stored, &final_path).await?;
        }
        Ok(digest)
    }

    pub(crate) fn remote_window_limit_exceeded(error: &Error) -> bool {
        stream::window_limit_exceeded(error)
    }

    pub(crate) async fn mirror_remote_verified<R: AsyncRead + Unpin, G: Send + 'static>(
        &self,
        reader: R,
        expected: u64,
        compressed: bool,
        expected_digest: Option<Digest>,
        sha256: Option<String>,
        guard: G,
    ) -> Result<Digest> {
        let scratch = self.scratch_dir();
        ensure_dir(&scratch).await?;
        let (stored, digest) =
            stream::ingest_remote_verified(&scratch, reader, expected, compressed, sha256, guard)
                .await?;
        if let Some(expected) = expected_digest {
            integrity::verify(&expected, &digest).map_err(|source| Error::Io {
                path: stored.to_path_buf(),
                source,
            })?;
        }
        let final_path = self.blob_path(&digest);
        if !fs::try_exists(&final_path).await.unwrap_or(false) {
            rename_into_place(&stored, &final_path).await?;
        }
        Ok(digest)
    }

    /// Read a content-addressed blob, verifying its contents before returning.
    pub async fn get_blob(&self, digest: &Digest) -> Result<Vec<u8>> {
        let path = self.blob_path(digest);
        let stored = match fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(Error::BlobNotFound(*digest))
            }
            Err(source) => return Err(Error::Io { path, source }),
        };
        let verified = blob::decode_bytes(stored)
            .map_err(|error| CorruptBlob::io_error(*digest, &error.to_string()))
            .and_then(|bytes| {
                integrity::verify(digest, &Digest::of_bytes(&bytes))?;
                Ok(bytes)
            });
        match verified {
            Ok(bytes) => Ok(bytes),
            Err(source) => {
                self.quarantine(digest, &path).await;
                Err(Error::Io { path, source })
            }
        }
    }

    pub(crate) async fn quarantine(&self, digest: &Digest, path: &Path) {
        tracing::warn!(blob_digest = %digest, "removing a local cache blob that failed its integrity check");
        if let Err(error) = fs::remove_file(path).await {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::warn!(blob_digest = %digest, %error, "could not remove corrupt blob");
            }
        }
    }

    /// True if a content-addressed blob exists at `digest`.
    pub async fn has_blob(&self, digest: &Digest) -> Result<bool> {
        let path = self.blob_path(digest);
        fs::try_exists(&path)
            .await
            .map_err(|source| Error::Io { path, source })
    }

    /// Record an action result durably: the write is flushed and its
    /// parent directory synchronized before the call returns, so a crash
    /// cannot leave a half-written entry behind.
    pub async fn put_action_result(&self, action: &Digest, result: &ActionResult) -> Result<()> {
        let path = self.action_path(action);
        let bytes = serde_json::to_vec(result).expect("ActionResult is serializable");
        write_durably(&path, &bytes).await
    }

    pub(crate) async fn mirror_action_result(
        &self,
        action: &Digest,
        result: &ActionResult,
    ) -> Result<()> {
        let path = self.action_path(action);
        let bytes = serde_json::to_vec(result).expect("ActionResult is serializable");
        write_with_durable_contents(&path, &bytes).await
    }

    /// Look up a cached action result. `None` is a miss.
    pub async fn get_action_result(&self, action: &Digest) -> Result<Option<ActionResult>> {
        let path = self.action_path(action);
        match fs::read(&path).await {
            Ok(bytes) => {
                let result =
                    serde_json::from_slice(&bytes).map_err(|e| Error::Corrupt(path.clone(), e))?;
                Ok(Some(result))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(Error::Io { path, source }),
        }
    }

    /// Delete a single action result. Useful for `struo cache forget`.
    pub async fn forget_action(&self, action: &Digest) -> Result<bool> {
        let path = self.action_path(action);
        match fs::remove_file(&path).await {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(Error::Io { path, source }),
        }
    }
}

#[cfg(test)]
#[path = "store/compression_benchmarks.rs"]
mod compression_benchmarks;
#[cfg(test)]
#[path = "store/stream_publication_tests.rs"]
mod stream_publication_tests;
#[cfg(test)]
mod tests;
