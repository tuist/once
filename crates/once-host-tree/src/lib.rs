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
//! path, mode, and either its symlink target or its file content hash.

use std::path::Path;

use sha2::Digest as _;

/// Hash the directory rooted at `root` into a lowercase hex digest.
///
/// Entries that are neither files, directories, nor symlinks (sockets, fifos,
/// device nodes) are skipped so the digest depends only on portable content.
pub fn host_tree_sha256_hex(root: &Path) -> std::io::Result<String> {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"once.host_tree.v1\0");
    hash_host_tree_directory(root, root, &mut hasher)?;
    Ok(hex_lower(&hasher.finalize()))
}

fn hash_host_tree_directory(
    root: &Path,
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
        hasher.update(host_file_mode(&metadata).to_le_bytes());
        if metadata.file_type().is_symlink() {
            hasher.update(std::fs::read_link(&path)?.to_string_lossy().as_bytes());
        } else if metadata.is_file() {
            hasher.update(file_sha256_hex(&path)?.as_bytes());
        }
        hasher.update([0]);
        if metadata.is_dir() {
            hash_host_tree_directory(root, &path, hasher)?;
        }
    }
    Ok(())
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
fn host_file_mode(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode()
}

#[cfg(not(unix))]
fn host_file_mode(_metadata: &std::fs::Metadata) -> u32 {
    0
}

fn hex_lower(bytes: &[u8]) -> String {
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
        assert_eq!(
            host_tree_sha256_hex(tmp.path()).unwrap(),
            {
                let mut hasher = sha2::Sha256::new();
                hasher.update(b"once.host_tree.v1\0");
                hex_lower(&hasher.finalize())
            }
        );
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
