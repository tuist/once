//! Local content-addressed store and action-result cache.
//!
//! Phase 0 substrate: blobs are addressed by BLAKE3 digest, action results
//! are keyed by action digest. Filesystem-backed; no remote tier yet.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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

/// Local content-addressed store rooted at `.fabrik/`.
///
/// Layout:
/// - `cas/<aa>/<rest-of-hex>` — blob bodies, sharded by first byte.
/// - `actions/<aa>/<rest-of-hex>.json` — action results, same sharding.
#[derive(Debug, Clone)]
pub struct Cas {
    root: PathBuf,
}

impl Cas {
    /// Open or create a CAS rooted at `root`. Creates the directory tree.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let cas = Self { root };
        cas.ensure_dir(&cas.blobs_dir())?;
        cas.ensure_dir(&cas.actions_dir())?;
        Ok(cas)
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

    fn ensure_dir(&self, path: &Path) -> Result<()> {
        fs::create_dir_all(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })
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

    /// Store a blob; returns its digest. Idempotent — a second put of the
    /// same bytes is a near-noop.
    pub fn put_blob(&self, bytes: &[u8]) -> Result<Digest> {
        let digest = Digest::of_bytes(bytes);
        let path = self.blob_path(&digest);
        if path.exists() {
            return Ok(digest);
        }
        if let Some(parent) = path.parent() {
            self.ensure_dir(parent)?;
        }
        write_atomic(&path, bytes)?;
        Ok(digest)
    }

    pub fn get_blob(&self, digest: &Digest) -> Result<Vec<u8>> {
        let path = self.blob_path(digest);
        match fs::read(&path) {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(Error::BlobNotFound(digest.clone()))
            }
            Err(source) => Err(Error::Io { path, source }),
        }
    }

    pub fn put_action_result(&self, action: &Digest, result: &ActionResult) -> Result<()> {
        let path = self.action_path(action);
        if let Some(parent) = path.parent() {
            self.ensure_dir(parent)?;
        }
        let bytes = serde_json::to_vec(result).expect("ActionResult is serializable");
        write_atomic(&path, &bytes)?;
        Ok(())
    }

    pub fn get_action_result(&self, action: &Digest) -> Result<Option<ActionResult>> {
        let path = self.action_path(action);
        match fs::read(&path) {
            Ok(bytes) => {
                let result = serde_json::from_slice(&bytes)
                    .map_err(|e| Error::Corrupt(path.clone(), e))?;
                Ok(Some(result))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(Error::Io { path, source }),
        }
    }

    pub fn stats(&self) -> Result<Stats> {
        let (blob_count, blob_bytes) = count_files(&self.blobs_dir())?;
        let (action_count, action_bytes) = count_files(&self.actions_dir())?;
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

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = parent.join(format!(
        ".tmp-{}",
        Digest::of_bytes(path.as_os_str().as_encoded_bytes()).to_hex()
    ));
    tmp.set_extension("part");
    fs::write(&tmp, bytes).map_err(|source| Error::Io {
        path: tmp.clone(),
        source,
    })?;
    fs::rename(&tmp, path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

fn count_files(root: &Path) -> Result<(u64, u64)> {
    let mut count = 0u64;
    let mut bytes = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => return Err(Error::Io { path: dir, source }),
        };
        for entry in entries {
            let entry = entry.map_err(|source| Error::Io {
                path: dir.clone(),
                source,
            })?;
            let ft = entry.file_type().map_err(|source| Error::Io {
                path: entry.path(),
                source,
            })?;
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                count += 1;
                bytes += entry
                    .metadata()
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
    use tempfile::TempDir;

    #[test]
    fn put_get_blob_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path()).unwrap();
        let d = cas.put_blob(b"hello").unwrap();
        assert_eq!(cas.get_blob(&d).unwrap(), b"hello");
    }

    #[test]
    fn put_blob_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path()).unwrap();
        let d1 = cas.put_blob(b"hello").unwrap();
        let d2 = cas.put_blob(b"hello").unwrap();
        assert_eq!(d1, d2);
    }

    #[test]
    fn action_result_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path()).unwrap();
        let stdout = cas.put_blob(b"out").unwrap();
        let stderr = cas.put_blob(b"err").unwrap();
        let action = Digest::of_bytes(b"action-key");
        let result = ActionResult {
            exit_code: 0,
            stdout,
            stderr,
        };
        cas.put_action_result(&action, &result).unwrap();
        assert_eq!(cas.get_action_result(&action).unwrap(), Some(result));
    }

    #[test]
    fn missing_action_returns_none() {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path()).unwrap();
        let d = Digest::of_bytes(b"nope");
        assert_eq!(cas.get_action_result(&d).unwrap(), None);
    }
}
