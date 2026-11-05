//! Canonical content hash for a host directory tree.
//!
//! `materialize_host_tree` records this digest at analysis time and
//! re-verifies it at execution time. The two happen in different crates, so
//! the hashing must live in one place: any drift between an analysis-side and
//! an execution-side implementation would make every `materialize_host_tree`
//! build fail with a spurious digest mismatch even when nothing on disk
//! changed. Keep this the single source of truth.
//!
//! Paths must be valid UTF-8 so their portable spelling is unambiguous.
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
//! Symlinks that stay inside the tree contribute only their target text, not
//! the contents behind them. A link whose target resolves outside the tree
//! would let external contents change without moving the digest, so such a
//! link to a regular file is hashed as that file, with its mode and contents,
//! and materialization copies it as a regular file, the way a package inside
//! a repository checkout sees a license file linked from the repository root.
//! A link proven to point at a directory outside the tree is
//! rejected rather than silently under-hashed.

use std::path::{Path, PathBuf};

use sha2::Digest as _;

/// Hash the directory rooted at `root` into a lowercase hex digest.
///
/// Entries that are neither files, directories, nor symlinks (sockets, fifos,
/// device nodes) are skipped so the digest depends only on portable content.
/// Fails if a symlink resolves to a directory outside `root`.
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
        let mut metadata = std::fs::symlink_metadata(&path)?;
        let relative = portable_path(path.strip_prefix(root).map_err(std::io::Error::other)?)?;
        let mut link_target = None;
        if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(&path)?;
            match escaping_symlink_target(root_canonical, &path, &target) {
                Some(resolved) if resolved.is_file() => metadata = std::fs::metadata(&resolved)?,
                Some(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "host tree `{}` contains symlink `{}` pointing to a directory \
                             outside the tree; external directory targets are not supported \
                             because their contents cannot be tracked in the tree digest",
                            root.display(),
                            path.display()
                        ),
                    ));
                }
                None => link_target = Some(target),
            }
        }
        let kind = if link_target.is_some() {
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
        if let Some(target) = &link_target {
            hasher.update(utf8_path(target)?.as_bytes());
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

/// The canonical path `target` (as read from the symlink at `symlink_path`)
/// resolves to, when it provably lies outside `root_canonical`. A target that
/// cannot be resolved (a dangling link) is treated as inside: it carries no
/// external contents to track, so hashing its link text alone stays sound.
fn escaping_symlink_target(
    root_canonical: &Path,
    symlink_path: &Path,
    target: &Path,
) -> Option<PathBuf> {
    let resolved = match symlink_path.parent() {
        Some(parent) if !target.is_absolute() => parent.join(target),
        _ => target.to_path_buf(),
    };
    resolved
        .canonicalize()
        .ok()
        .filter(|canonical| !canonical.starts_with(root_canonical))
}

/// Replace each symlink under `destination` whose counterpart under `source`
/// resolves to a file outside `source` with a copy of that file, so a snapshot
/// of `source` carries the external contents its digest records.
pub fn copy_escaping_symlink_targets(source: &Path, destination: &Path) -> std::io::Result<()> {
    let root_canonical = source.canonicalize()?;
    copy_escaping_symlink_targets_in(&root_canonical, source, destination)
}

fn copy_escaping_symlink_targets_in(
    root_canonical: &Path,
    source: &Path,
    destination: &Path,
) -> std::io::Result<()> {
    for child in std::fs::read_dir(source)? {
        let child = child?;
        let path = child.path();
        let file_type = child.file_type()?;
        let copied = destination.join(child.file_name());
        if file_type.is_dir() {
            copy_escaping_symlink_targets_in(root_canonical, &path, &copied)?;
        } else if file_type.is_symlink() {
            let target = std::fs::read_link(&path)?;
            if let Some(resolved) = escaping_symlink_target(root_canonical, &path, &target) {
                if resolved.is_file() {
                    std::fs::remove_file(&copied)?;
                    std::fs::copy(&resolved, &copied)?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn utf8_path(path: &Path) -> std::io::Result<&str> {
    path.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("host tree path `{}` is not valid UTF-8", path.display()),
        )
    })
}

pub(crate) fn portable_path(path: &Path) -> std::io::Result<String> {
    let path = utf8_path(path)?;
    if cfg!(windows) {
        Ok(path.replace('\\', "/"))
    } else {
        Ok(path.to_string())
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
pub(crate) fn host_file_mode(_metadata: &std::fs::Metadata) -> u32 {
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
    fn external_file_symlink_is_hashed_and_copied_as_its_target() {
        let outside = tempfile::TempDir::new().unwrap();
        let external = outside.path().join("LICENSE");
        std::fs::write(&external, b"external payload").unwrap();

        let linked = tempfile::TempDir::new().unwrap();
        std::os::unix::fs::symlink(&external, linked.path().join("LICENSE")).unwrap();
        let copied = tempfile::TempDir::new().unwrap();
        std::fs::copy(&external, copied.path().join("LICENSE")).unwrap();

        let linked_digest = host_tree_sha256_hex(linked.path()).unwrap();
        assert_eq!(linked_digest, host_tree_sha256_hex(copied.path()).unwrap());

        std::fs::write(&external, b"changed payload").unwrap();
        assert_ne!(linked_digest, host_tree_sha256_hex(linked.path()).unwrap());

        let destination = tempfile::TempDir::new().unwrap();
        std::os::unix::fs::symlink(&external, destination.path().join("LICENSE")).unwrap();
        copy_escaping_symlink_targets(linked.path(), destination.path()).unwrap();
        let restored = destination.path().join("LICENSE");
        assert!(!std::fs::symlink_metadata(&restored)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(&restored).unwrap(), b"changed payload");
        assert_eq!(
            host_tree_sha256_hex(linked.path()).unwrap(),
            host_tree_sha256_hex(destination.path()).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn external_directory_symlink_is_rejected() {
        let outside = tempfile::TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"external payload").unwrap();

        let tmp = tempfile::TempDir::new().unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("linked")).unwrap();

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

    #[cfg(unix)]
    #[test]
    fn literal_backslash_does_not_alias_a_directory_separator() {
        let flat = tempfile::TempDir::new().unwrap();
        let nested = tempfile::TempDir::new().unwrap();
        for root in [flat.path(), nested.path()] {
            std::fs::create_dir(root.join("a")).unwrap();
        }
        std::fs::write(flat.path().join("a\\b"), b"payload").unwrap();
        std::fs::write(nested.path().join("a/b"), b"payload").unwrap();

        assert_ne!(
            host_tree_sha256_hex(flat.path()).unwrap(),
            host_tree_sha256_hex(nested.path()).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_are_rejected_instead_of_hashed_lossily() {
        use std::os::unix::ffi::OsStringExt;

        let name = std::ffi::OsString::from_vec(vec![0xff]);
        assert_eq!(
            portable_path(Path::new(&name)).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
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
