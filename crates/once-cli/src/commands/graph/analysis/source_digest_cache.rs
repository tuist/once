use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::UNIX_EPOCH;

use once_cas::Digest;
use serde::{Deserialize, Serialize};

const SCHEMA: &str = "once.source-digests.v1";

#[derive(Clone)]
pub struct SourceDigestCache {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    entries: RwLock<BTreeMap<String, Entry>>,
    output_entries: RwLock<BTreeMap<String, Entry>>,
    observed_digests: RwLock<BTreeMap<String, Digest>>,
    dirty: AtomicBool,
}

#[derive(Clone, Deserialize, Serialize)]
struct Entry {
    fingerprint: Fingerprint,
    digest: Digest,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
struct Fingerprint {
    len: u64,
    modified_seconds: u64,
    modified_nanos: u32,
    file_type: u8,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanos: i64,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    mode: u32,
}

#[derive(Deserialize, Serialize)]
struct CacheFile {
    schema: String,
    entries: BTreeMap<String, Entry>,
    #[serde(default)]
    output_entries: BTreeMap<String, Entry>,
}

#[derive(Serialize)]
struct CacheFileRef<'a> {
    schema: &'static str,
    entries: &'a BTreeMap<String, Entry>,
    output_entries: &'a BTreeMap<String, Entry>,
}

