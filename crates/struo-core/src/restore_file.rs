use std::io::{Read, Write};
use std::path::Path;

const COPY_BUFFER_LIMIT: usize = 512 * 1024;
const COPY_BUFFER_MIN: usize = 8 * 1024;

pub(crate) fn restore(
    path: &Path,
    reader: impl Read,
    mode: u32,
    expected_len: Option<u64>,
) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        std::fs::create_dir_all(parent)?;
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    write_content(temporary.as_file_mut(), reader, mode, expected_len)?;
    match temporary.persist(path) {
        Ok(_) => Ok(()),
        Err(error) => {
            #[cfg(windows)]
            if error.error.kind() == std::io::ErrorKind::PermissionDenied {
                let metadata = std::fs::symlink_metadata(path)?;
                if metadata.is_file() && metadata.permissions().readonly() {
                    let mut permissions = metadata.permissions();
                    permissions.set_readonly(false);
                    std::fs::set_permissions(path, permissions)?;
                    return error
                        .file
                        .persist(path)
                        .map(|_| ())
                        .map_err(|error| error.error);
                }
            }
            Err(error.error)
        }
    }
}

pub(crate) fn write_content(
    file: &mut std::fs::File,
    reader: impl Read,
    mode: u32,
    expected_len: Option<u64>,
) -> std::io::Result<()> {
    copy_content(file, reader, expected_len)?;
    set_mode(file, mode)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub(crate) fn write_private_content(
    file: &mut std::fs::File,
    reader: impl Read,
    mode: u32,
    expected_len: Option<u64>,
) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    copy_content(file, reader, expected_len)?;
    if file.metadata()?.permissions().mode() & 0o7777 != mode & 0o7777 {
        set_mode(file, mode)?;
    }
    Ok(())
}

fn copy_content(
    file: &mut std::fs::File,
    reader: impl Read,
    expected_len: Option<u64>,
) -> std::io::Result<()> {
    let capacity = expected_len
        .and_then(|len| usize::try_from(len).ok())
        .unwrap_or(COPY_BUFFER_LIMIT)
        .clamp(COPY_BUFFER_MIN, COPY_BUFFER_LIMIT);
    let mut reader = std::io::BufReader::with_capacity(capacity, reader);
    let copied = std::io::copy(&mut reader, file)?;
    if expected_len.is_some_and(|expected| copied != expected) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "truncated entry content",
        ));
    }
    file.flush()
}

fn set_mode(file: &std::fs::File, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let mut permissions = file.metadata()?.permissions();
        permissions.set_readonly(mode & 0o222 == 0);
        file.set_permissions(permissions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};

    #[test]
    fn missing_nested_parents_are_created() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("missing/nested/output");
        restore(&path, Cursor::new(b"content"), 0o644, Some(7)).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"content");
    }

    #[test]
    fn a_file_parent_is_not_replaced_by_a_directory() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("parent");
        std::fs::write(&parent, b"keep this file").unwrap();
        assert!(restore(
            &parent.join("output"),
            Cursor::new(b"content"),
            0o644,
            Some(7)
        )
        .is_err());
        assert_eq!(std::fs::read(parent).unwrap(), b"keep this file");
    }

    #[test]
    fn truncated_content_preserves_existing_output_and_cleans_temporary_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("output");
        std::fs::write(&path, b"original").unwrap();
        assert!(restore(&path, Cursor::new(b"short"), 0o644, Some(10)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn read_error_after_full_buffers_preserves_existing_output() {
        struct Failed;
        impl Read for Failed {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("interrupted reader"))
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("output");
        std::fs::write(&path, b"original").unwrap();
        let reader = Cursor::new(vec![42; COPY_BUFFER_LIMIT * 2]).chain(Failed);
        assert!(restore(&path, reader, 0o644, None).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn buffering_does_not_read_past_a_directory_entry_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("output");
        let mut reader = Cursor::new(b"entry-next-entry");
        restore(&path, reader.by_ref().take(5), 0o640, Some(5)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"entry");
        let mut remainder = Vec::new();
        reader.read_to_end(&mut remainder).unwrap();
        assert_eq!(remainder, b"-next-entry");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
    }
}
