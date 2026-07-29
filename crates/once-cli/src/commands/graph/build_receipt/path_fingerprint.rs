use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct PathFingerprint {
    len: u64,
    modified_seconds: u64,
    modified_nanos: u32,
    file_type: u8,
    link_target: Option<String>,
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

pub(super) fn capture(paths: &BTreeSet<PathBuf>) -> BTreeMap<String, Option<PathFingerprint>> {
    paths
        .iter()
        .map(|path| {
            (
                path.to_string_lossy().into_owned(),
                PathFingerprint::capture(path),
            )
        })
        .collect()
}

pub(super) fn changed(expected: &BTreeMap<String, Option<PathFingerprint>>) -> Vec<String> {
    expected
        .iter()
        .filter(|(path, fingerprint)| PathFingerprint::capture(Path::new(path)) != **fingerprint)
        .map(|(path, _)| path.clone())
        .collect()
}

pub(super) fn fingerprint(path: &Path) -> Option<PathFingerprint> {
    PathFingerprint::capture(path)
}

impl PathFingerprint {
    fn capture(path: &Path) -> Option<Self> {
        let metadata = std::fs::symlink_metadata(path).ok()?;
        let modified = metadata
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let link_target = metadata
            .file_type()
            .is_symlink()
            .then(|| std::fs::read_link(path).ok())
            .flatten()
            .map(|target| target.to_string_lossy().into_owned());
        Some(Self {
            len: metadata.len(),
            modified_seconds: modified.as_secs(),
            modified_nanos: modified.subsec_nanos(),
            file_type: u8::from(metadata.is_file())
                | (u8::from(metadata.is_dir()) << 1)
                | (u8::from(metadata.file_type().is_symlink()) << 2),
            link_target,
            #[cfg(unix)]
            changed_seconds: std::os::unix::fs::MetadataExt::ctime(&metadata),
            #[cfg(unix)]
            changed_nanos: std::os::unix::fs::MetadataExt::ctime_nsec(&metadata),
            #[cfg(unix)]
            device: std::os::unix::fs::MetadataExt::dev(&metadata),
            #[cfg(unix)]
            inode: std::os::unix::fs::MetadataExt::ino(&metadata),
            #[cfg(unix)]
            mode: std::os::unix::fs::MetadataExt::mode(&metadata),
        })
    }
}
