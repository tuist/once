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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolved: Option<Box<PathFingerprint>>,
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
        let is_symlink = metadata.file_type().is_symlink();
        let link_target = is_symlink
            .then(|| std::fs::read_link(path).ok())
            .flatten()
            .map(|target| target.to_string_lossy().into_owned());
        // When the observed path is a symlink (for example /usr/bin/cc), the
        // link's own metadata and the recorded target path stay constant if the
        // destination binary is replaced in place. Follow the link and bind the
        // resolved target's identity so such an in-place tool change still
        // invalidates the receipt, while the link's own identity is retained.
        let resolved = is_symlink
            .then(|| {
                std::fs::metadata(path)
                    .ok()
                    .and_then(|resolved| Self::from_metadata(&resolved))
            })
            .flatten()
            .map(Box::new);
        let mut fingerprint = Self::from_metadata(&metadata)?;
        fingerprint.link_target = link_target;
        fingerprint.resolved = resolved;
        Some(fingerprint)
    }

    fn from_metadata(metadata: &std::fs::Metadata) -> Option<Self> {
        let modified = metadata
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Some(Self {
            len: metadata.len(),
            modified_seconds: modified.as_secs(),
            modified_nanos: modified.subsec_nanos(),
            file_type: u8::from(metadata.is_file())
                | (u8::from(metadata.is_dir()) << 1)
                | (u8::from(metadata.file_type().is_symlink()) << 2),
            link_target: None,
            resolved: None,
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
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn symlink_fingerprint_tracks_in_place_target_changes() {
        let directory = tempfile::TempDir::new().unwrap();
        let target = directory.path().join("gcc-real");
        let link = directory.path().join("cc");
        std::fs::write(&target, b"compiler one").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let observed = BTreeSet::from([link.clone()]);
        let baseline = capture(&observed);
        // The link itself is untouched; only the resolved binary is replaced in
        // place. Without binding the resolved target this would look unchanged.
        std::fs::write(&target, b"compiler two has more bytes").unwrap();

        assert_eq!(
            changed(&baseline),
            vec![link.to_string_lossy().into_owned()]
        );
    }
}
