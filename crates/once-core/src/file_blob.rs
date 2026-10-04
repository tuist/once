//! Binary encoding for file-shaped action outputs.
//!
//! Raw file bytes alone cannot preserve Unix permission bits on restore.
//! This format stores the mode beside the contents while keeping directory
//! output encoding separate.

use std::io::Read;
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

/// Digest a source file for use as an action input. It matches
/// [`digest_file_blob`] for every file whose mode has no special bits, and also
/// covers the setuid, setgid, and sticky bits, which an action that runs on the
/// host can observe.
pub(crate) fn digest_source_file(
    path: &Path,
    metadata: &std::fs::Metadata,
) -> std::io::Result<Digest> {
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o7777
    };
    #[cfg(not(unix))]
    let mode = 0o644_u32;
    let mut header = Vec::with_capacity(FILE_BLOB_MAGIC.len() + 4);
    header.extend_from_slice(FILE_BLOB_MAGIC);
    header.extend_from_slice(&mode.to_le_bytes());
    let file = std::fs::File::open(path)?;
    Digest::of_parts_and_reader(&[&header], file)
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
    crate::restore_file::restore(abs, reader, mode, None).map_err(|source| Error::RestoreOutput {
        path: logical_path.to_string(),
        source,
    })
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
    fn restoring_a_file_preserves_existing_readers() {
        use std::io::Seek;

        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("output");
        std::fs::write(&path, b"original output").unwrap();
        let mut reader = std::fs::File::open(&path).unwrap();
        let mut blob = file_blob_header(&std::fs::metadata(&path).unwrap());
        blob.extend_from_slice(b"replacement output");

        restore_file_blob_from_reader("output", &path, std::io::Cursor::new(blob)).unwrap();

        reader.rewind().unwrap();
        let mut original = Vec::new();
        reader.read_to_end(&mut original).unwrap();
        assert_eq!(original, b"original output");
        assert_eq!(std::fs::read(path).unwrap(), b"replacement output");
    }

    #[test]
    fn failed_file_restore_preserves_the_existing_output() {
        struct FailedReader;
        impl Read for FailedReader {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("interrupted blob read"))
            }
        }

        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("output");
        std::fs::write(&path, b"original output").unwrap();
        let header = file_blob_header(&std::fs::metadata(&path).unwrap());
        let reader = std::io::Cursor::new(header).chain(FailedReader);

        assert!(restore_file_blob_from_reader("output", &path, reader).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"original output");
    }

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

    #[test]
    #[cfg(unix)]
    fn restore_replaces_a_read_only_output_left_by_an_earlier_restore() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let restored = dir.path().join("restored");
        std::fs::write(&source, b"signed").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o555)).unwrap();
        let blob = capture_file_blob(&source).unwrap();

        restore_file_blob_from_reader("restored", &restored, blob.as_slice()).unwrap();
        restore_file_blob_from_reader("restored", &restored, blob.as_slice()).unwrap();

        assert_eq!(std::fs::read(restored).unwrap(), b"signed");
    }
}
