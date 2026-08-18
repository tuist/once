//! Remember a tree's content digest against a cheap description of the tree.
//!
//! Hashing a directory means reading every byte in it. A build that leaves
//! its inputs alone pays that on every invocation: the snapshot of a locked
//! package taken from a package manager's cache never changes, and neither
//! does an output already sitting on disk from the previous run, yet both get
//! re-read to prove it.
//!
//! Walking the same tree and recording each entry's size, mode, inode, device,
//! modification time, and inode change time costs a `lstat` per entry rather
//! than a read, and any write to a file moves at least one of those. That makes
//! the stat description a stand-in for "this tree is the one I hashed last
//! time", the same description the source digest cache already keeps for
//! individual files.
//!
//! The inode change time is what carries the argument. Modification time is a
//! claim a process can make about a file: `cp -p`, `rsync --times`, and archive
//! extraction all restore a recorded mtime onto content they just wrote, and a
//! description resting on mtime alone calls that unchanged. No process can set
//! ctime, because the kernel stamps it on every write to the inode, so any of
//! those rewrites moves it. What is left is a machine whose clock is being
//! moved backwards under it, or writes going around the filesystem to the raw
//! device, neither of which any content digest survives either.
//!
//! Set `ONCE_TREE_DIGEST_CACHE=0` to hash every tree in full instead.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use sha2::Digest as _;

use crate::{hex_lower, host_file_mode};

const SCHEMA: &str = "once.tree_digests.v2";

/// Cap on remembered trees, so a long-lived workspace cannot grow the file
/// without bound. Eviction is wholesale rather than least-recently-used: the
/// file is a cache, and rebuilding it costs one slow invocation.
const MAX_ENTRIES: usize = 20_000;

pub struct TreeDigestCache {
    path: PathBuf,
    entries: Mutex<BTreeMap<String, Entry>>,
    /// Answers already given during this run, so the same tree is described
    /// once however many callers ask about it.
    ///
    /// A build with a hundred locked dependencies asks about the same unpacked
    /// package from every target that depends on it, and each ask would
    /// otherwise walk it again. Nothing a build produces lands in one of these
    /// trees, and a tree that did change under a running build was already
    /// racing the reader; settling on one answer is what the persisted entries
    /// below do across runs, and this does the same within one.
    answered: Mutex<BTreeMap<String, String>>,
    dirty: AtomicBool,
}

#[derive(Clone, PartialEq, Eq)]
struct Entry {
    fingerprint: String,
    digest: String,
}

