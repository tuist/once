use std::io::{self, Write};

use struo_cas::Digest;

#[derive(Default)]
pub(crate) struct DigestWriter(blake3::Hasher);

impl DigestWriter {
    pub(crate) fn finish(self) -> Digest {
        Digest::from_bytes(*self.0.finalize().as_bytes())
    }
}

impl Write for DigestWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
