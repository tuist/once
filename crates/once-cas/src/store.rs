use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::Ordering;

use tokio::fs::{self, File};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use crate::filesystem::{
    ensure_dir, rename_into_place, write_atomically, write_durably, write_with_durable_contents,
    TMP_COUNTER,
};
use crate::{blob, ActionResult, Digest, Error, Result};

const STREAM_CHUNK: usize = 64 * 1024;

/// Local content-addressed store rooted at a workspace `.once/` directory.
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

    /// Write a blob's contents to `destination`, streaming rather than
    /// buffering, and creating parent directories as needed.
    pub async fn copy_blob_to_file(&self, digest: &Digest, destination: &Path) -> Result<()> {
        let stored_path = self.blob_path(digest);
        if !fs::try_exists(&stored_path).await.unwrap_or(false) {
            return Err(Error::BlobNotFound(*digest));
        }
        let output_path = destination.to_path_buf();
        let error_path = output_path.clone();
        tokio::task::spawn_blocking(move || blob::decode_file(&stored_path, &output_path))
            .await
            .map_err(|source| Error::Io {
                path: error_path.clone(),
                source: io::Error::other(source.to_string()),
            })?
            .map_err(|source| Error::Io {
                path: error_path,
                source,
            })
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
    /// stream is hashed and written to a scratch file in one pass; on
    /// completion the scratch file is optionally compressed, then
    /// renamed into place or discarded if the blob already exists. On
    /// returned error or cancellation, temporary files are cleaned up.
    /// A process crash can leave scratch files that future cache invocations ignore.
    pub async fn put_stream<R: AsyncRead + Unpin>(&self, reader: R) -> Result<Digest> {
        let (tmp, digest) = self.stream_to_tmp("stream", reader).await?;
        self.commit_stream(tmp, &digest).await?;
        Ok(digest)
    }

    async fn commit_stream(&self, tmp: tempfile::TempPath, digest: &Digest) -> Result<()> {
        let final_path = self.blob_path(digest);
        if fs::try_exists(&final_path).await.unwrap_or(false) {
            return Ok(());
        }

        let stored_tmp = self.prepare_blob_tmp(tmp).await?;
        if fs::try_exists(&final_path).await.unwrap_or(false) {
            return Ok(());
        }

        rename_into_place(&stored_tmp, &final_path).await
    }

    /// Stream `reader` into the scratch dir and compute the BLAKE3 of
    /// the bytes. The caller is responsible for renaming the returned
    /// tmp path into its final location.
    async fn stream_to_tmp<R: AsyncRead + Unpin>(
        &self,
        prefix: &str,
        reader: R,
    ) -> Result<(tempfile::TempPath, Digest)> {
        let scratch = self.scratch_dir();
        ensure_dir(&scratch).await?;
        let temporary = tempfile::Builder::new()
            .prefix(&format!("{prefix}-"))
            .make_in(&scratch, |path| {
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(path)
            })
            .map_err(|source| Error::Io {
                path: scratch,
                source,
            })?;
        let (file, temporary_path) = temporary.into_parts();
        let tmp = temporary_path.to_path_buf();
        let digest = self
            .write_stream_to(&tmp, File::from_std(file), reader)
            .await?;
        Ok((temporary_path, digest))
    }

    async fn write_stream_to<R: AsyncRead + Unpin>(
        &self,
        tmp: &Path,
        mut file: File,
        mut reader: R,
    ) -> Result<Digest> {
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; STREAM_CHUNK];
        loop {
            let n = reader.read(&mut buf).await.map_err(|source| Error::Io {
                path: tmp.to_path_buf(),
                source,
            })?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])
                .await
                .map_err(|source| Error::Io {
                    path: tmp.to_path_buf(),
                    source,
                })?;
        }
        file.sync_all().await.map_err(|source| Error::Io {
            path: tmp.to_path_buf(),
            source,
        })?;
        Ok(Digest::from_bytes(*hasher.finalize().as_bytes()))
    }

    async fn prepare_blob_tmp(&self, raw_tmp: tempfile::TempPath) -> Result<tempfile::TempPath> {
        let scratch = self.scratch_dir();
        ensure_dir(&scratch).await?;
        let pid = process::id();
        let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let encoded_tmp = scratch.join(format!("zstd-{pid}-{seq}"));
        let raw_path = raw_tmp.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let encoded_tmp =
                tempfile::TempPath::try_from_path(encoded_tmp).map_err(|source| Error::Io {
                    path: raw_tmp.to_path_buf(),
                    source,
                })?;
            let encoded =
                blob::encode_file(&raw_tmp, &encoded_tmp).map_err(|source| Error::Io {
                    path: raw_tmp.to_path_buf(),
                    source,
                })?;
            if encoded.should_store {
                Ok(encoded_tmp)
            } else {
                Ok(raw_tmp)
            }
        })
        .await
        .map_err(|source| Error::Io {
            path: raw_path,
            source: io::Error::other(source.to_string()),
        })?
    }

    /// Read a content-addressed blob.
    pub async fn get_blob(&self, digest: &Digest) -> Result<Vec<u8>> {
        let path = self.blob_path(digest);
        match fs::read(&path).await {
            Ok(bytes) => blob::decode_bytes(bytes).map_err(|source| Error::Io { path, source }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(Error::BlobNotFound(*digest)),
            Err(source) => Err(Error::Io { path, source }),
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
        write_atomically(&path, &bytes).await
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

    /// Delete a single action result. Useful for `once cache forget`.
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
mod tests;
