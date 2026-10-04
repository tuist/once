use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tokio::fs;

use crate::{ActionResult, Cas, Digest, Error, Result};

impl Cas {
    /// Reclaim disk space until the store fits within `max_bytes`.
    ///
    /// Eviction is reference-aware and runs in priority order:
    /// 1. Orphan blobs - content no surviving action result points at,
    ///    oldest first. These are pure waste, so they always go first.
    /// 2. Action results, oldest first. Evicting an action releases the
    ///    blobs that only it referenced, which then become collectable.
    ///
    /// A blob is never removed while a surviving action result still
    /// references it, so a pass never manufactures a dangling cache
    /// entry (which would force an otherwise-avoidable re-execution). A
    /// large, stable input shared by a recent action therefore stays
    /// cached even when it is the oldest thing on disk - the opposite of
    /// a plain oldest-first sweep, which would evict exactly those hot
    /// toolchain blobs first.
    ///
    /// Recency is a file's modification time. Because content-addressed
    /// blobs are immutable, a blob's own mtime is its first-seen time,
    /// which is why blobs are ranked by reachability rather than their
    /// own age; action-result files carry the recency signal.
    ///
    /// With `dry_run`, nothing is deleted and the report describes what a
    /// real pass would reclaim.
    ///
    /// Safe to run against a store an in-flight build shares: a racing
    /// writer may push the store back over budget, and the next lookup of
    /// a just-evicted entry re-executes (restore stages every blob before
    /// touching the workspace, so nothing is left partially applied).
    pub async fn gc(&self, max_bytes: u64, dry_run: bool) -> Result<GcReport> {
        let blobs_dir = self.blobs_dir();
        let actions_dir = self.actions_dir();
        let (blobs, actions) = tokio::try_join!(
            collect_blob_files(&blobs_dir),
            collect_action_files(&actions_dir)
        )?;

        let bytes_before: u64 =
            blobs.iter().map(|b| b.size).sum::<u64>() + actions.iter().map(|a| a.size).sum::<u64>();
        if bytes_before <= max_bytes {
            return Ok(GcReport {
                bytes_before,
                bytes_after: bytes_before,
                removed: 0,
                dry_run,
            });
        }

        // How many surviving action results reference each blob digest.
        let mut refcount: HashMap<Digest, usize> = HashMap::new();
        for action in &actions {
            for digest in &action.refs {
                *refcount.entry(*digest).or_insert(0) += 1;
            }
        }
        let blob_index: HashMap<Digest, usize> = blobs
            .iter()
            .enumerate()
            .filter_map(|(i, b)| b.digest.map(|d| (d, i)))
            .collect();

        let mut blob_removed = vec![false; blobs.len()];
        let mut action_removed = vec![false; actions.len()];
        let mut bytes_after = bytes_before;

        // Tier 1: orphan blobs, oldest first. A file that is not a valid
        // content address (a stray) references nothing, so it counts as
        // an orphan too.
        let mut orphans: Vec<usize> = blobs
            .iter()
            .enumerate()
            .filter(|(_, b)| match b.digest {
                Some(d) => refcount.get(&d).copied().unwrap_or(0) == 0,
                None => true,
            })
            .map(|(i, _)| i)
            .collect();
        orphans.sort_by_key(|&i| blobs[i].modified);
        for i in orphans {
            if bytes_after <= max_bytes {
                break;
            }
            blob_removed[i] = true;
            bytes_after = bytes_after.saturating_sub(blobs[i].size);
        }

        // Tier 2: action results, oldest first, cascading the blobs they
        // release once no surviving action references them.
        if bytes_after > max_bytes {
            let mut order: Vec<usize> = (0..actions.len()).collect();
            order.sort_by_key(|&i| actions[i].modified);
            for ai in order {
                if bytes_after <= max_bytes {
                    break;
                }
                action_removed[ai] = true;
                bytes_after = bytes_after.saturating_sub(actions[ai].size);
                for digest in &actions[ai].refs {
                    let Some(count) = refcount.get_mut(digest) else {
                        continue;
                    };
                    *count = count.saturating_sub(1);
                    // A blob whose last surviving action just went away is
                    // now unreachable garbage; collect it unconditionally
                    // rather than gating on the byte budget, so evicting an
                    // action never leaves its private blobs (including
                    // zero-byte ones) stranded.
                    if *count == 0 {
                        if let Some(&bi) = blob_index.get(digest) {
                            if !blob_removed[bi] {
                                blob_removed[bi] = true;
                                bytes_after = bytes_after.saturating_sub(blobs[bi].size);
                            }
                        }
                    }
                }
            }
        }

        // Remove owners first so interruption cannot leave an evicted owner's
        // result pointing at blobs already removed by this pass.
        let victims: Vec<PathBuf> = action_removed
            .iter()
            .enumerate()
            .filter(|(_, &removed)| removed)
            .map(|(i, _)| actions[i].path.clone())
            .chain(
                blob_removed
                    .iter()
                    .enumerate()
                    .filter(|(_, &removed)| removed)
                    .map(|(i, _)| blobs[i].path.clone()),
            )
            .collect();
        let removed = victims.len() as u64;

        if !dry_run {
            remove_files(victims).await?;
        }

        Ok(GcReport {
            bytes_before,
            bytes_after,
            removed,
            dry_run,
        })
    }
}

