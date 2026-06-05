//! Binary encoding for directory-shaped action outputs.
//!
//! When an action's declared output is a directory (e.g. an `ebin/`
//! tree or a `.app` bundle), the runner serializes every file inside
//! into one CAS blob. The format is a magic prefix followed by
//! length-tagged entries (path length, mode, content length, then the
//! path bytes and the content bytes). Restoring is the inverse walk:
//! recreate the directory at the declared output path with each
//! entry's contents and unix permission bits.

use std::path::Path;

use crate::path::WorkspacePath;
use crate::{Error, Result};

pub(crate) const DIRECTORY_BLOB_MAGIC: &[u8] = b"once.directory.v1\0";

pub(crate) fn capture_directory_blob(root: &Path) -> std::io::Result<Vec<u8>> {
    let mut files = Vec::new();
    collect_directory_files(root, root, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = Vec::from(DIRECTORY_BLOB_MAGIC);
    for (rel, mode, bytes) in files {
        let path_bytes = rel.as_bytes();
        let path_len = u32::try_from(path_bytes.len()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "directory path is too long",
            )
        })?;
        let content_len = u64::try_from(bytes.len()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "directory file is too large",
            )
        })?;
        out.extend_from_slice(&path_len.to_le_bytes());
        out.extend_from_slice(&mode.to_le_bytes());
        out.extend_from_slice(&content_len.to_le_bytes());
        out.extend_from_slice(path_bytes);
        out.extend_from_slice(&bytes);
    }
    Ok(out)
}

fn collect_directory_files(
    root: &Path,
    dir: &Path,
    files: &mut Vec<(String, u32, Vec<u8>)>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            collect_directory_files(root, &path, files)?;
        } else if metadata.is_file() {
            let rel = path
                .strip_prefix(root)
                .expect("directory walk stays under root")
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode() & 0o777
            };
            #[cfg(not(unix))]
            let mode = 0o644;
            files.push((rel, mode, std::fs::read(&path)?));
        }
    }
    Ok(())
}

pub(crate) fn restore_directory_blob(logical_path: &str, abs: &Path, bytes: &[u8]) -> Result<()> {
    let files = decode_directory_blob(logical_path, bytes)?;
    if abs.exists() {
        let metadata = std::fs::metadata(abs).map_err(|source| Error::RestoreOutput {
            path: logical_path.to_string(),
            source,
        })?;
        if metadata.is_dir() {
            std::fs::remove_dir_all(abs).map_err(|source| Error::RestoreOutput {
                path: logical_path.to_string(),
                source,
            })?;
        } else {
            std::fs::remove_file(abs).map_err(|source| Error::RestoreOutput {
                path: logical_path.to_string(),
                source,
            })?;
        }
    }
    std::fs::create_dir_all(abs).map_err(|source| Error::RestoreOutput {
        path: logical_path.to_string(),
        source,
    })?;
    for (rel, mode, content) in files {
        let rel_path = WorkspacePath::try_from(rel.as_str()).map_err(|source| {
            Error::InvalidDirectoryOutput {
                path: logical_path.to_string(),
                message: source.to_string(),
            }
        })?;
        if rel_path.as_str().is_empty() {
            return Err(Error::InvalidDirectoryOutput {
                path: logical_path.to_string(),
                message: "directory entry path is empty".to_string(),
            });
        }
        let dest = rel_path.resolve(abs);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::RestoreOutput {
                path: logical_path.to_string(),
                source,
            })?;
        }
        std::fs::write(&dest, content).map_err(|source| Error::RestoreOutput {
            path: logical_path.to_string(),
            source,
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(mode.max(0o400)))
                .map_err(|source| Error::RestoreOutput {
                    path: logical_path.to_string(),
                    source,
                })?;
        }
    }
    Ok(())
}

fn decode_directory_blob(logical_path: &str, bytes: &[u8]) -> Result<Vec<(String, u32, Vec<u8>)>> {
    let mut pos = DIRECTORY_BLOB_MAGIC.len();
    let mut files = Vec::new();
    while pos < bytes.len() {
        let path_len = usize::try_from(read_u32(logical_path, bytes, &mut pos)?).map_err(|_| {
            Error::InvalidDirectoryOutput {
                path: logical_path.to_string(),
                message: "entry path length does not fit usize".to_string(),
            }
        })?;
        let mode = read_u32(logical_path, bytes, &mut pos)?;
        let content_len =
            usize::try_from(read_u64(logical_path, bytes, &mut pos)?).map_err(|_| {
                Error::InvalidDirectoryOutput {
                    path: logical_path.to_string(),
                    message: "entry content length does not fit usize".to_string(),
                }
            })?;
        if bytes.len().saturating_sub(pos) < path_len {
            return Err(Error::InvalidDirectoryOutput {
                path: logical_path.to_string(),
                message: "truncated entry path".to_string(),
            });
        }
        let path_bytes = &bytes[pos..pos + path_len];
        pos += path_len;
        let path = std::str::from_utf8(path_bytes)
            .map_err(|e| Error::InvalidDirectoryOutput {
                path: logical_path.to_string(),
                message: format!("entry path is not utf-8: {e}"),
            })?
            .to_string();
        if bytes.len().saturating_sub(pos) < content_len {
            return Err(Error::InvalidDirectoryOutput {
                path: logical_path.to_string(),
                message: "truncated entry content".to_string(),
            });
        }
        let content = bytes[pos..pos + content_len].to_vec();
        pos += content_len;
        files.push((path, mode, content));
    }
    Ok(files)
}

fn read_u32(logical_path: &str, bytes: &[u8], pos: &mut usize) -> Result<u32> {
    if bytes.len().saturating_sub(*pos) < 4 {
        return Err(Error::InvalidDirectoryOutput {
            path: logical_path.to_string(),
            message: "truncated u32".to_string(),
        });
    }
    let mut raw = [0u8; 4];
    raw.copy_from_slice(&bytes[*pos..*pos + 4]);
    *pos += 4;
    Ok(u32::from_le_bytes(raw))
}

fn read_u64(logical_path: &str, bytes: &[u8], pos: &mut usize) -> Result<u64> {
    if bytes.len().saturating_sub(*pos) < 8 {
        return Err(Error::InvalidDirectoryOutput {
            path: logical_path.to_string(),
            message: "truncated u64".to_string(),
        });
    }
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[*pos..*pos + 8]);
    *pos += 8;
    Ok(u64::from_le_bytes(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[cfg(unix)]
    #[test]
    fn directory_blob_restores_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("source");
        let restored = tmp.path().join("restored");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("tool"), b"run").unwrap();
        std::fs::set_permissions(source.join("tool"), std::fs::Permissions::from_mode(0o750))
            .unwrap();

        let blob = capture_directory_blob(&source).unwrap();
        restore_directory_blob("bundle", &restored, &blob).unwrap();

        let mode = std::fs::metadata(restored.join("tool"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o750);
    }
}
