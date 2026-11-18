use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use rustix::fs::{self, FileType, Mode, OFlags};
use std::os::unix::ffi::OsStrExt;

mod parent;
#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(in crate::remote) struct Directory(Arc<File>);

#[derive(Clone)]
pub(in crate::remote) struct Parent {
    directory: Directory,
    name: OsString,
}

pub(in crate::remote) struct Metadata {
    pub kind: FileType,
    pub len: u64,
}

impl Metadata {
    pub fn is_dir(&self) -> bool {
        self.kind == FileType::Directory
    }

    pub fn is_file(&self) -> bool {
        self.kind == FileType::RegularFile
    }

    pub fn is_symlink(&self) -> bool {
        self.kind == FileType::Symlink
    }
}

impl Directory {
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self(Arc::new(
            fs::open(path, directory_flags(), Mode::empty())?.into(),
        )))
    }

    pub fn filesystem_metadata(&self) -> io::Result<std::fs::Metadata> {
        self.0.metadata()
    }

    pub fn descend(&self, path: &Path, create: bool) -> io::Result<Self> {
        let mut directory = self.clone();
        for component in path.components() {
            let name = match component {
                Component::Normal(name) => name,
                Component::CurDir => continue,
                _ => return Err(invalid_path()),
            };
            if create {
                match fs::mkdirat(&directory.0, name, Mode::from_raw_mode(0o755)) {
                    Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            directory = Self(Arc::new(
                fs::openat(&directory.0, name, directory_flags(), Mode::empty())?.into(),
            ));
        }
        Ok(directory)
    }

    pub fn parent(&self, path: &Path, create: bool) -> io::Result<Parent> {
        // Check all components before performing even the first filesystem operation.
        if path
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
        {
            return Err(invalid_path());
        }
        let name = path.file_name().ok_or_else(invalid_path)?.to_owned();
        let directory = self.descend(path.parent().unwrap_or(Path::new("")), create)?;
        Ok(Parent { directory, name })
    }

    pub fn metadata(&self, path: &Path) -> io::Result<Metadata> {
        if path.as_os_str().is_empty() {
            return metadata(fs::fstat(&self.0)?);
        }
        self.parent(path, false)?.metadata()
    }

    pub fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        let parent = self.parent(path, false)?;
        let target = fs::readlinkat(&parent.directory.0, &parent.name, Vec::new())?;
        Ok(PathBuf::from(OsStr::from_bytes(target.to_bytes())))
    }

    pub fn entries(&self) -> io::Result<impl Iterator<Item = io::Result<OsString>>> {
        Ok(
            fs::Dir::read_from(&self.0)?.filter_map(|entry| match entry {
                Ok(entry)
                    if entry.file_name().to_bytes() == b"."
                        || entry.file_name().to_bytes() == b".." =>
                {
                    None
                }
                Ok(entry) => Some(Ok(
                    OsStr::from_bytes(entry.file_name().to_bytes()).to_owned()
                )),
                Err(error) => Some(Err(error.into())),
            }),
        )
    }

    pub fn open_file(&self, path: &Path) -> io::Result<File> {
        let parent = self.parent(path, false)?;
        let file: File = fs::openat(
            &parent.directory.0,
            &parent.name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?
        .into();
        if !file.metadata()?.is_file() {
            return Err(invalid_path());
        }
        Ok(file)
    }

    pub fn create_file(&self, path: &Path, mode: u32) -> io::Result<File> {
        let parent = self.parent(path, true)?;
        let mode = Mode::from_raw_mode((mode & 0o777) as _);
        let file: File = fs::openat(
            &parent.directory.0,
            &parent.name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            mode,
        )?
        .into();
        fs::fchmod(&file, mode)?;
        Ok(file)
    }

    pub fn open_or_create_file(&self, path: &Path) -> io::Result<File> {
        let parent = self.parent(path, true)?;
        let file: File = fs::openat(
            &parent.directory.0,
            &parent.name,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?
        .into();
        if !file.metadata()?.is_file() {
            return Err(invalid_path());
        }
        Ok(file)
    }

    pub fn set_directory_mode(&self, path: &Path, mode: u32) -> io::Result<()> {
        let directory = self.descend(path, false)?;
        Ok(fs::fchmod(
            &directory.0,
            Mode::from_raw_mode((mode & 0o777) as _),
        )?)
    }

    pub fn symlink(&self, path: &Path, target: &Path) -> io::Result<()> {
        let parent = self.parent(path, true)?;
        Ok(fs::symlinkat(target, &parent.directory.0, &parent.name)?)
    }

    pub fn mkdir(&self, name: &Path) -> io::Result<()> {
        let parent = self.parent(name, false)?;
        Ok(fs::mkdirat(
            &parent.directory.0,
            &parent.name,
            Mode::from_raw_mode(0o700),
        )?)
    }
}

fn metadata(stat: fs::Stat) -> io::Result<Metadata> {
    Ok(Metadata {
        kind: FileType::from_raw_mode(stat.st_mode),
        len: u64::try_from(stat.st_size).map_err(|_| invalid_path())?,
    })
}

fn directory_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

fn invalid_path() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "remote path must traverse only real directories beneath its root",
    )
}
