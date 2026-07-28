//! Binary encoding for file-shaped action outputs.
//!
//! Raw file bytes alone cannot preserve Unix permission bits on restore.
//! This format stores the mode beside the contents while keeping directory
//! output encoding separate.

use std::io::{Read, Write};
use std::path::Path;

use once_cas::Digest;

use crate::{Error, Result};

pub(crate) const FILE_BLOB_MAGIC: &[u8] = b"once.file.v1\0";

#[cfg(test)]
pub(crate) fn capture_file_blob(path: &Path) -> std::io::Result<Vec<u8>> {
    let metadata = std::fs::metadata(path)?;
    let header = file_blob_header(&metadata);
    let content = std::fs::read(path)?;
    let mut out = Vec::with_capacity(header.len() + content.len());
    out.extend_from_slice(&header);
    out.extend_from_slice(&content);
    Ok(out)
}

pub(crate) fn file_blob_header(metadata: &std::fs::Metadata) -> Vec<u8> {
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777
    };
    #[cfg(not(unix))]
    let mode = 0o644_u32;

    let mut header = Vec::with_capacity(FILE_BLOB_MAGIC.len() + 4);
    header.extend_from_slice(FILE_BLOB_MAGIC);
    header.extend_from_slice(&mode.to_le_bytes());
    header
}

pub(crate) fn digest_file_blob(
    path: &Path,
    metadata: &std::fs::Metadata,
) -> std::io::Result<Digest> {
    let header = file_blob_header(metadata);
    let file = std::fs::File::open(path)?;
    Digest::of_parts_and_reader(&[&header], file)
}

pub(crate) fn restore_file_blob_from_reader(
    logical_path: &str,
    abs: &Path,
    mut reader: impl Read,
) -> Result<()> {
    let mut header = [0_u8; FILE_BLOB_MAGIC.len() + 4];
    let mut filled = 0;
    while filled < header.len() {
        match reader.read(&mut header[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(source) => {
                return Err(Error::RestoreOutput {
                    path: logical_path.to_string(),
                    source,
                });
            }
        }
    }
    let (mode, _) = decode_file_blob(logical_path, &header[..filled])?;
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::RestoreOutput {
            path: logical_path.to_string(),
            source,
        })?;
    }
    let mut output = std::fs::File::create(abs).map_err(|source| Error::RestoreOutput {
        path: logical_path.to_string(),
        source,
    })?;
    std::io::copy(&mut reader, &mut output).map_err(|source| Error::RestoreOutput {
        path: logical_path.to_string(),
        source,
    })?;
    output.flush().map_err(|source| Error::RestoreOutput {
        path: logical_path.to_string(),
        source,
    })?;
    drop(output);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(abs, std::fs::Permissions::from_mode(mode)).map_err(|source| {
            Error::RestoreOutput {
                path: logical_path.to_string(),
                source,
            }
        })?;
    }
    #[cfg(not(unix))]
    {
        let mut permissions = std::fs::metadata(abs)
            .map_err(|source| Error::RestoreOutput {
                path: logical_path.to_string(),
                source,
            })?
            .permissions();
        permissions.set_readonly(mode & 0o222 == 0);
        std::fs::set_permissions(abs, permissions).map_err(|source| Error::RestoreOutput {
            path: logical_path.to_string(),
            source,
        })?;
    }
    Ok(())
}

fn decode_file_blob<'a>(logical_path: &str, bytes: &'a [u8]) -> Result<(u32, &'a [u8])> {
    if !bytes.starts_with(FILE_BLOB_MAGIC) {
        return Err(Error::InvalidFileOutput {
            path: logical_path.to_string(),
            message: "missing file blob magic".to_string(),
        });
    }
    let mode_bytes = bytes
        .get(FILE_BLOB_MAGIC.len()..FILE_BLOB_MAGIC.len() + 4)
        .ok_or_else(|| Error::InvalidFileOutput {
            path: logical_path.to_string(),
            message: "truncated file mode".to_string(),
        })?;
    let content = bytes.get(FILE_BLOB_MAGIC.len() + 4..).unwrap_or_default();
    let mut raw_mode = [0u8; 4];
    raw_mode.copy_from_slice(mode_bytes);
    Ok((u32::from_le_bytes(raw_mode) & 0o777, content))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn decode_rejects_missing_magic() {
        let error = decode_file_blob("out/file", b"raw").unwrap_err();

        assert!(matches!(error, Error::InvalidFileOutput { .. }));
        assert!(error.to_string().contains("missing file blob magic"));
    }

    #[test]
    fn decode_rejects_truncated_mode() {
        let mut bytes = Vec::from(FILE_BLOB_MAGIC);
        bytes.extend_from_slice(&[1, 2, 3]);

        let error = decode_file_blob("out/file", &bytes).unwrap_err();

        assert!(matches!(error, Error::InvalidFileOutput { .. }));
        assert!(error.to_string().contains("truncated file mode"));
    }

    #[test]
    fn streaming_digest_matches_captured_file_blob() {
        let tmp = TempDir::new().unwrap();
        for (name, bytes) in [
            ("empty", Vec::new()),
            ("small", b"x".to_vec()),
            ("large", vec![b'x'; 1024 * 1024]),
        ] {
            let path = tmp.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            let metadata = std::fs::metadata(&path).unwrap();

            assert_eq!(
                digest_file_blob(&path, &metadata).unwrap(),
                Digest::of_bytes(&capture_file_blob(&path).unwrap())
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn streaming_digest_matches_non_default_file_modes() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("mode");
        std::fs::write(&path, b"content").unwrap();
        for mode in [0o600, 0o755] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            let metadata = std::fs::metadata(&path).unwrap();

            assert_eq!(
                digest_file_blob(&path, &metadata).unwrap(),
                Digest::of_bytes(&capture_file_blob(&path).unwrap())
            );
        }
    }

    #[test]
    fn streaming_restore_matches_buffered_restore() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("source");
        let restored = tmp.path().join("restored");
        let bytes = vec![b'x'; 1024 * 1024];
        std::fs::write(&source, &bytes).unwrap();
        let blob = capture_file_blob(&source).unwrap();

        restore_file_blob_from_reader("restored", &restored, blob.as_slice()).unwrap();

        assert_eq!(std::fs::read(restored).unwrap(), bytes);
    }
}