async fn remove_files(paths: Vec<PathBuf>) -> Result<()> {
    for path in paths {
        match fs::remove_file(&path).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(Error::Io { path, source }),
        }
    }
    Ok(())
}

/// Outcome of a [`Cas::gc`] pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcReport {
    /// Total on-disk size before collection.
    pub bytes_before: u64,
    /// Total on-disk size after collection (projected, when `dry_run`).
    pub bytes_after: u64,
    /// Number of blobs and action results removed (or that would be).
    pub removed: u64,
    /// Whether this pass only reported and left the store untouched.
    pub dry_run: bool,
}

/// A stored blob and the metadata `gc` ranks it by. `digest` is `None`
/// for a file whose path is not a valid content address.
struct BlobFile {
    path: PathBuf,
    digest: Option<Digest>,
    size: u64,
    modified: SystemTime,
}

/// A stored action result plus the blob digests it references (its
/// outputs, stdout, and stderr).
struct ActionFile {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
    refs: Vec<Digest>,
}

async fn collect_blob_files(root: &Path) -> Result<Vec<BlobFile>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries = match fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => return Err(Error::Io { path: dir, source }),
        };
        while let Some(entry) = entries.next_entry().await.map_err(|source| Error::Io {
            path: dir.clone(),
            source,
        })? {
            let ft = entry.file_type().await.map_err(|source| Error::Io {
                path: entry.path(),
                source,
            })?;
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                let name = entry.file_name();
                if name.to_string_lossy().starts_with(".tmp-") {
                    continue;
                }
                let path = entry.path();
                let metadata = entry.metadata().await.map_err(|source| Error::Io {
                    path: path.clone(),
                    source,
                })?;
                let digest = digest_from_blob_path(&path);
                out.push(BlobFile {
                    digest,
                    size: metadata.len(),
                    modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    path,
                });
            }
        }
    }
    Ok(out)
}

async fn collect_action_files(root: &Path) -> Result<Vec<ActionFile>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries = match fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => return Err(Error::Io { path: dir, source }),
        };
        while let Some(entry) = entries.next_entry().await.map_err(|source| Error::Io {
            path: dir.clone(),
            source,
        })? {
            let ft = entry.file_type().await.map_err(|source| Error::Io {
                path: entry.path(),
                source,
            })?;
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(".tmp-") || !name.ends_with(".json") {
                    continue;
                }
                let path = entry.path();
                let metadata = entry.metadata().await.map_err(|source| Error::Io {
                    path: path.clone(),
                    source,
                })?;
                let refs = match fs::read(&path).await {
                    Ok(bytes) => parse_action_refs(&bytes),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
                    Err(source) => return Err(Error::Io { path, source }),
                };
                out.push(ActionFile {
                    size: metadata.len(),
                    modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    refs,
                    path,
                });
            }
        }
    }
    Ok(out)
}

