use std::io::{self, Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::Cas;
use crate::{blob, Digest, Error, Result};

const MAX_RAW_PINS: usize = 64;
// Prefetch can retain many completed leases, so excess pins spill rather than wait.
static RAW_PINS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(MAX_RAW_PINS);

/// An owned, read-only staging lease for a decoded blob.
///
/// Raw storage may be pinned without copying. No mutable file handle or path is
/// exposed, and dropping the lease removes its temporary storage.
pub struct StagedBlob {
    backing: Backing,
    directory: Option<tempfile::TempDir>,
}

enum Backing {
    Raw(std::fs::File, tokio::sync::SemaphorePermit<'static>),
    Temporary(PathBuf),
}

impl std::fmt::Debug for StagedBlob {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("StagedBlob").finish_non_exhaustive()
    }
}

impl StagedBlob {
    /// Open a bounded-memory reader, retaining the lease until it is dropped.
    pub fn into_reader(self) -> Result<StagedBlobReader> {
        let (file, pin) = match self.backing {
            Backing::Raw(file, pin) => (file, Some(pin)),
            Backing::Temporary(path) => {
                let file =
                    std::fs::File::open(&path).map_err(|source| Error::Io { path, source })?;
                (file, None)
            }
        };
        Ok(StagedBlobReader {
            file,
            _directory: self.directory,
            _pin: pin,
        })
    }
}

/// Read and seek within a staged blob without exposing mutable storage access.
pub struct StagedBlobReader {
    file: std::fs::File,
    _directory: Option<tempfile::TempDir>,
    _pin: Option<tokio::sync::SemaphorePermit<'static>>,
}

impl Read for StagedBlobReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.file.read(bytes)
    }
}

impl Seek for StagedBlobReader {
    fn seek(&mut self, position: io::SeekFrom) -> io::Result<u64> {
        self.file.seek(position)
    }
}

impl Cas {
    /// Stage a blob for read-only access under `parent`.
    ///
    /// Pins raw storage with a read-only handle and decodes compressed storage.
    pub async fn stage_blob(&self, digest: &Digest, parent: &Path) -> Result<StagedBlob> {
        self.stage_blob_guarded(digest, parent, ()).await
    }

    /// Stage a blob while retaining `guard` through any blocking decoding work.
    ///
    /// Cancellation does not release the guard or temporary directory while a
    /// decoder still uses them. The completed lease does not retain the guard.
    pub async fn stage_blob_guarded<G: Send + 'static>(
        &self,
        digest: &Digest,
        parent: &Path,
        guard: G,
    ) -> Result<StagedBlob> {
        self.stage_blob_with(
            digest,
            parent,
            guard,
            |path| std::fs::File::open(path),
            |reader, writer| {
                let mut reader = io::BufReader::with_capacity(512 * 1024, reader);
                io::copy(&mut reader, writer).map(|_| ())
            },
        )
        .await
    }

    async fn stage_blob_with<G: Send + 'static>(
        &self,
        digest: &Digest,
        parent: &Path,
        guard: G,
        open: fn(&Path) -> io::Result<std::fs::File>,
        copy: impl FnOnce(&mut dyn Read, &mut dyn io::Write) -> io::Result<()> + Send + 'static,
    ) -> Result<StagedBlob> {
        let expected = *digest;
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _stop = blob::StopOnDrop(Arc::clone(&stop));
        let original = self.blob_path(digest);
        let error_path = original.clone();
        let parent = parent.to_path_buf();
        let staged = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let mut file = open(&original)?;
            let mut prefix = Vec::with_capacity(blob::ZSTD_BLOB_MAGIC.len());
            (&mut file)
                .take(blob::ZSTD_BLOB_MAGIC.len() as u64)
                .read_to_end(&mut prefix)?;
            file.rewind()?;
            if prefix != blob::ZSTD_BLOB_MAGIC {
                if let Ok(pin) = RAW_PINS.try_acquire() {
                    let actual = Digest::of_reader(io::BufReader::new(&mut file))?;
                    crate::integrity::verify(&expected, &actual)?;
                    if stop.load(std::sync::atomic::Ordering::Acquire) {
                        return Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "blob staging was cancelled",
                        ));
                    }
                    file.rewind()?;
                    return Ok(StagedBlob {
                        backing: Backing::Raw(file, pin),
                        directory: None,
                    });
                }
            }
            let mut builder = tempfile::Builder::new();
            builder.prefix(".struo-blob-");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                builder.permissions(std::fs::Permissions::from_mode(0o700));
            }
            let directory = builder.tempdir_in(parent)?;
            let destination = directory.path().join("decoded");
            blob::decode_open_file(file, &destination, &expected, &stop, false, copy)?;
            Ok(StagedBlob {
                backing: Backing::Temporary(destination),
                directory: Some(directory),
            })
        })
        .await
        .map_err(|source| Error::Io {
            path: error_path.clone(),
            source: io::Error::other(source.to_string()),
        })?;
        match staged {
            Ok(lease) => Ok(lease),
            Err(source) => {
                if crate::integrity::CorruptBlob::is(&source) {
                    self.quarantine(digest, &self.blob_path(digest)).await;
                }
                if source.kind() == io::ErrorKind::NotFound {
                    Err(Error::BlobNotFound(*digest))
                } else {
                    Err(Error::Io {
                        path: error_path,
                        source,
                    })
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "staged_integrity_tests.rs"]
mod integrity_tests;
#[cfg(test)]
#[path = "staged_tests.rs"]
mod tests;
