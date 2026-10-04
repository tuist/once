use std::io::{Read, Write};
use std::path::Path;

pub(crate) fn restore(
    path: &Path,
    mut reader: impl Read,
    mode: u32,
    expected_len: Option<u64>,
) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let copied = std::io::copy(&mut reader, &mut temporary)?;
    if expected_len.is_some_and(|expected| copied != expected) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "truncated entry content",
        ));
    }
    temporary.flush()?;
    set_mode(temporary.as_file(), mode)?;
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