/// The blob digests an action result depends on. A corrupt or foreign
/// file references nothing, so it protects no blob from collection.
fn parse_action_refs(bytes: &[u8]) -> Vec<Digest> {
    let Ok(result) = serde_json::from_slice::<ActionResult>(bytes) else {
        return Vec::new();
    };
    let mut refs: Vec<Digest> = result.outputs.into_values().collect();
    refs.extend(result.stdout);
    refs.extend(result.stderr);
    refs
}

/// Reconstruct a blob digest from its sharded path (`.../aa/<rest>`).
/// Returns `None` for any path that is not a 64-hex content address.
fn digest_from_blob_path(path: &Path) -> Option<Digest> {
    let file = path.file_name()?.to_str()?;
    let shard = path.parent()?.file_name()?.to_str()?;
    if shard.len() != 2 {
        return None;
    }
    let mut hex = String::with_capacity(shard.len() + file.len());
    hex.push_str(shard);
    hex.push_str(file);
    Digest::from_hex(&hex)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    #[tokio::test]
    async fn gc_on_empty_cas_reports_nothing_removed() {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path());
        let report = cas.gc(0, false).await.unwrap();
        assert_eq!(report.bytes_before, 0);
        assert_eq!(report.bytes_after, 0);
        assert_eq!(report.removed, 0);
    }

    #[tokio::test]
    async fn gc_under_budget_removes_nothing() {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path());
        cas.put_blob(b"keep me").await.unwrap();
        let before = cas.stats().await.unwrap().blob_bytes;

        let report = cas.gc(u64::MAX, false).await.unwrap();

        assert_eq!(report.removed, 0);
        assert_eq!(report.bytes_before, before);
        assert_eq!(report.bytes_after, before);
        assert_eq!(cas.stats().await.unwrap().blob_count, 1);
    }

    #[tokio::test]
    async fn gc_evicts_oldest_orphan_blobs_until_within_budget() {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path());

        // Three distinct, incompressible orphan blobs (no action result
        // references them) written oldest-first. Explicit mtimes make the
        // ordering deterministic regardless of filesystem resolution.
        let payloads: [&[u8]; 3] = [b"first-blob-aaaa", b"second-blob-bbb", b"third-blob-cccc"];
        let mut digests = Vec::new();
        for (index, payload) in payloads.iter().enumerate() {
            let digest = cas.put_blob(payload).await.unwrap();
            let when = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000 + index as u64);
            filetime_set(&cas.blob_path(&digest), when);
            digests.push(digest);
        }

        let total = cas.stats().await.unwrap().blob_bytes;
        let per_blob = total / 3;

        let report = cas.gc(per_blob * 2 + 1, false).await.unwrap();

        assert_eq!(report.bytes_before, total);
        assert!(report.bytes_after <= per_blob * 2 + 1);
        assert_eq!(report.removed, 1);
        // The oldest blob is gone; the two newest survive.
        assert!(!cas.has_blob(&digests[0]).await.unwrap());
        assert!(cas.has_blob(&digests[1]).await.unwrap());
        assert!(cas.has_blob(&digests[2]).await.unwrap());
    }

    #[tokio::test]
    async fn gc_keeps_a_blob_a_surviving_action_references_even_if_oldest() {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path());

        // An OLD blob referenced by a recent action (think: a toolchain
        // blob), and a NEWER orphan blob referenced by nothing.
        let referenced = cas.put_blob(b"toolchain-blob-xxxxxxxx").await.unwrap();
        filetime_set(
            &cas.blob_path(&referenced),
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000),
        );
        let orphan = cas.put_blob(b"stale-output-blob-yyyyyy").await.unwrap();
        filetime_set(
            &cas.blob_path(&orphan),
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(5000),
        );

        let action = Digest::of_bytes(b"toolchain-consumer");
        cas.put_action_result(
            &action,
            &ActionResult {
                exit_code: 0,
                stdout: None,
                stderr: None,
                outputs: std::collections::BTreeMap::from([("out/bin".to_string(), referenced)]),
            },
        )
        .await
        .unwrap();

        let total = cas.stats().await.unwrap().blob_bytes + cas.stats().await.unwrap().action_bytes;
        // Budget forces roughly one blob's worth out. A naive oldest-first
        // sweep would evict the referenced (older) blob; reference-aware
        // GC must evict the orphan instead.
        let per_blob = cas.blob_size(&orphan).await.unwrap();
        cas.gc(total - per_blob, false).await.unwrap();

        assert!(
            cas.has_blob(&referenced).await.unwrap(),
            "a blob a surviving action references must be kept"
        );
        assert!(
            !cas.has_blob(&orphan).await.unwrap(),
            "the unreferenced orphan should be evicted first"
        );
    }

    #[tokio::test]
    async fn gc_cascades_a_blob_when_its_only_action_is_evicted() {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path());
        let blob = cas.put_blob(b"exclusively-owned-output").await.unwrap();
        // A zero-byte blob (an empty stderr capture) that only this action
        // references: it must be cascaded out even though removing it
        // reclaims no bytes.
        let empty = cas.put_blob(b"").await.unwrap();
        let action = Digest::of_bytes(b"sole-owner");
        cas.put_action_result(
            &action,
            &ActionResult {
                exit_code: 0,
                stdout: None,
                stderr: Some(empty),
                outputs: std::collections::BTreeMap::from([("out/x".to_string(), blob)]),
            },
        )
        .await
        .unwrap();

        // A zero budget evicts the action and, with it, every blob only
        // it referenced - leaving nothing stranded.
        let report = cas.gc(0, false).await.unwrap();

        assert_eq!(report.bytes_after, 0);
        assert!(!cas.has_blob(&blob).await.unwrap());
        assert!(!cas.has_blob(&empty).await.unwrap());
        assert_eq!(cas.get_action_result(&action).await.unwrap(), None);
        assert_eq!(cas.stats().await.unwrap().blob_count, 0);
    }

    #[tokio::test]
    async fn gc_dry_run_reports_without_deleting() {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path());
        let blob = cas.put_blob(b"still here after dry run").await.unwrap();

        let report = cas.gc(0, true).await.unwrap();

        assert!(report.dry_run);
        assert!(report.removed >= 1);
        assert_eq!(report.bytes_after, 0);
        // Nothing was actually deleted.
        assert!(cas.has_blob(&blob).await.unwrap());
        assert_eq!(cas.stats().await.unwrap().blob_count, 1);
    }

    #[tokio::test]
    async fn gc_ignores_orphaned_tmp_files() {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path());
        cas.put_blob(b"real").await.unwrap();
        let orphan = cas.blobs_dir().join("zz").join(".tmp-leftover-1234-5");
        fs::create_dir_all(orphan.parent().unwrap()).await.unwrap();
        fs::write(&orphan, b"junk").await.unwrap();

        // A zero budget removes the real blob but must leave the tmp
        // file alone - it is not durable cache state.
        cas.gc(0, false).await.unwrap();

        assert!(orphan.exists());
    }

    /// Set the modified time on `path`. Kept local to the test module so
    /// production code carries no timestamp-mutation API.
    fn filetime_set(path: &Path, when: SystemTime) {
        let file = std::fs::File::options()
            .write(true)
            .open(path)
            .expect("open blob to set mtime");
        file.set_modified(when).expect("set mtime");
    }
}
