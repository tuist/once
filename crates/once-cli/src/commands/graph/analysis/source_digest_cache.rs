use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::UNIX_EPOCH;

use once_cas::Digest;
use serde::{Deserialize, Serialize};

use crate::commands::change_tracker::ChangePosition;

const SCHEMA: &str = "once.source-digests.v3";

/// What a filesystem watcher can say about the window since this cache was
/// written.
///
/// Every recorded digest here answers the question "what was in this path when
/// I last looked". Deciding whether the answer still holds normally means
/// looking again: a `stat` for a file, a walk for a directory. A watcher that
/// has been running across the whole window can answer instead, and it answers
/// for every path at once.
#[derive(Debug, Default, Clone)]
pub(crate) enum KnownChanges {
    /// No watcher, a watcher that started after this cache was written, or one
    /// that lost track. Every path is looked at.
    #[default]
    Unknown,
    /// Exactly these paths changed since the cache was written; nothing else
    /// did. Paths are workspace-relative.
    Since {
        sources: BTreeSet<String>,
        outputs: BTreeSet<String>,
    },
}

impl KnownChanges {
    /// The same statement, in the form the analysis layer takes.
    pub(super) fn unchanged_workspace(&self) -> once_frontend::analysis::UnchangedWorkspace {
        match self {
            Self::Unknown => once_frontend::analysis::UnchangedWorkspace::Unknown,
            Self::Since { sources, .. } => {
                once_frontend::analysis::UnchangedWorkspace::Except(sources.clone())
            }
        }
    }

    /// Whether the watcher positively says this path is untouched.
    ///
    /// A directory is untouched only when nothing under it changed either, so a
    /// changed path is matched against the whole subtree.
    fn settles(changed: &BTreeSet<String>, relative: &str) -> bool {
        // The common case by far: nothing changed at all.
        if changed.is_empty() {
            return true;
        }
        if changed.contains(relative) {
            return false;
        }
        let prefix = format!("{relative}/");
        // Ordered set, so the candidates for "inside this directory" are a
        // contiguous run starting at the prefix.
        !changed
            .range(prefix.clone()..)
            .take_while(|path| path.starts_with(&prefix))
            .any(|_| true)
    }

    fn source_is_untouched(&self, relative: &str) -> bool {
        match self {
            Self::Unknown => false,
            Self::Since { sources, .. } => Self::settles(sources, relative),
        }
    }

    fn output_is_untouched(&self, relative: &str) -> bool {
        match self {
            Self::Unknown => false,
            Self::Since { outputs, .. } => Self::settles(outputs, relative),
        }
    }
}

#[derive(Clone)]
pub struct SourceDigestCache {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    entries: RwLock<BTreeMap<String, Entry>>,
    output_entries: RwLock<BTreeMap<String, Entry>>,
    observed_digests: RwLock<BTreeMap<String, Digest>>,
    /// What the watcher says moved since these entries were written.
    known_changes: RwLock<KnownChanges>,
    /// Watcher position to stamp on the way out, so the next invocation knows
    /// which window to ask about.
    position: RwLock<Option<ChangePosition>>,
    dirty: AtomicBool,
}

#[derive(Clone, Deserialize, Serialize)]
struct Entry {
    fingerprint: Fingerprint,
    digest: Digest,
    /// Stat description of the whole tree, for directory paths only.
    ///
    /// A directory's own metadata says nothing about the files beneath it: a
    /// file can be rewritten without the directory being touched at all. So a
    /// directory needs the description of everything under it, which is a walk
    /// of `lstat` calls rather than a read of every byte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tree: Option<String>,
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
    /// Watcher position when these entries were written. Absent when no watcher
    /// was running, which leaves the next invocation nothing to ask about.
    #[serde(default)]
    position: Option<ChangePosition>,
}

#[derive(Serialize)]
struct CacheFileRef<'a> {
    schema: &'static str,
    entries: &'a BTreeMap<String, Entry>,
    output_entries: &'a BTreeMap<String, Entry>,
    position: &'a Option<ChangePosition>,
}

/// The watcher position stamped on a workspace's cache, without loading it.
///
/// Read before the session exists, so the one snapshot an invocation takes can
/// cover this cache's window as well as the build receipt's.
pub(crate) fn stored_position(workspace: &Path) -> Option<ChangePosition> {
    let path = workspace.join(".once").join("source-digests.json");
    let cache = serde_json::from_slice::<CacheFile>(&std::fs::read(path).ok()?).ok()?;
    (cache.schema == SCHEMA).then_some(cache.position).flatten()
}

