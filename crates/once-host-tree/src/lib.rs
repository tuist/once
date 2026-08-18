//! Canonical content hash for a host directory tree.
//!
//! `materialize_host_tree` records this digest at analysis time and
//! re-verifies it at execution time. The two happen in different crates, so
//! the hashing must live in one place: any drift between an analysis-side and
//! an execution-side implementation would make every `materialize_host_tree`
//! build fail with a spurious digest mismatch even when nothing on disk
//! changed. Keep this the single source of truth.
//!
//! The digest is stable and order-independent: directory entries are visited
//! sorted by file name, and each entry contributes its kind, workspace-relative
//! path, and either its symlink target or its file content hash. Files and
//! symlinks also contribute their mode; directories do not.
//!
//! Directory permission bits are deliberately excluded. The same digest is
//! computed over a source tree and over the copy that materialization writes
//! next to it, and neither the copy nor a restore from the content-addressed
//! cache carries a directory's permission bits: both create directories with
//! the process umask. Hashing them would make every snapshot of a tree whose
//! directories are not exactly umask-shaped, such as a package unpacked into
//! Cargo's registry cache, fail verification against its own copy. File modes
//! survive both paths and stay in the digest, so the executable bit that a
//! build actually depends on is still an input.
//!
//! Symlinks contribute only their target text, not the contents behind them.
//! That is sound for links that stay inside the tree, but a link whose target
//! resolves outside the tree would let external contents change without moving
//! the digest, while output capture materializes those same external contents.
//! To keep the digest an honest cache key, a symlink proven to point outside
//! the tree is rejected rather than silently under-hashed.

use std::path::Path;

use sha2::Digest as _;

mod digest_cache;

pub use digest_cache::{tree_stat_fingerprint, TreeDigestCache};

/// Hash the directory rooted at `root` into a lowercase hex digest.
///
/// Entries that are neither files, directories, nor symlinks (sockets, fifos,
/// device nodes) are skipped so the digest depends only on portable content.
/// Fails if a symlink resolves to a target outside `root`.
pub fn host_tree_sha256_hex(root: &Path) -> std::io::Result<String> {
    let root_canonical = root.canonicalize()?;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"once.host_tree.v1\0");
    hash_host_tree_directory(root, &root_canonical, root, &mut hasher)?;
    Ok(hex_lower(&hasher.finalize()))
}

fn hash_host_tree_directory(
    root: &Path,
    root_canonical: &Path,
    directory: &Path,
    hasher: &mut sha2::Sha256,
) -> std::io::Result<()> {
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
        let kind = if metadata.file_type().is_symlink() {
            b'l'
        } else if metadata.is_dir() {
            b'd'
        } else if metadata.is_file() {
            b'f'
        } else {
            continue;
        };
        hasher.update([kind]);
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        if kind != b'd' {
            hasher.update(host_file_mode(&metadata).to_le_bytes());
        }
        if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(&path)?;
            if symlink_target_escapes(root_canonical, &path, &target) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "host tree `{}` contains symlink `{}` pointing outside the tree; \
                         external symlink targets are not supported because their contents \
                         cannot be tracked in the tree digest",
                        root.display(),
                        path.display()
                    ),
                ));
            }
            hasher.update(target.to_string_lossy().as_bytes());
        } else if metadata.is_file() {
            hasher.update(file_sha256_hex(&path)?.as_bytes());
        }
        hasher.update([0]);
        if metadata.is_dir() {
            hash_host_tree_directory(root, root_canonical, &path, hasher)?;
        }
    }
    Ok(())
}

/// Whether `target` (as read from the symlink at `symlink_path`) provably
/// resolves outside `root_canonical`. A target that cannot be resolved (a
/// dangling link) is treated as inside: it carries no external contents to
/// track, so hashing its link text alone stays sound.
fn symlink_target_escapes(root_canonical: &Path, symlink_path: &Path, target: &Path) -> bool {
    let resolved = if target.is_absolute() {
        target.to_path_buf()
    } else {
        match symlink_path.parent() {
            Some(parent) => parent.join(target),
            None => target.to_path_buf(),
        }
    };
    match resolved.canonicalize() {
        Ok(canonical) => !canonical.starts_with(root_canonical),
        Err(_) => false,
    }
}

