use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use struo_cas::Digest;

use crate::digest_writer::DigestWriter;

pub(super) fn digest_file(
    path: &Path,
    metadata: &std::fs::Metadata,
    stop: &AtomicBool,
) -> io::Result<Digest> {
    let mut writer = Writer {
        digest: DigestWriter::default(),
        stop,
    };
    writer.check()?;
    crate::file_blob::write_file_content(path, metadata, &mut writer)?;
    Ok(writer.digest.finish())
}

struct Writer<'a> {
    digest: DigestWriter,
    stop: &'a AtomicBool,
}

impl Writer<'_> {
    fn check(&self) -> io::Result<()> {
        if self.stop.load(Ordering::Relaxed) {
            return Err(io::Error::other("legacy output validation stopped"));
        }
        Ok(())
    }
}

impl Write for Writer<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.check()?;
        self.digest.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.digest.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncancelled_hash_preserves_raw_identity_across_buffer_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file");
        for size in [0, 1, 8 * 1024, 256 * 1024, 1024 * 1024 + 1] {
            let bytes = (0..size)
                .map(|i| u8::try_from(i % 251).unwrap())
                .collect::<Vec<_>>();
            std::fs::write(&path, &bytes).unwrap();
            let metadata = std::fs::metadata(&path).unwrap();
            assert_eq!(
                digest_file(&path, &metadata, &AtomicBool::new(false)).unwrap(),
                Digest::of_bytes(&bytes)
            );
        }
    }

    #[test]
    fn cancellation_stops_before_opening_and_between_chunks() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file");
        std::fs::write(&path, b"bytes").unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let stop = AtomicBool::new(true);
        let error = digest_file(&path, &metadata, &stop).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);

        stop.store(false, Ordering::Relaxed);
        let mut writer = Writer {
            digest: DigestWriter::default(),
            stop: &stop,
        };
        writer.write_all(b"first chunk").unwrap();
        stop.store(true, Ordering::Relaxed);
        assert_eq!(
            writer.write_all(b"second chunk").unwrap_err().kind(),
            io::ErrorKind::Other
        );
        assert_eq!(writer.digest.finish(), Digest::of_bytes(b"first chunk"));
    }
}