impl SourceDigestCache {
    pub(super) fn open(workspace: &Path) -> Self {
        let path = workspace.join(".once").join("source-digests.json");
        let cache = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<CacheFile>(&bytes).ok())
            .filter(|cache| cache.schema == SCHEMA);
        let (entries, output_entries) = cache.map_or_else(
            || (BTreeMap::new(), BTreeMap::new()),
            |cache| (cache.entries, cache.output_entries),
        );
        Self {
            inner: Arc::new(Inner {
                path,
                entries: RwLock::new(entries),
                output_entries: RwLock::new(output_entries),
                observed_digests: RwLock::new(BTreeMap::new()),
                dirty: AtomicBool::new(false),
            }),
        }
    }

    pub(super) fn digest(&self, workspace: &Path, relative: &str) -> std::io::Result<Digest> {
        let metadata = std::fs::symlink_metadata(workspace.join(relative))?;
        if metadata.is_dir() {
            let digest = once_core::digest_source_path(workspace, relative)?;
            self.record_observation(relative, digest);
            return Ok(digest);
        }
        let fingerprint = Fingerprint::from_metadata(&metadata)?;
        if let Some(digest) = self
            .inner
            .entries
            .read()
            .expect("source digest cache lock poisoned")
            .get(relative)
            .filter(|entry| Fingerprint::can_reuse() && entry.fingerprint == fingerprint)
            .map(|entry| entry.digest)
        {
            self.record_observation(relative, digest);
            return Ok(digest);
        }
        let digest = once_core::digest_source_path(workspace, relative)?;
        self.inner
            .entries
            .write()
            .expect("source digest cache lock poisoned")
            .insert(
                relative.to_string(),
                Entry {
                    fingerprint,
                    digest,
                },
            );
        self.inner.dirty.store(true, Ordering::Relaxed);
        self.record_observation(relative, digest);
        Ok(digest)
    }

    pub(super) fn observed_digests_for(
        &self,
        changes: Option<&[String]>,
    ) -> BTreeMap<String, Digest> {
        let Some(changes) = changes else {
            return BTreeMap::new();
        };
        let observed = self
            .inner
            .observed_digests
            .read()
            .expect("source digest observation lock poisoned");
        changes
            .iter()
            .filter_map(|path| {
                observed
                    .get(path)
                    .copied()
                    .map(|digest| (path.clone(), digest))
            })
            .collect()
    }

    pub(super) fn changes_match(&self, workspace: &Path, changes: Option<&[String]>) -> bool {
        let Some(changes) = changes else {
            return false;
        };
        let observed = self
            .inner
            .observed_digests
            .read()
            .expect("source digest observation lock poisoned");
        changes.iter().all(|relative| {
            let path = workspace.join(relative);
            observed.get(relative).map_or_else(
                || !path.exists(),
                |expected| {
                    once_core::digest_source_path(workspace, relative)
                        .is_ok_and(|actual| actual == *expected)
                },
            )
        })
    }

    pub(super) async fn materialize_outputs(
        &self,
        result: &once_cas::ActionResult,
        workspace: &Path,
        cache: &once_cas::CacheProvider,
    ) -> once_core::Result<()> {
        let mut required = result.clone();
        required
            .outputs
            .retain(|relative, expected| !self.output_matches(workspace, relative, *expected));
        if !required.outputs.is_empty() {
            once_core::materialize_outputs(&required, workspace, cache).await?;
        }
        self.record_outputs(result, workspace);
        Ok(())
    }

    pub(super) fn record_outputs(&self, result: &once_cas::ActionResult, workspace: &Path) {
        for (relative, digest) in &result.outputs {
            self.record_output(workspace, relative, *digest);
        }
    }

    pub(super) fn output_matches(
        &self,
        workspace: &Path,
        relative: &str,
        expected: Digest,
    ) -> bool {
        let Ok(metadata) = std::fs::symlink_metadata(workspace.join(relative)) else {
            return false;
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return false;
        }
        let Ok(fingerprint) = Fingerprint::from_metadata(&metadata) else {
            return false;
        };
        self.inner
            .output_entries
            .read()
            .expect("output digest cache lock poisoned")
            .get(relative)
            .is_some_and(|entry| {
                Fingerprint::can_reuse()
                    && entry.digest == expected
                    && entry.fingerprint == fingerprint
            })
    }

    fn record_output(&self, workspace: &Path, relative: &str, digest: Digest) {
        let entry = std::fs::symlink_metadata(workspace.join(relative))
            .ok()
            .filter(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
            .and_then(|metadata| {
                Fingerprint::from_metadata(&metadata)
                    .ok()
                    .map(|fingerprint| Entry {
                        fingerprint,
                        digest,
                    })
            });
        let mut entries = self
            .inner
            .output_entries
            .write()
            .expect("output digest cache lock poisoned");
        match entry {
            Some(entry)
                if entries.get(relative).is_some_and(|current| {
                    current.digest == entry.digest && current.fingerprint == entry.fingerprint
                }) => {}
            Some(entry) => {
                entries.insert(relative.to_string(), entry);
                self.inner.dirty.store(true, Ordering::Relaxed);
            }
            None if entries.remove(relative).is_some() => {
                self.inner.dirty.store(true, Ordering::Relaxed);
            }
            None => {}
        }
    }

    fn record_observation(&self, relative: &str, digest: Digest) {
        self.inner
            .observed_digests
            .write()
            .expect("source digest observation lock poisoned")
            .insert(relative.to_string(), digest);
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if !self.dirty.load(Ordering::Relaxed) {
            return;
        }
        let Ok(entries) = self.entries.read() else {
            return;
        };
        let Ok(output_entries) = self.output_entries.read() else {
            return;
        };
        let Some(parent) = self.path.parent() else {
            return;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let cache = CacheFileRef {
            schema: SCHEMA,
            entries: &entries,
            output_entries: &output_entries,
        };
        let Ok(bytes) = serde_json::to_vec(&cache) else {
            return;
        };
        let temporary = self
            .path
            .with_extension(format!("json.tmp-{}", std::process::id()));
        if std::fs::write(&temporary, bytes).is_err() {
            return;
        }
        if std::fs::rename(&temporary, &self.path).is_err() {
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::rename(&temporary, &self.path);
        }
    }
}

impl Fingerprint {
    fn from_metadata(metadata: &std::fs::Metadata) -> std::io::Result<Self> {
        let modified = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Ok(Self {
            len: metadata.len(),
            modified_seconds: modified.as_secs(),
            modified_nanos: modified.subsec_nanos(),
            file_type: u8::from(metadata.is_file())
                | (u8::from(metadata.file_type().is_symlink()) << 1),
            #[cfg(unix)]
            changed_seconds: std::os::unix::fs::MetadataExt::ctime(metadata),
            #[cfg(unix)]
            changed_nanos: std::os::unix::fs::MetadataExt::ctime_nsec(metadata),
            #[cfg(unix)]
            device: std::os::unix::fs::MetadataExt::dev(metadata),
            #[cfg(unix)]
            inode: std::os::unix::fs::MetadataExt::ino(metadata),
            #[cfg(unix)]
            mode: std::os::unix::fs::MetadataExt::mode(metadata),
        })
    }

    fn can_reuse() -> bool {
        cfg!(unix)
    }
}

#[cfg(test)]
mod tests;
