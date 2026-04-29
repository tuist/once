//! Local content-addressed store and action-result cache.
//!
//! Blobs are addressed by their BLAKE3 digest; action results are keyed
//! by an action digest supplied by the caller. Filesystem-backed via
//! `tokio::fs`, no remote tier.
//!
//! Durability: writes go via a uniquely-named tmp file, are fsynced
//! before rename, and the parent directory is fsynced after rename.
//! This is enough to survive an OS crash on common journaled
//! filesystems; for stricter expectations the caller should swap the
//! substrate for a REAPI-backed store.

use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tokio::fs::{self, File};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

mod digest;

pub use digest::Digest;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("corrupt action result at {0}: {1}")]
    Corrupt(PathBuf, serde_json::Error),
    #[error("blob not found: {0}")]
    BlobNotFound(Digest),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Cached result of a single action execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionResult {
    pub exit_code: i32,
    pub stdout: Digest,
    pub stderr: Digest,
}

/// Local content-addressed store rooted at a workspace `.fabrik/`
/// directory.
///
/// Layout:
/// - `cas/<aa>/<rest-of-hex>` — blob bodies, sharded by first byte.
/// - `actions/<aa>/<rest-of-hex>.json` — action results, same sharding.
///
/// `open` is cheap and side-effect-free; the directory tree is created
/// lazily on the first write. A read-only consumer never touches disk
/// outside its own reads.
#[derive(Debug, Clone)]
pub struct Cas {
    root: PathBuf,
}

impl Cas {
    /// Borrow a CAS rooted at `root`. Does no I/O.
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn blobs_dir(&self) -> PathBuf {
        self.root.join("cas")
    }

    fn actions_dir(&self) -> PathBuf {
        self.root.join("actions")
    }

    fn scratch_dir(&self) -> PathBuf {
        self.root.join("scratch")
    }

    fn shard_path(base: &Path, digest: &Digest, suffix: &str) -> PathBuf {
        let hex = digest.to_hex();
        let (prefix, rest) = hex.split_at(2);
        base.join(prefix).join(format!("{rest}{suffix}"))
    }

    fn blob_path(&self, digest: &Digest) -> PathBuf {
        Self::shard_path(&self.blobs_dir(), digest, "")
    }

    fn action_path(&self, digest: &Digest) -> PathBuf {
        Self::shard_path(&self.actions_dir(), digest, ".json")
    }