impl TreeDigestCache {
    /// Open the cache stored at `path`, starting empty when it is missing or
    /// unreadable. A cache that cannot be read is not an error: it only means
    /// the next hash is computed the slow way.
    #[must_use]
    pub fn open(path: PathBuf) -> Self {
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| parse(&text))
            .unwrap_or_default();
        Self {
            path,
            entries: Mutex::new(entries),
            answered: Mutex::new(BTreeMap::new()),
            dirty: AtomicBool::new(false),
        }
    }

    /// Digest of the tree at `root`, computing it with `compute` only when the
    /// tree's stat description differs from the remembered one.
    ///
    /// `key` distinguishes callers that hash the same tree differently, so two
    /// digest schemes cannot read each other's answers.
    pub fn digest<F>(&self, key: &str, root: &Path, compute: F) -> std::io::Result<String>
    where
        F: FnOnce() -> std::io::Result<String>,
    {
        if !enabled() {
            return compute();
        }
        let slot = format!("{key}\u{0}{}", root.to_string_lossy());
        if let Some(answer) = self.lock_answered().get(&slot) {
            return Ok(answer.clone());
        }
        // A tree that cannot be walked cannot be described, so fall through to
        // the real hash and let it report the problem.
        let Ok(fingerprint) = tree_stat_fingerprint(root) else {
            return compute();
        };
        if let Some(entry) = self.lock().get(&slot) {
            if entry.fingerprint == fingerprint {
                let digest = entry.digest.clone();
                self.remember_answer(slot, &digest);
                return Ok(digest);
            }
        }
        let digest = compute()?;
        {
            let mut entries = self.lock();
            if entries.len() >= MAX_ENTRIES {
                entries.clear();
            }
            entries.insert(
                slot.clone(),
                Entry {
                    fingerprint,
                    digest: digest.clone(),
                },
            );
        }
        self.dirty.store(true, Ordering::Relaxed);
        self.remember_answer(slot, &digest);
        Ok(digest)
    }

    fn remember_answer(&self, slot: String, digest: &str) {
        self.lock_answered().insert(slot, digest.to_string());
    }

    fn lock_answered(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, String>> {
        self.answered
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Write the cache back when anything was learned. Failures are ignored
    /// for the same reason a missing file is: the cache is an optimization.
    pub fn save(&self) {
        if !self.dirty.swap(false, Ordering::Relaxed) {
            return;
        }
        let entries = self.lock();
        let mut out = String::from(SCHEMA);
        out.push('\n');
        for (slot, entry) in entries.iter() {
            // One record per line, tab separated. The slot holds a path, so
            // it can contain almost anything except a newline or a tab.
            if slot.contains('\n') || slot.contains('\t') {
                continue;
            }
            out.push_str(slot);
            out.push('\t');
            out.push_str(&entry.fingerprint);
            out.push('\t');
            out.push_str(&entry.digest);
            out.push('\n');
        }
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let temporary = self
            .path
            .with_extension(format!("tmp-{}", std::process::id()));
        if std::fs::write(&temporary, out).is_ok()
            && std::fs::rename(&temporary, &self.path).is_err()
        {
            let _ = std::fs::remove_file(&temporary);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Entry>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for TreeDigestCache {
    fn drop(&mut self) {
        self.save();
    }
}

/// Whether the stat-described shortcut is in use. Off makes every lookup a
/// full content hash, which is slower and answers only to the bytes.
fn enabled() -> bool {
    !matches!(
        std::env::var("ONCE_TREE_DIGEST_CACHE").ok().as_deref(),
        Some("0" | "false")
    )
}

fn parse(text: &str) -> Option<BTreeMap<String, Entry>> {
    let mut lines = text.lines();
    if lines.next()? != SCHEMA {
        return None;
    }
    let mut entries = BTreeMap::new();
    for line in lines {
        let mut parts = line.split('\t');
        let (Some(slot), Some(fingerprint), Some(digest), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        entries.insert(
            slot.to_string(),
            Entry {
                fingerprint: fingerprint.to_string(),
                digest: digest.to_string(),
            },
        );
    }
    Some(entries)
}

/// Describe a tree by the metadata of everything in it, without reading any
/// contents. Entries are visited in sorted order so the description is stable.
pub fn tree_stat_fingerprint(root: &Path) -> std::io::Result<String> {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"once.tree_stat.v2\0");
    hash_stat_directory(root, root, &mut hasher, 0)?;
    Ok(hex_lower(&hasher.finalize()))
}

/// Guard against a symlink loop turning the walk into an infinite descent.
/// Real source trees are nowhere near this deep.
const MAX_DEPTH: u32 = 64;

fn hash_stat_directory(
    root: &Path,
    directory: &Path,
    hasher: &mut sha2::Sha256,
    depth: u32,
) -> std::io::Result<()> {
    if depth > MAX_DEPTH {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "tree `{}` is nested deeper than {MAX_DEPTH}",
                root.display()
            ),
        ));
    }
    let mut children = std::fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let path = child.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        let relative = path
            .strip_prefix(root)
            .map_err(std::io::Error::other)?
            .to_string_lossy()
            .replace('\\', "/");
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update(host_file_mode(&metadata).to_le_bytes());
        hasher.update(metadata.len().to_le_bytes());
        hasher.update(stat_change_marker(&metadata));
        if metadata.file_type().is_symlink() {
            // The link's own text is cheap and is what the content digest
            // records, so include it rather than trusting its metadata alone.
            hasher.update(std::fs::read_link(&path)?.to_string_lossy().as_bytes());
        } else if metadata.is_dir() {
            hash_stat_directory(root, &path, hasher, depth + 1)?;
        }
        hasher.update([0]);
    }
    Ok(())
}

