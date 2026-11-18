use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(in crate::remote) struct Directory;
#[derive(Clone)]
pub(in crate::remote) struct Parent;
pub(in crate::remote) struct Metadata {
    pub len: u64,
}

impl Metadata {
    pub fn is_dir(&self) -> bool {
        false
    }
    pub fn is_file(&self) -> bool {
        false
    }
    pub fn is_symlink(&self) -> bool {
        false
    }
}

impl Directory {
    pub fn open(_: &Path) -> io::Result<Self> {
        Err(unsupported())
    }
    pub fn filesystem_metadata(&self) -> io::Result<std::fs::Metadata> {
        Err(unsupported())
    }
    pub fn descend(&self, _: &Path, _: bool) -> io::Result<Self> {
        Err(unsupported())
    }
    pub fn parent(&self, _: &Path, _: bool) -> io::Result<Parent> {
        Err(unsupported())
    }
    pub fn metadata(&self, _: &Path) -> io::Result<Metadata> {
        Err(unsupported())
    }
    pub fn read_link(&self, _: &Path) -> io::Result<PathBuf> {
        Err(unsupported())
    }
    pub fn entries(&self) -> io::Result<std::vec::IntoIter<io::Result<OsString>>> {
        Err(unsupported())
    }
    pub fn open_file(&self, _: &Path) -> io::Result<File> {
        Err(unsupported())
    }
    pub fn create_file(&self, _: &Path, _: u32) -> io::Result<File> {
        Err(unsupported())
    }
    pub fn open_or_create_file(&self, _: &Path) -> io::Result<File> {
        Err(unsupported())
    }
    pub fn set_directory_mode(&self, _: &Path, _: u32) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn symlink(&self, _: &Path, _: &Path) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn mkdir(&self, _: &Path) -> io::Result<()> {
        Err(unsupported())
    }
}

impl Parent {
    pub fn metadata(&self) -> io::Result<Metadata> {
        Err(unsupported())
    }
    pub fn rename_to(&self, _: &Self) -> io::Result<()> {
        Err(unsupported())
    }
    pub fn remove(&self) -> io::Result<()> {
        Err(unsupported())
    }
}

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "race-safe remote filesystem access is unavailable on this platform",
    )
}
