//! Blocking filesystem helpers that portable actions run on admitted
//! blocking tasks: copying, removing, linking, and digesting trees.

use std::io::Read as _;
use std::path::Path;

use sha2::Digest as ShaDigest;

pub(super) fn remove_path_blocking(path: &Path) -> std::io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(source),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        remove_tree_blocking(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Remove a directory tree, first restoring owner access to directories whose
/// mode forbids deleting their entries. Sandboxes mirror source permissions,
/// so a read-only source directory must not make its copy undeletable.
pub(super) fn remove_tree_blocking(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(source) if source.kind() == std::io::ErrorKind::PermissionDenied => {
            make_tree_removable(path)?;
            std::fs::remove_dir_all(path)
        }
        other => other,
    }
}

pub(super) fn make_tree_removable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            let mut permissions = metadata.permissions();
            permissions.set_mode(permissions.mode() | 0o700);
            std::fs::set_permissions(path, permissions)?;
            for entry in std::fs::read_dir(path)? {
                make_tree_removable(&entry?.path())?;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

pub(super) fn copy_tree_contents_blocking(
    source: &Path,
    destination: &Path,
) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        let file_name = source.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("source `{}` has no file name", source.display()),
            )
        })?;
        copy_symlink_blocking(source, &destination.join(file_name))?;
        return Ok(());
    }
    if metadata.is_file() {
        let file_name = source.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("source `{}` has no file name", source.display()),
            )
        })?;
        copy_file_blocking(source, &destination.join(file_name))?;
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    copy_directory_contents_blocking(source, destination)
}

pub(super) fn copy_directory_contents_blocking(
    source: &Path,
    destination: &Path,
) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    let mut children = std::fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let child_path = child.path();
        let child_destination = destination.join(child.file_name());
        let metadata = std::fs::symlink_metadata(&child_path)?;
        if metadata.file_type().is_symlink() {
            copy_symlink_blocking(&child_path, &child_destination)?;
        } else if metadata.is_dir() {
            copy_directory_contents_blocking(&child_path, &child_destination)?;
        } else if metadata.is_file() {
            copy_file_blocking(&child_path, &child_destination)?;
        }
    }
    std::fs::set_permissions(
        destination,
        std::fs::symlink_metadata(source)?.permissions(),
    )?;
    Ok(())
}

pub(super) fn copy_symlink_blocking(source: &Path, destination: &Path) -> std::io::Result<()> {
    let target = std::fs::read_link(source)?;
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    remove_path_blocking(destination)?;
    create_symlink_blocking(&target, destination, source)
}

#[cfg(unix)]
pub(super) fn create_symlink_blocking(
    target: &Path,
    destination: &Path,
    _source: &Path,
) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, destination)
}

#[cfg(windows)]
pub(super) fn create_symlink_blocking(
    target: &Path,
    destination: &Path,
    source: &Path,
) -> std::io::Result<()> {
    let target_is_dir = std::fs::metadata(source)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false);
    if target_is_dir {
        std::os::windows::fs::symlink_dir(target, destination)
    } else {
        std::os::windows::fs::symlink_file(target, destination)
    }
}

pub(super) fn copy_file_blocking(source: &Path, destination: &Path) -> std::io::Result<()> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    remove_path_blocking(destination)?;
    std::fs::copy(source, destination)?;
    // The platform copy does not always carry the setuid, setgid, and sticky
    // bits, so set the source's permissions explicitly.
    std::fs::set_permissions(destination, std::fs::metadata(source)?.permissions())?;
    Ok(())
}

pub(super) fn tree_digest_bytes(
    root: &Path,
    include_suffixes: &[String],
) -> std::io::Result<Vec<u8>> {
    let metadata = match std::fs::metadata(root) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(source),
    };
    if !metadata.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    collect_tree_files(root, root, include_suffixes, &mut files)?;
    files.sort();
    let mut out = Vec::new();
    for relative in files {
        let absolute = root.join(&relative);
        let digest = file_sha256_hex(&absolute)?;
        out.extend_from_slice(digest.as_bytes());
        out.extend_from_slice(b"  ");
        out.extend_from_slice(relative.to_string_lossy().replace('\\', "/").as_bytes());
        out.push(b'\n');
    }
    Ok(out)
}

pub(super) fn collect_tree_files(
    root: &Path,
    dir: &Path,
    include_suffixes: &[String],
    files: &mut Vec<std::path::PathBuf>,
) -> std::io::Result<()> {
    let mut children = std::fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let path = child.path();
        let metadata = std::fs::metadata(&path)?;
        if metadata.is_dir() {
            collect_tree_files(root, &path, include_suffixes, files)?;
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(&path);
        let relative_str = relative.to_string_lossy().replace('\\', "/");
        if include_suffixes.is_empty()
            || include_suffixes
                .iter()
                .any(|suffix| relative_str.ends_with(suffix))
        {
            files.push(relative.to_path_buf());
        }
    }
    Ok(())
}

pub(super) fn file_sha256_hex(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match file.read(&mut buf)? {
            0 => return Ok(hex::encode(hasher.finalize())),
            read => hasher.update(&buf[..read]),
        }
    }
}
