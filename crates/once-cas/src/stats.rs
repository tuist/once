use std::io;
use std::path::Path;

use tokio::fs;

use crate::{Cas, Error, Result, Stats};

impl Cas {
    /// Count blobs and action records by walking the store.
    pub async fn stats(&self) -> Result<Stats> {
        let blobs_dir = self.blobs_dir();
        let actions_dir = self.actions_dir();
        let (blobs, actions) =
            tokio::try_join!(count_files(&blobs_dir), count_files(&actions_dir))?;
        Ok(Stats {
            blob_count: blobs.0,
            blob_bytes: blobs.1,
            action_count: actions.0,
            action_bytes: actions.1,
        })
    }
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
    use crate::{ActionResult, Cas, Digest};
    use tempfile::TempDir;
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

    #[tokio::test]
    async fn stats_counts_blobs_and_actions() {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path());
        cas.put_blob(b"abc").await.unwrap();
        cas.put_blob(b"defg").await.unwrap();
        let key = Digest::of_bytes(b"k");
        let stdout = cas.put_blob(b"out").await.unwrap();
        cas.put_action_result(
            &key,
            &ActionResult {
                exit_code: 0,
                stdout: Some(stdout),
                stderr: Some(stdout),
                outputs: std::collections::BTreeMap::new(),
            },
        )
        .await
        .unwrap();
        let s = cas.stats().await.unwrap();
        assert_eq!(s.blob_count, 3);
        assert_eq!(s.blob_bytes, 3 + 4 + 3);
        assert_eq!(s.action_count, 1);
        assert!(s.action_bytes > 0);
    }

    #[tokio::test]
    async fn stats_on_empty_cas_returns_zeros() {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path());
        let s = cas.stats().await.unwrap();
        assert_eq!(s.blob_count, 0);
        assert_eq!(s.blob_bytes, 0);
        assert_eq!(s.action_count, 0);
        assert_eq!(s.action_bytes, 0);
    }
}
