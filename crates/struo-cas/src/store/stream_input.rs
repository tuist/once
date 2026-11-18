use std::io::{self, BufWriter, Cursor, Read, Write};

use tokio::sync::mpsc;

pub(super) struct Chunks {
    receiver: mpsc::Receiver<Option<Vec<u8>>>,
    current: Cursor<Vec<u8>>,
    ended: bool,
    encoded: BufWriter<std::fs::File>,
    received: u64,
}

impl Chunks {
    pub(super) fn new(
        receiver: mpsc::Receiver<Option<Vec<u8>>>,
        mut encoded: std::fs::File,
    ) -> io::Result<Self> {
        encoded.write_all(crate::blob::ZSTD_BLOB_MAGIC)?;
        encoded.write_all(&0_u64.to_le_bytes())?;
        Ok(Self {
            receiver,
            current: Cursor::new(Vec::new()),
            ended: false,
            encoded: BufWriter::new(encoded),
            received: 0,
        })
    }

    pub(super) fn received(&self) -> u64 {
        self.received
    }

    pub(super) fn finish(mut self) -> io::Result<std::fs::File> {
        self.encoded.flush()?;
        self.encoded
            .into_inner()
            .map_err(io::IntoInnerError::into_error)
    }
}

impl Read for Chunks {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.current.read(buffer)?;
            if count > 0 || self.ended {
                return Ok(count);
            }
            match self.receiver.blocking_recv() {
                Some(Some(bytes)) => {
                    self.received = self
                        .received
                        .checked_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
                        .ok_or_else(|| io::Error::other("encoded blob is too large"))?;
                    self.encoded.write_all(&bytes)?;
                    self.current = Cursor::new(bytes);
                }
                Some(None) => self.ended = true,
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "blob stream interrupted",
                    ))
                }
            }
        }
    }
}
