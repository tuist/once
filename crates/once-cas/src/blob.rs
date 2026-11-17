use std::borrow::Cow;
use std::fs::File;
use std::io::{self, Read, Seek};
use std::path::Path;

use crate::{
    integrity::{self, CorruptBlob, HashingWriter},
    Digest,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use zstd::stream::encode_all;

pub(crate) const ZSTD_BLOB_MAGIC: &[u8] = b"once.cas.zstd.v1\0";
pub(crate) const ZSTD_BLOB_HEADER_LEN: usize = ZSTD_BLOB_MAGIC.len() + RAW_SIZE_LEN;
pub(crate) const ZSTD_LEVEL: i32 = 1;
const RAW_SIZE_LEN: usize = 8;

pub(crate) fn encode_bytes(raw: &[u8]) -> io::Result<Cow<'_, [u8]>> {
    let compressed = encode_all(raw, ZSTD_LEVEL)?;
    let wrapped_len = checked_wrapped_len(compressed.len())?;
    if raw.starts_with(ZSTD_BLOB_MAGIC) || wrapped_len < raw.len() {
        let mut out = Vec::with_capacity(wrapped_len);
        out.extend_from_slice(ZSTD_BLOB_MAGIC);
        out.extend_from_slice(&raw_len_header(raw.len())?);
        out.extend_from_slice(&compressed);
        Ok(Cow::Owned(out))
    } else {
        Ok(Cow::Borrowed(raw))
    }
}

pub(crate) fn decode_bytes(stored: Vec<u8>) -> io::Result<Vec<u8>> {
    if !stored.starts_with(ZSTD_BLOB_MAGIC) {
        return Ok(stored);
    }
    let Some(raw_len) = stored
        .get(ZSTD_BLOB_MAGIC.len()..ZSTD_BLOB_HEADER_LEN)
        .map(read_raw_size)
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "truncated zstd blob header",
        ));
    };
    let compressed = stored
        .get(ZSTD_BLOB_HEADER_LEN..)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "truncated zstd blob body"))?;
    let mut inflater = zstd::stream::read::Decoder::new(compressed)?;
    inflater.window_log_max(23)?;
    let mut decoded = Vec::new();
    inflater
        .take(raw_len.saturating_add(1))
        .read_to_end(&mut decoded)?;
    if decoded.len() as u64 != raw_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "zstd blob decoded size mismatch",
        ));
    }
    Ok(decoded)
}

pub(crate) fn raw_size_from_header(header: &[u8]) -> Option<u64> {
    header
        .starts_with(ZSTD_BLOB_MAGIC)
        .then(|| {
            header
                .get(ZSTD_BLOB_MAGIC.len()..ZSTD_BLOB_HEADER_LEN)
                .map(read_raw_size)
        })
        .flatten()
}

pub(crate) fn decode_file<T>(
    stored_path: &Path,
    output_path: &Path,
    expected: &Digest,
    stop: &AtomicBool,
    copy: impl FnOnce(&mut dyn Read, &mut dyn io::Write) -> io::Result<T>,
) -> io::Result<T> {
    decode_open_file(
        File::open(stored_path)?,
        output_path,
        expected,
        stop,
        true,
        copy,
    )
}

pub(crate) fn decode_open_file<T>(
    mut input: File,
    output_path: &Path,
    expected: &Digest,
    stop: &AtomicBool,
    durable: bool,
    copy: impl FnOnce(&mut dyn Read, &mut dyn io::Write) -> io::Result<T>,
) -> io::Result<T> {
    let mut header = vec![0_u8; ZSTD_BLOB_HEADER_LEN];
    let mut header_len = 0;
    while header_len < header.len() {
        let read = input.read(&mut header[header_len..])?;
        if read == 0 {
            break;
        }
        header_len += read;
    }
    input.rewind()?;
    let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
    let mut output = HashingWriter::new(tempfile::NamedTempFile::new_in(parent)?);
    let raw_size = (header_len == ZSTD_BLOB_HEADER_LEN && header.starts_with(ZSTD_BLOB_MAGIC))
        .then(|| raw_size_from_header(&header))
        .flatten();
    let result = if let Some(raw_size) = raw_size {
        input.seek(io::SeekFrom::Start(ZSTD_BLOB_HEADER_LEN as u64))?;
        let mut decoded = zstd::stream::read::Decoder::new(input)
            .map_err(|error| CorruptBlob::io_error(*expected, &error.to_string()))?;
        decoded
            .window_log_max(23)
            .map_err(|error| CorruptBlob::io_error(*expected, &error.to_string()))?;
        let mut reader = Stoppable {
            input: decoded.take(raw_size.saturating_add(1)),
            stop,
        };
        copy(&mut reader, &mut output).map_err(|error| {
            if !stop.load(Ordering::Acquire)
                && error.raw_os_error().is_none()
                && error.kind() != io::ErrorKind::Interrupted
            {
                CorruptBlob::io_error(*expected, &error.to_string())
            } else {
                error
            }
        })?
    } else {
        let mut reader = Stoppable { input, stop };
        copy(&mut reader, &mut output)?
    };
    integrity::verify(expected, &output.digest())?;
    let output = output.into_inner();
    if raw_size.is_some_and(|size| {
        output
            .as_file()
            .metadata()
            .is_ok_and(|metadata| metadata.len() != size)
    }) {
        return Err(CorruptBlob::io_error(
            *expected,
            "zstd blob decoded size mismatch",
        ));
    }
    if durable {
        output.as_file().sync_all()?;
    }
    if stop.load(Ordering::Acquire) {
        return Err(cancelled());
    }
    output.persist(output_path).map_err(|error| error.error)?;
    Ok(result)
}

