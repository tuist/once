use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};

use crate::{blob, Digest};

#[cfg(test)]
#[path = "stream_raw_tests.rs"]
mod tests;

pub(super) struct Writer {
    raw: BufWriter<std::fs::File>,
    encoded: Option<zstd::stream::write::Encoder<'static, BufWriter<std::fs::File>>>,
    hasher: super::hasher::Hasher,
    size: u64,
    raw_size: u64,
    defer_raw: bool,
    prefix: Vec<u8>,
    expected: Option<u64>,
    sha256: Option<(sha2::Sha256, String)>,
}

impl Writer {
    pub(super) fn new(
        raw: std::fs::File,
        mut encoded: std::fs::File,
        expected: Option<u64>,
    ) -> io::Result<Self> {
        encoded.write_all(blob::ZSTD_BLOB_MAGIC)?;
        encoded.write_all(&0_u64.to_le_bytes())?;
        let mut output = Self::raw(raw, expected);
        output.encoded = Some(zstd::stream::write::Encoder::new(
            BufWriter::with_capacity(512 * 1024, encoded),
            blob::ZSTD_LEVEL,
        )?);
        Ok(output)
    }

    pub(super) fn raw(raw: std::fs::File, expected: Option<u64>) -> Self {
        Self {
            raw: BufWriter::new(raw),
            encoded: None,
            hasher: super::hasher::Hasher::new(),
            size: 0,
            raw_size: 0,
            defer_raw: false,
            prefix: Vec::with_capacity(blob::ZSTD_BLOB_MAGIC.len()),
            expected,
            sha256: None,
        }
    }

    pub(super) fn with_sha256(mut self, expected: Option<String>) -> Self {
        use sha2::Digest as _;
        self.sha256 = expected.map(|expected| (sha2::Sha256::new(), expected));
        self
    }

    pub(super) fn pipeline_hash(&mut self) -> io::Result<()> {
        self.hasher = super::hasher::Hasher::pipelined()?;
        Ok(())
    }

    pub(super) fn consider_captured(&mut self, received: u64) {
        if self.size >= 512 * 1024 && received < self.size - self.size / 4 {
            self.defer_raw = true;
        }
    }

    pub(super) fn push(&mut self, bytes: &[u8]) -> io::Result<()> {
        let count = (blob::ZSTD_BLOB_MAGIC.len() - self.prefix.len()).min(bytes.len());
        self.prefix.extend_from_slice(&bytes[..count]);
        self.size = self
            .size
            .checked_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "blob is too large"))?;
        if self.expected.is_some_and(|expected| self.size > expected) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "remote blob exceeds declared size",
            ));
        }
        self.hasher.update(bytes)?;
        if let Some((hasher, _)) = &mut self.sha256 {
            use sha2::Digest as _;
            hasher.update(bytes);
        }
        if !self.defer_raw {
            self.raw.write_all(bytes)?;
            self.raw_size = self.size;
        }
        if let Some(encoded) = &mut self.encoded {
            encoded.write_all(bytes)?;
        }
        Ok(())
    }

    pub(super) fn prepare(
        mut self,
        captured: Option<std::fs::File>,
    ) -> io::Result<(bool, Digest, std::fs::File)> {
        if self.expected.is_some_and(|expected| self.size != expected) {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "remote blob does not match declared size",
            ));
        }
        if let Some((hasher, expected)) = self.sha256.take() {
            use sha2::Digest as _;
            if hex::encode(hasher.finalize()) != expected {
                return Err(crate::integrity::CorruptBlob::io_error(
                    Digest::of_bytes(&[]),
                    "remote blob failed SHA-256 verification",
                ));
            }
        }
        self.raw.flush()?;
        let mut raw = self
            .raw
            .into_inner()
            .map_err(io::IntoInnerError::into_error)?;
        let mut encoded = if let Some(encoder) = self.encoded {
            let mut encoded = encoder.finish()?;
            encoded.flush()?;
            encoded
                .into_inner()
                .map_err(io::IntoInnerError::into_error)?
        } else {
            captured.ok_or_else(|| io::Error::other("missing captured encoding"))?
        };
        encoded.seek(SeekFrom::Start(
            u64::try_from(blob::ZSTD_BLOB_MAGIC.len()).unwrap(),
        ))?;
        encoded.write_all(&self.size.to_le_bytes())?;
        let compressed =
            self.prefix == blob::ZSTD_BLOB_MAGIC || encoded.metadata()?.len() < self.size;
        let file = if compressed {
            encoded
        } else {
            if self.raw_size < self.size {
                complete_raw(&mut raw, &mut encoded, self.raw_size, self.size)?;
            }
            raw
        };
        Ok((
            compressed,
            Digest::from_bytes(*self.hasher.finalize()?.as_bytes()),
            file,
        ))
    }
}

fn complete_raw(
    raw: &mut std::fs::File,
    encoded: &mut std::fs::File,
    staged: u64,
    expected: u64,
) -> io::Result<()> {
    encoded.seek(SeekFrom::Start(
        u64::try_from(blob::ZSTD_BLOB_HEADER_LEN).unwrap(),
    ))?;
    let mut decoder = zstd::stream::read::Decoder::new(encoded)?;
    decoder.window_log_max(super::window::MAX_WINDOW_LOG)?;
    let mut buffer = vec![0; 512 * 1024];
    let mut position = 0_u64;
    loop {
        let count = decoder.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let end = position
            .checked_add(u64::try_from(count).unwrap())
            .filter(|end| *end <= expected)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "captured blob exceeds declared size",
                )
            })?;
        let skip = usize::try_from(
            staged
                .saturating_sub(position)
                .min(u64::try_from(count).unwrap()),
        )
        .unwrap();
        raw.write_all(&buffer[skip..count])?;
        position = end;
    }
    if position != expected {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "captured blob does not match declared size",
        ));
    }
    Ok(())
}