fn file_sha256_hex(path: &Path) -> std::io::Result<String> {
    use std::io::Read;

    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = sha2::Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

#[cfg(unix)]
pub(crate) fn host_file_mode(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode()
}

#[cfg(not(unix))]
fn host_file_mode(_metadata: &std::fs::Metadata) -> u32 {
    0
}

pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_directory_hashes_to_the_version_prefix_only() {
        let tmp = tempfile::TempDir::new().unwrap();
        // sha256("once.host_tree.v1\0")
        assert_eq!(host_tree_sha256_hex(tmp.path()).unwrap(), {
            let mut hasher = sha2::Sha256::new();
            hasher.update(b"once.host_tree.v1\0");
            hex_lower(&hasher.finalize())
        });
    }

    #[test]
    fn digest_is_stable_and_order_independent() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("b.txt"), b"b").unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"a").unwrap();
        std::fs::write(tmp.path().join("sub/c.txt"), b"c").unwrap();

        let first = host_tree_sha256_hex(tmp.path()).unwrap();
        let second = host_tree_sha256_hex(tmp.path()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
    }

    #[cfg(unix)]
    #[test]
    fn internal_symlink_is_hashed_by_target_text() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("real.txt"), b"payload").unwrap();
        std::os::unix::fs::symlink("real.txt", tmp.path().join("link.txt")).unwrap();
        // A link that stays inside the tree is accepted and stable.
        let first = host_tree_sha256_hex(tmp.path()).unwrap();
        let second = host_tree_sha256_hex(tmp.path()).unwrap();
        assert_eq!(first, second);
    }

    #[cfg(unix)]
    #[test]
    fn external_symlink_is_rejected() {
        let outside = tempfile::TempDir::new().unwrap();
        let external = outside.path().join("secret.txt");
        std::fs::write(&external, b"external payload").unwrap();

        let tmp = tempfile::TempDir::new().unwrap();
        std::os::unix::fs::symlink(&external, tmp.path().join("link.txt")).unwrap();

        let err = host_tree_sha256_hex(tmp.path()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_is_accepted() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::os::unix::fs::symlink("does-not-exist", tmp.path().join("link.txt")).unwrap();
        // Unresolvable links carry no external contents, so hashing the link
        // text alone stays sound and must not error.
        assert_eq!(host_tree_sha256_hex(tmp.path()).unwrap().len(), 64);
    }

    #[cfg(unix)]
    #[test]
    fn directory_permissions_do_not_change_the_digest() {
        use std::os::unix::fs::PermissionsExt as _;

        let tmp = tempfile::TempDir::new().unwrap();
        let nested = tmp.path().join("src");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("lib.rs"), b"payload").unwrap();

        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o755)).unwrap();
        let umask_shaped = host_tree_sha256_hex(tmp.path()).unwrap();
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o700)).unwrap();
        let private = host_tree_sha256_hex(tmp.path()).unwrap();

        assert_eq!(umask_shaped, private);
    }

    #[cfg(unix)]
    #[test]
    fn file_permissions_change_the_digest() {
        use std::os::unix::fs::PermissionsExt as _;

        let tmp = tempfile::TempDir::new().unwrap();
        let script = tmp.path().join("configure");
        std::fs::write(&script, b"payload").unwrap();

        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644)).unwrap();
        let plain = host_tree_sha256_hex(tmp.path()).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let executable = host_tree_sha256_hex(tmp.path()).unwrap();

        assert_ne!(plain, executable);
    }

    #[test]
    fn content_change_changes_the_digest() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"a").unwrap();
        let before = host_tree_sha256_hex(tmp.path()).unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"changed").unwrap();
        let after = host_tree_sha256_hex(tmp.path()).unwrap();
        assert_ne!(before, after);
    }
}