struct Stoppable<'a, R> {
    input: R,
    stop: &'a AtomicBool,
}
impl<R: Read> Read for Stoppable<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if self.stop.load(Ordering::Acquire) {
            return Err(cancelled());
        }
        self.input.read(bytes)
    }
}
fn cancelled() -> io::Error {
    // Interrupted is retried by io::copy and read_to_end, so it cannot stop them.
    io::Error::other("blob restore was cancelled")
}
pub(crate) struct StopOnDrop(pub(crate) Arc<AtomicBool>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn read_raw_size(bytes: &[u8]) -> u64 {
    let mut raw = [0_u8; RAW_SIZE_LEN];
    raw.copy_from_slice(bytes);
    u64::from_le_bytes(raw)
}

fn checked_wrapped_len(compressed_len: usize) -> io::Result<usize> {
    ZSTD_BLOB_HEADER_LEN
        .checked_add(compressed_len)
        .ok_or_else(blob_too_large)
}

fn raw_len_header(raw_len: usize) -> io::Result<[u8; RAW_SIZE_LEN]> {
    let raw_len = u64::try_from(raw_len).map_err(|_| blob_too_large())?;
    Ok(raw_len.to_le_bytes())
}

#[cfg(test)]
fn raw_len_header_from_u128(raw_len: u128) -> io::Result<[u8; RAW_SIZE_LEN]> {
    let raw_len = u64::try_from(raw_len).map_err(|_| blob_too_large())?;
    Ok(raw_len.to_le_bytes())
}

fn blob_too_large() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "blob is too large")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_stops_copy_without_publishing_or_quarantining_valid_data() {
        for payload in [b"raw".to_vec(), vec![b'x'; 16 * 1024]] {
            let directory = tempfile::tempdir().unwrap();
            let source = directory.path().join("blob");
            let output = directory.path().join("output");
            std::fs::write(&source, encode_bytes(&payload).unwrap()).unwrap();
            std::fs::write(&output, b"original").unwrap();
            let digest = Digest::of_bytes(&payload);
            let stop = Arc::new(AtomicBool::new(true));
            let cancel = Arc::clone(&stop);
            let (started, ready) = std::sync::mpsc::channel();
            let (finished, result) = std::sync::mpsc::channel();
            let destination = output.clone();
            let worker = std::thread::spawn(move || {
                started.send(()).unwrap();
                let copied =
                    decode_file(&source, &destination, &digest, &cancel, |input, output| {
                        io::copy(input, output)
                    });
                finished.send(copied).unwrap();
            });
            ready.recv().unwrap();
            let copied = result.recv_timeout(std::time::Duration::from_secs(1));
            // Let a broken retry loop finish rather than hang the test process.
            stop.store(false, Ordering::Release);
            worker.join().unwrap();
            let error = copied.expect("cancelled copying must stop").unwrap_err();
            assert!(!CorruptBlob::is(&error));
            assert_eq!(std::fs::read(&output).unwrap(), b"original");
        }
    }

    #[test]
    fn small_payload_stays_raw_when_compression_would_grow_it() {
        let raw = b"abc";

        let encoded = encode_bytes(raw).unwrap();

        assert_eq!(encoded.as_ref(), raw);
        assert!(matches!(encoded, Cow::Borrowed(_)));
        assert_eq!(decode_bytes(encoded.into_owned()).unwrap(), raw);
    }

    #[test]
    fn compressible_payload_uses_zstd_wrapper() {
        let raw = b"same line\n".repeat(1024);

        let encoded = encode_bytes(&raw).unwrap();

        assert!(encoded.starts_with(ZSTD_BLOB_MAGIC));
        assert!(encoded.len() < raw.len());
        assert!(matches!(encoded, Cow::Owned(_)));
        assert_eq!(decode_bytes(encoded.into_owned()).unwrap(), raw);
    }

    #[test]
    fn magic_prefixed_payload_gets_wrapped_even_when_small() {
        let mut raw = Vec::from(ZSTD_BLOB_MAGIC);
        raw.extend_from_slice(b"literal");

        let encoded = encode_bytes(&raw).unwrap();

        assert!(encoded.starts_with(ZSTD_BLOB_MAGIC));
        assert!(matches!(encoded, Cow::Owned(_)));
        assert_eq!(decode_bytes(encoded.into_owned()).unwrap(), raw);
    }

    #[test]
    fn checked_wrapped_len_rejects_overflow() {
        let err = checked_wrapped_len(usize::MAX).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(err.to_string(), "blob is too large");
    }

    #[test]
    fn raw_len_header_rejects_u64_overflow() {
        let err = raw_len_header_from_u128(u128::from(u64::MAX) + 1).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(err.to_string(), "blob is too large");
    }
}