    /// Store a blob; returns its digest. Idempotent — putting the same
    /// bytes twice is safe even from concurrent writers.
    pub async fn put_blob(&self, bytes: &[u8]) -> Result<Digest> {
        let digest = Digest::of_bytes(bytes);
        let path = self.blob_path(&digest);
        if fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(digest);
        }
        write_durably(&path, bytes).await?;
        Ok(digest)
    }

    /// Stream `reader` into the CAS, returning the content's digest.
    ///
    /// Memory use is bounded by `STREAM_CHUNK` regardless of the input
    /// size — this is the path subprocess stdout/stderr go through, so
    /// a multi-GB linker log doesn't OOM the executor. The stream is
    /// hashed and written to a scratch file in one pass; on completion
    /// the scratch file is renamed into place (or discarded if the
    /// blob already exists).
    pub async fn put_stream<R: AsyncRead + Unpin>(&self, mut reader: R) -> Result<Digest> {
        let scratch = self.scratch_dir();
        ensure_dir(&scratch).await?;
        let pid = process::id();
        let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = scratch.join(format!("stream-{pid}-{seq}"));

        let mut file = File::create(&tmp).await.map_err(|source| Error::Io {
            path: tmp.clone(),
            source,
        })?;
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; STREAM_CHUNK];
        loop {
            let n = reader.read(&mut buf).await.map_err(|source| Error::Io {
                path: tmp.clone(),
                source,
            })?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])
                .await
                .map_err(|source| Error::Io {
                    path: tmp.clone(),
                    source,
                })?;
        }
        file.sync_all().await.map_err(|source| Error::Io {
            path: tmp.clone(),
            source,
        })?;
        drop(file);

        let digest = Digest::from_bytes(*hasher.finalize().as_bytes());
        let final_path = self.blob_path(&digest);

        if fs::try_exists(&final_path).await.unwrap_or(false) {
            // Another writer beat us, or this content is already cached.
            let _ = fs::remove_file(&tmp).await;
            return Ok(digest);
        }

        let parent = final_path.parent().expect("shard path has parent");
        ensure_dir(parent).await?;
        if let Err(source) = fs::rename(&tmp, &final_path).await {
            let _ = fs::remove_file(&tmp).await;
            return Err(Error::Io {
                path: final_path,
                source,
            });
        }
        fsync_dir(parent).await?;
        Ok(digest)
    }

    pub async fn get_blob(&self, digest: &Digest) -> Result<Vec<u8>> {
        let path = self.blob_path(digest);
        match fs::read(&path).await {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(Error::BlobNotFound(*digest)),
            Err(source) => Err(Error::Io { path, source }),
        }
    }

    pub async fn put_action_result(&self, action: &Digest, result: &ActionResult) -> Result<()> {
        let path = self.action_path(action);
        let bytes = serde_json::to_vec(result).expect("ActionResult is serializable");
        write_durably(&path, &bytes).await
    }

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

    /// Delete a single action result. Useful for `fabrik cache forget`.
    pub async fn forget_action(&self, action: &Digest) -> Result<bool> {
        let path = self.action_path(action);
        match fs::remove_file(&path).await {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(Error::Io { path, source }),
        }
    }

    pub async fn stats(&self) -> Result<Stats> {
        let (blob_count, blob_bytes) = count_files(&self.blobs_dir()).await?;
        let (action_count, action_bytes) = count_files(&self.actions_dir()).await?;
        Ok(Stats {
            blob_count,
            blob_bytes,
            action_count,
            action_bytes,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Stats {
    pub blob_count: u64,
    pub blob_bytes: u64,
    pub action_count: u64,
    pub action_bytes: u64,
}

/// Buffer size for [`Cas::put_stream`]. Bounds per-stream memory use.
const STREAM_CHUNK: usize = 64 * 1024;

/// Process-wide tmp-file counter so concurrent writers within one
/// process never collide.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Atomically write `bytes` to `path` with crash-survivable durability:
/// fsync the temp file, rename, fsync the parent dir.
///
/// Tmp filename includes the PID and a per-process counter so that two
/// concurrent writers of the same target never collide on the temp
/// path.
async fn write_durably(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    ensure_dir(parent).await?;

    let pid = process::id();
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let basename = path
        .file_name()
        .map_or_else(|| std::borrow::Cow::Borrowed(""), |n| n.to_string_lossy());
    let tmp = parent.join(format!(".tmp-{basename}-{pid}-{seq}"));

    write_and_sync(&tmp, bytes).await?;

    if let Err(source) = fs::rename(&tmp, path).await {
        // Best-effort cleanup; failure here doesn't change the caller's
        // error.
        let _ = fs::remove_file(&tmp).await;
        return Err(Error::Io {
            path: path.to_path_buf(),
            source,
        });
    }
    fsync_dir(parent).await
}

async fn write_and_sync(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = File::create(path).await.map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    f.write_all(bytes).await.map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    f.sync_all().await.map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

#[cfg(unix)]
async fn fsync_dir(dir: &Path) -> Result<()> {
    let f = File::open(dir).await.map_err(|source| Error::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    f.sync_all().await.map_err(|source| Error::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    Ok(())
}

// Windows can't fsync a directory; the rename is the durable point.
#[cfg(not(unix))]
async fn fsync_dir(_: &Path) -> Result<()> {
    Ok(())
}

async fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).await.map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

async fn count_files(root: &Path) -> Result<(u64, u64)> {
    let mut count = 0u64;
    let mut bytes = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries = match fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => return Err(Error::Io { path: dir, source }),
        };
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(source) => {
                    return Err(Error::Io {
                        path: dir.clone(),
                        source,
                    })
                }
            };
            let ft = entry.file_type().await.map_err(|source| Error::Io {
                path: entry.path(),
                source,
            })?;
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                // Skip half-written tmp files that crashed before rename.
                let name = entry.file_name();
                if name.to_string_lossy().starts_with(".tmp-") {
                    continue;
                }
                count += 1;
                bytes += entry
                    .metadata()
                    .await
                    .map_err(|source| Error::Io {
                        path: entry.path(),
                        source,
                    })?
                    .len();
            }
        }
    }
    Ok((count, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[test]
    fn open_does_no_io() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("not/yet/created");
        let _cas = Cas::open(&nested);
        assert!(!nested.exists(), "open must not touch disk");
    }

    #[tokio::test]
    async fn put_get_blob_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path());
        let d = cas.put_blob(b"hello").await.unwrap();
        assert_eq!(cas.get_blob(&d).await.unwrap(), b"hello");
    }

    #[tokio::test]
    async fn put_blob_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path());
        let d1 = cas.put_blob(b"hello").await.unwrap();
        let d2 = cas.put_blob(b"hello").await.unwrap();
        assert_eq!(d1, d2);
    }

    #[tokio::test]
    async fn concurrent_writers_of_identical_blob_do_not_race() {
        let tmp = TempDir::new().unwrap();
        let cas = Arc::new(Cas::open(tmp.path()));
        let mut handles = Vec::new();
        for _ in 0..16 {
            let cas = Arc::clone(&cas);
            handles.push(tokio::spawn(async move {
                cas.put_blob(b"shared content").await
            }));
        }
        let mut digests = Vec::new();
        for h in handles {
            digests.push(h.await.unwrap().unwrap());
        }
        assert!(digests.windows(2).all(|w| w[0] == w[1]));
        assert_eq!(cas.get_blob(&digests[0]).await.unwrap(), b"shared content");
    }

    #[tokio::test]
    async fn action_result_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path());
        let stdout = cas.put_blob(b"out").await.unwrap();
        let stderr = cas.put_blob(b"err").await.unwrap();
        let action = Digest::of_bytes(b"action-key");
        let result = ActionResult {
            exit_code: 0,
            stdout,
            stderr,
        };
        cas.put_action_result(&action, &result).await.unwrap();
        assert_eq!(cas.get_action_result(&action).await.unwrap(), Some(result));
    }

    #[tokio::test]
    async fn missing_action_returns_none() {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path());
        let d = Digest::of_bytes(b"nope");
        assert_eq!(cas.get_action_result(&d).await.unwrap(), None);
    }

    #[tokio::test]
    async fn forget_action_removes_only_the_target() {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path());
        let stdout = cas.put_blob(b"x").await.unwrap();
        let key = Digest::of_bytes(b"k");
        let result = ActionResult {
            exit_code: 0,
            stdout,
            stderr: stdout,
        };
        cas.put_action_result(&key, &result).await.unwrap();
        assert!(cas.forget_action(&key).await.unwrap());
        assert_eq!(cas.get_action_result(&key).await.unwrap(), None);
        // Blob is untouched — multiple actions may share a stdout blob.
        assert_eq!(cas.get_blob(&stdout).await.unwrap(), b"x");
        // Forgetting again is a no-op.
        assert!(!cas.forget_action(&key).await.unwrap());
    }

    #[tokio::test]
    async fn stats_ignores_orphaned_tmp_files() {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path());
        cas.put_blob(b"real").await.unwrap();
        // Simulate a crashed write.
        let orphan = cas.blobs_dir().join("zz").join(".tmp-leftover-1234-5");
        fs::create_dir_all(orphan.parent().unwrap()).await.unwrap();
        fs::write(&orphan, b"junk").await.unwrap();
        let s = cas.stats().await.unwrap();
        assert_eq!(s.blob_count, 1);
    }
}