impl SourceDigestCache {
    pub(crate) fn open(workspace: &Path) -> Self {
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
                known_changes: RwLock::new(KnownChanges::Unknown),
                position: RwLock::new(None),
                dirty: AtomicBool::new(false),
            }),
        }
    }

    /// Tell the cache what a watcher observed since its entries were written.
    pub(super) fn with_known_changes(&self, changes: KnownChanges) {
        *self
            .inner
            .known_changes
            .write()
            .expect("known change lock poisoned") = changes;
    }

    /// Stamp the watcher position these entries now describe.
    pub(super) fn set_position(&self, position: Option<ChangePosition>) {
        let mut stored = self.inner.position.write().expect("position lock poisoned");
        if *stored == position {
            return;
        }
        *stored = position;
        self.inner.dirty.store(true, Ordering::Relaxed);
    }

    fn known_changes(&self) -> KnownChanges {
        self.inner
            .known_changes
            .read()
            .expect("known change lock poisoned")
            .clone()
    }

    pub(super) fn digest(&self, workspace: &Path, relative: &str) -> std::io::Result<Digest> {
        // Nothing wrote here since the entry was recorded, so the entry is the
        // answer and the filesystem has nothing to add. This is where a watcher
        // earns its keep: for a directory it replaces a walk of every entry
        // beneath it with a lookup.
        if self.known_changes().source_is_untouched(relative) {
            if let Some(digest) = self
                .inner
                .entries
                .read()
                .expect("source digest cache lock poisoned")
                .get(relative)
                .map(|entry| entry.digest)
            {
                self.record_observation(relative, digest);
                return Ok(digest);
            }
        }
        let absolute = workspace.join(relative);
        let metadata = std::fs::symlink_metadata(&absolute)?;
        let fingerprint = Fingerprint::from_metadata(&metadata)?;
        // A directory input is the expensive case by a wide margin: hashing one
        // reads every byte under it, and a graph derived from a package manifest
        // declares whole unpacked dependency trees as inputs. Describe the tree
        // by metadata instead, and read it only when that description moves.
        let tree = if metadata.is_dir() {
            Some(once_core::tree_stat_fingerprint(&absolute)?)
        } else {
            None
        };
        if let Some(digest) = self
            .inner
            .entries
            .read()
            .expect("source digest cache lock poisoned")
            .get(relative)
            .filter(|entry| {
                Fingerprint::can_reuse() && entry.fingerprint == fingerprint && entry.tree == tree
            })
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
                    tree,
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
        if required.outputs.is_empty() {
            // Every output was already the one this action produced, which is
            // what the recorded description said. Describing them again to
            // record what is already recorded would double the cost of the
            // cheapest possible outcome.
            return Ok(());
        }
        once_core::materialize_outputs(&required, workspace, cache).await?;
        self.record_outputs(&required, workspace);
        Ok(())
    }

    pub(super) fn record_outputs(&self, result: &once_cas::ActionResult, workspace: &Path) {
        for (relative, digest) in &result.outputs {
            self.record_output(workspace, relative, *digest);
        }
    }

    /// The digest recorded for an output, if this cache has one.
    pub(crate) fn recorded_output_digest(&self, relative: &str) -> Option<Digest> {
        self.inner
            .output_entries
            .read()
            .expect("output digest cache lock poisoned")
            .get(relative)
            .map(|entry| entry.digest)
    }

    pub(crate) fn output_matches(
        &self,
        workspace: &Path,
        relative: &str,
        expected: Digest,
    ) -> bool {
        if self.known_changes().output_is_untouched(relative) {
            if workspace.join(relative).symlink_metadata().is_err() {
                return false;
            }
            return self
                .inner
                .output_entries
                .read()
                .expect("output digest cache lock poisoned")
                .get(relative)
                .is_some_and(|entry| entry.digest == expected);
        }
        let Some(described) = describe_output(workspace, relative) else {
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
                    && entry.fingerprint == described.fingerprint
                    && entry.tree == described.tree
            })
    }

    fn record_output(&self, workspace: &Path, relative: &str, digest: Digest) {
        let entry = describe_output(workspace, relative).map(|described| Entry {
            fingerprint: described.fingerprint,
            digest,
            tree: described.tree,
        });
        let mut entries = self
            .inner
            .output_entries
            .write()
            .expect("output digest cache lock poisoned");
        match entry {
            Some(entry)
                if entries.get(relative).is_some_and(|current| {
                    current.digest == entry.digest
                        && current.fingerprint == entry.fingerprint
                        && current.tree == entry.tree
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

struct DescribedOutput {
    fingerprint: Fingerprint,
    tree: Option<String>,
}

/// Describe an output on disk cheaply enough to be worth doing on every
/// invocation, or `None` when it is a shape this cache does not speak for.
///
/// Symlinks are excluded rather than described: what matters about one is what
/// it resolves to, which is a different question from whether the link itself
/// moved, and reading the link is cheap anyway.
fn describe_output(workspace: &Path, relative: &str) -> Option<DescribedOutput> {
    let absolute = workspace.join(relative);
    let metadata = std::fs::symlink_metadata(&absolute).ok()?;
    if metadata.file_type().is_symlink() {
        return None;
    }
    // Directory outputs are why this exists. A compiler's output directory,
    // or a build script's, is a directory holding anything from one file to
    // thousands, and hashing it is the largest single cost of an invocation
    // that has nothing to do. The description below walks it with `lstat`.
    let tree = if metadata.is_dir() {
        Some(once_core::tree_stat_fingerprint(&absolute).ok()?)
    } else if metadata.is_file() {
        None
    } else {
        return None;
    };
    Some(DescribedOutput {
        fingerprint: Fingerprint::from_metadata(&metadata).ok()?,
        tree,
    })
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
        let Ok(position) = self.position.read() else {
            return;
        };
        let cache = CacheFileRef {
            schema: SCHEMA,
            entries: &entries,
            output_entries: &output_entries,
            position: &position,
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