/// Size of one entry's metadata marker. Fixed width so the fields cannot
/// shift into one another between platforms.
const STAT_MARKER_LEN: usize = 48;

#[cfg(unix)]
fn stat_change_marker(metadata: &std::fs::Metadata) -> [u8; STAT_MARKER_LEN] {
    use std::os::unix::fs::MetadataExt as _;

    let mut marker = [0_u8; STAT_MARKER_LEN];
    marker[..8].copy_from_slice(&metadata.mtime().to_le_bytes());
    marker[8..16].copy_from_slice(&metadata.mtime_nsec().to_le_bytes());
    // Inode and device catch a file replaced by a different one that happens
    // to keep the same size and timestamp.
    marker[16..24].copy_from_slice(&metadata.ino().to_le_bytes());
    marker[24..32].copy_from_slice(&metadata.dev().to_le_bytes());
    // The inode change time is what makes this a description of the file
    // rather than a description of what someone claims about it. A process can
    // set mtime to any value it likes, which is how `cp -p`, `rsync --times`,
    // and archive extraction reproduce a source timestamp on new content;
    // none of them can set ctime, because the kernel stamps it on every
    // inode write. Without it, restoring an old copy of a file over a new one
    // looks like no change at all.
    marker[32..40].copy_from_slice(&metadata.ctime().to_le_bytes());
    marker[40..].copy_from_slice(&metadata.ctime_nsec().to_le_bytes());
    marker
}

