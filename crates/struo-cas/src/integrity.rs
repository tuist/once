//! Content verification for blobs read back from a store.

use std::io::{self, Write};

use crate::Digest;

/// Stored content did not hash to the digest it is addressed by.
#[derive(Debug, thiserror::Error)]
#[error("blob {digest} failed its integrity check: {detail}")]
pub(crate) struct CorruptBlob {
    digest: Digest,
    detail: String,
}

impl CorruptBlob {
    pub(crate) fn io_error(digest: Digest, detail: &str) -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            Self {
                digest,
                detail: detail.to_string(),
            },
        )
    }

    pub(crate) fn is(error: &io::Error) -> bool {
        error
            .get_ref()
            .is_some_and(<dyn std::error::Error + Send + Sync>::is::<Self>)
    }
}

/// Hashes everything written through it, so a copy is verified in the same
/// pass that writes it.
pub(crate) struct HashingWriter<W> {
    inner: W,
    hasher: blake3::Hasher,
}

impl<W: Write> HashingWriter<W> {
    pub(crate) fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: blake3::Hasher::new(),
        }
    }

    pub(crate) fn digest(&self) -> Digest {
        Digest::from_bytes(*self.hasher.finalize().as_bytes())
    }

    pub(crate) fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.hasher.update(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub(crate) fn verify(expected: &Digest, actual: &Digest) -> io::Result<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(CorruptBlob::io_error(
            *expected,
            &format!("content hashes to {actual}"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashing_writer_matches_a_one_shot_digest() {
        let mut writer = HashingWriter::new(Vec::new());
        writer.write_all(b"hello ").unwrap();
        writer.write_all(b"world").unwrap();
        assert_eq!(writer.digest(), Digest::of_bytes(b"hello world"));
        assert_eq!(writer.inner, b"hello world");
    }

    #[test]
    fn verification_failures_are_recognizable_corruption() {
        let expected = Digest::of_bytes(b"expected");
        assert!(verify(&expected, &expected).is_ok());
        let error = verify(&expected, &Digest::of_bytes(b"other")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(CorruptBlob::is(&error));
        assert!(!CorruptBlob::is(&io::Error::other("plain")));
    }
}
