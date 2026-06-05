use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;

use zstd::stream::{copy_encode, decode_all, encode_all};

pub(crate) const ZSTD_BLOB_MAGIC: &[u8] = b"once.cas.zstd.v1\0";
pub(crate) const ZSTD_BLOB_HEADER_LEN: usize = ZSTD_BLOB_MAGIC.len() + RAW_SIZE_LEN;
const ZSTD_LEVEL: i32 = 3;
const RAW_SIZE_LEN: usize = 8;

pub(crate) fn encode_bytes(raw: &[u8]) -> io::Result<Vec<u8>> {
    let compressed = encode_all(raw, ZSTD_LEVEL)?;
    let wrapped_len = ZSTD_BLOB_HEADER_LEN
        .checked_add(compressed.len())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "blob is too large"))?;
    if raw.starts_with(ZSTD_BLOB_MAGIC) || wrapped_len < raw.len() {
        let raw_len = u64::try_from(raw.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "blob is too large"))?;
        let mut out = Vec::with_capacity(wrapped_len);
        out.extend_from_slice(ZSTD_BLOB_MAGIC);
        out.extend_from_slice(&raw_len.to_le_bytes());
        out.extend_from_slice(&compressed);
        Ok(out)
    } else {
        Ok(raw.to_vec())
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
    let decoded = decode_all(compressed)?;
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

pub(crate) fn encode_file(raw_path: &Path, encoded_path: &Path) -> io::Result<EncodedFile> {
    let raw_size = std::fs::metadata(raw_path)?.len();
    let raw_starts_with_magic = file_starts_with(raw_path, ZSTD_BLOB_MAGIC)?;

    let mut input = File::open(raw_path)?;
    let mut output = File::create(encoded_path)?;
    output.write_all(ZSTD_BLOB_MAGIC)?;
    output.write_all(&raw_size.to_le_bytes())?;
    copy_encode(&mut input, &mut output, ZSTD_LEVEL)?;
    output.sync_all()?;

    let encoded_size = output.metadata()?.len();
    Ok(EncodedFile {
        should_store: raw_starts_with_magic || encoded_size < raw_size,
    })
}

pub(crate) struct EncodedFile {
    pub(crate) should_store: bool,
}

fn read_raw_size(bytes: &[u8]) -> u64 {
    let mut raw = [0_u8; RAW_SIZE_LEN];
    raw.copy_from_slice(bytes);
    u64::from_le_bytes(raw)
}

fn file_starts_with(path: &Path, needle: &[u8]) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let mut prefix = vec![0_u8; needle.len()];
    match file.read_exact(&mut prefix) {
        Ok(()) => Ok(prefix == needle),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_payload_stays_raw_when_compression_would_grow_it() {
        let raw = b"abc";

        let encoded = encode_bytes(raw).unwrap();

        assert_eq!(encoded, raw);
        assert_eq!(decode_bytes(encoded).unwrap(), raw);
    }

    #[test]
    fn compressible_payload_uses_zstd_wrapper() {
        let raw = b"same line\n".repeat(1024);

        let encoded = encode_bytes(&raw).unwrap();

        assert!(encoded.starts_with(ZSTD_BLOB_MAGIC));
        assert!(encoded.len() < raw.len());
        assert_eq!(decode_bytes(encoded).unwrap(), raw);
    }

    #[test]
    fn magic_prefixed_payload_gets_wrapped_even_when_small() {
        let mut raw = Vec::from(ZSTD_BLOB_MAGIC);
        raw.extend_from_slice(b"literal");

        let encoded = encode_bytes(&raw).unwrap();

        assert!(encoded.starts_with(ZSTD_BLOB_MAGIC));
        assert_eq!(decode_bytes(encoded).unwrap(), raw);
    }
}
