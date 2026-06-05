//! Binary encoding for file-shaped action outputs.
//!
//! Raw file bytes alone cannot preserve Unix permission bits on restore.
//! This format stores the mode beside the contents while keeping directory
//! output encoding separate.

use std::path::Path;

use crate::{Error, Result};

pub(crate) const FILE_BLOB_MAGIC: &[u8] = b"once.file.v1\0";

pub(crate) fn capture_file_blob(path: &Path) -> std::io::Result<Vec<u8>> {
    let metadata = std::fs::metadata(path)?;
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777
    };
    #[cfg(not(unix))]
    let mode = 0o644_u32;

    let content = std::fs::read(path)?;
    let mut out = Vec::with_capacity(FILE_BLOB_MAGIC.len() + 4 + content.len());
    out.extend_from_slice(FILE_BLOB_MAGIC);
    out.extend_from_slice(&mode.to_le_bytes());
    out.extend_from_slice(&content);
    Ok(out)
}

pub(crate) fn restore_file_blob(logical_path: &str, abs: &Path, bytes: &[u8]) -> Result<()> {
    let (mode, content) = decode_file_blob(logical_path, bytes)?;
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::RestoreOutput {
            path: logical_path.to_string(),
            source,
        })?;
    }
    std::fs::write(abs, content).map_err(|source| Error::RestoreOutput {
        path: logical_path.to_string(),
        source,
    })?;
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
    Ok(())
}

fn decode_file_blob<'a>(logical_path: &str, bytes: &'a [u8]) -> Result<(u32, &'a [u8])> {
    if !bytes.starts_with(FILE_BLOB_MAGIC) {
        return Err(Error::InvalidFileOutput {
            path: logical_path.to_string(),
            message: "missing file blob magic".to_string(),
        });
    }
    let mode_start = FILE_BLOB_MAGIC.len();
    let content_start = mode_start + 4;
    if bytes.len() < content_start {
        return Err(Error::InvalidFileOutput {
            path: logical_path.to_string(),
            message: "truncated file mode".to_string(),
        });
    }
    let mut raw_mode = [0u8; 4];
    raw_mode.copy_from_slice(&bytes[mode_start..content_start]);
    Ok((
        u32::from_le_bytes(raw_mode) & 0o777,
        &bytes[content_start..],
    ))
}