#[cfg(not(unix))]
fn stat_change_marker(metadata: &std::fs::Metadata) -> [u8; STAT_MARKER_LEN] {
    let mut marker = [0_u8; STAT_MARKER_LEN];
    if let Ok(modified) = metadata.modified() {
        if let Ok(since) = modified.duration_since(std::time::SystemTime::UNIX_EPOCH) {
            marker[..8].copy_from_slice(&since.as_secs().to_le_bytes());
            marker[8..12].copy_from_slice(&since.subsec_nanos().to_le_bytes());
        }
    }
    marker
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reuses_a_digest_while_the_tree_is_untouched() {
        let tmp = tempfile::TempDir::new().unwrap();
        let tree = tmp.path().join("tree");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join("a.txt"), b"a").unwrap();
        let cache = TreeDigestCache::open(tmp.path().join("cache"));

        let first = cache
            .digest("test", &tree, || Ok("computed-1".to_string()))
            .unwrap();
        let second = cache
            .digest("test", &tree, || panic!("should not recompute"))
            .unwrap();

        assert_eq!(first, "computed-1");
        assert_eq!(second, "computed-1");
    }

    /// Reopen the cache from disk, standing in for the next invocation. Within
    /// one invocation a tree is described once; noticing a change is the next
    /// invocation's job, and that is what the persisted entries are for.
    fn next_run(cache: TreeDigestCache, path: &std::path::Path) -> TreeDigestCache {
        cache.save();
        drop(cache);
        TreeDigestCache::open(path.to_path_buf())
    }

    #[test]
    fn recomputes_after_a_content_change() {
        let tmp = tempfile::TempDir::new().unwrap();
        let tree = tmp.path().join("tree");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join("a.txt"), b"a").unwrap();
        let path = tmp.path().join("cache");
        let cache = TreeDigestCache::open(path.clone());
        cache
            .digest("test", &tree, || Ok("computed-1".to_string()))
            .unwrap();

        std::fs::write(tree.join("a.txt"), b"changed and longer").unwrap();
        let after = next_run(cache, &path)
            .digest("test", &tree, || Ok("computed-2".to_string()))
            .unwrap();

        assert_eq!(after, "computed-2");
    }

    #[test]
    fn recomputes_after_a_file_appears() {
        let tmp = tempfile::TempDir::new().unwrap();
        let tree = tmp.path().join("tree");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join("a.txt"), b"a").unwrap();
        let path = tmp.path().join("cache");
        let cache = TreeDigestCache::open(path.clone());
        cache
            .digest("test", &tree, || Ok("computed-1".to_string()))
            .unwrap();

        std::fs::write(tree.join("b.txt"), b"b").unwrap();
        let after = next_run(cache, &path)
            .digest("test", &tree, || Ok("computed-2".to_string()))
            .unwrap();

        assert_eq!(after, "computed-2");
    }

    /// The cost of asking once: a tree rewritten while a build is reading it
    /// keeps the answer the build already gave. Every reader in that build
    /// then agrees, rather than some seeing the tree before the write and
    /// some after, and the next invocation sees the new tree.
    #[test]
    fn one_run_describes_a_tree_once_even_if_it_changes_underneath() {
        let tmp = tempfile::TempDir::new().unwrap();
        let tree = tmp.path().join("tree");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join("a.txt"), b"a").unwrap();
        let cache = TreeDigestCache::open(tmp.path().join("cache"));
        cache
            .digest("test", &tree, || Ok("computed-1".to_string()))
            .unwrap();

        std::fs::write(tree.join("a.txt"), b"changed and longer").unwrap();

        assert_eq!(
            cache
                .digest("test", &tree, || Ok("computed-2".to_string()))
                .unwrap(),
            "computed-1"
        );
    }

    /// The case a modification-time description gets wrong: content replaced
    /// with a different revision of the same length, carrying the timestamp it
    /// had before. This is what `cp -p`, `rsync --times`, and unpacking an
    /// archive over a working copy all do.
    #[test]
    fn recomputes_after_a_rewrite_that_restores_the_timestamp() {
        let tmp = tempfile::TempDir::new().unwrap();
        let tree = tmp.path().join("tree");
        std::fs::create_dir_all(&tree).unwrap();
        let file = tree.join("a.txt");
        std::fs::write(&file, b"aaaa").unwrap();
        let original = std::fs::metadata(&file).unwrap().modified().unwrap();
        let path = tmp.path().join("cache");
        let cache = TreeDigestCache::open(path.clone());
        cache
            .digest("test", &tree, || Ok("computed-1".to_string()))
            .unwrap();

        std::fs::write(&file, b"bbbb").unwrap();
        std::fs::File::open(&file)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(original))
            .unwrap();
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            original,
            "the rewrite must present the original modification time"
        );

        let after = next_run(cache, &path)
            .digest("test", &tree, || Ok("computed-2".to_string()))
            .unwrap();

        assert_eq!(after, "computed-2");
    }

    #[test]
    fn keeps_digests_of_different_schemes_apart() {
        let tmp = tempfile::TempDir::new().unwrap();
        let tree = tmp.path().join("tree");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join("a.txt"), b"a").unwrap();
        let cache = TreeDigestCache::open(tmp.path().join("cache"));

        let one = cache
            .digest("scheme-a", &tree, || Ok("a".to_string()))
            .unwrap();
        let two = cache
            .digest("scheme-b", &tree, || Ok("b".to_string()))
            .unwrap();

        assert_eq!((one.as_str(), two.as_str()), ("a", "b"));
    }

    #[test]
    fn survives_a_round_trip_through_the_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let tree = tmp.path().join("tree");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join("a.txt"), b"a").unwrap();
        let path = tmp.path().join("cache");

        let cache = TreeDigestCache::open(path.clone());
        cache
            .digest("test", &tree, || Ok("computed-1".to_string()))
            .unwrap();
        cache.save();
        drop(cache);

        let reopened = TreeDigestCache::open(path);
        let reused = reopened
            .digest("test", &tree, || panic!("should not recompute"))
            .unwrap();

        assert_eq!(reused, "computed-1");
    }
}
