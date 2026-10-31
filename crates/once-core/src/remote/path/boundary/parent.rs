use std::io;
use std::path::Path;

use rustix::fs::{self, AtFlags};

use super::{metadata, Metadata, Parent};

impl Parent {
    pub fn metadata(&self) -> io::Result<Metadata> {
        metadata(fs::statat(
            &self.directory.0,
            &self.name,
            AtFlags::SYMLINK_NOFOLLOW,
        )?)
    }

    pub fn rename_to(&self, destination: &Self) -> io::Result<()> {
        Ok(fs::renameat(
            &self.directory.0,
            &self.name,
            &destination.directory.0,
            &destination.name,
        )?)
    }

    pub fn remove(&self) -> io::Result<()> {
        let metadata = match self.metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let flags = if metadata.is_dir() {
            let directory = self.directory.descend(Path::new(&self.name), false)?;
            for name in directory.entries()? {
                directory.parent(Path::new(&name?), false)?.remove()?;
            }
            AtFlags::REMOVEDIR
        } else {
            AtFlags::empty()
        };
        Ok(fs::unlinkat(&self.directory.0, &self.name, flags)?)
    }
}
