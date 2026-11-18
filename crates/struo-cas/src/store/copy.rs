use std::io;
use std::path::Path;

use tokio::fs;

use super::Cas;
use crate::{blob, Digest, Error, Result};

const COPY_BUFFER: usize = 512 * 1024;

impl Cas {
    /// Write a blob's contents to `destination` through a bounded buffer.
    pub async fn copy_blob_to_file(&self, digest: &Digest, destination: &Path) -> Result<()> {
        self.copy_blob_to_file_with(digest, destination, |reader, writer| {
            let mut reader = io::BufReader::with_capacity(COPY_BUFFER, reader);
            io::copy(&mut reader, writer).map(|_| ())
        })
        .await
    }

    pub(crate) async fn copy_blob_to_file_with<T: Send + 'static>(
        &self,
        digest: &Digest,
        destination: &Path,
        copy: impl FnOnce(&mut dyn io::Read, &mut dyn io::Write) -> io::Result<T> + Send + 'static,
    ) -> Result<T> {
        let stored_path = self.blob_path(digest);
        if !fs::try_exists(&stored_path).await.unwrap_or(false) {
            return Err(Error::BlobNotFound(*digest));
        }
        let output_path = destination.to_path_buf();
        let error_path = output_path.clone();
        let expected = *digest;
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _stop = blob::StopOnDrop(std::sync::Arc::clone(&stop));
        let source_path = stored_path.clone();
        let decoded = tokio::task::spawn_blocking(move || {
            blob::decode_file(&stored_path, &output_path, &expected, &stop, copy)
        })
        .await
        .map_err(|source| Error::Io {
            path: error_path.clone(),
            source: io::Error::other(source.to_string()),
        })?;
        match decoded {
            Ok(value) => Ok(value),
            Err(source) if crate::integrity::CorruptBlob::is(&source) => {
                self.quarantine(digest, &source_path).await;
                Err(Error::Io {
                    path: source_path,
                    source,
                })
            }
            Err(source) => Err(Error::Io {
                path: error_path,
                source,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn buffered_copies_cover_empty_small_raw_compressed_and_literal_magic() {
        let tmp = tempfile::tempdir().unwrap();
        let cas = Cas::open(tmp.path().join("cas"));
        let mut state = 0x1234_5678_u64;
        let raw: Vec<u8> = (0..COPY_BUFFER * 2 + 7)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state.to_le_bytes()[0]
            })
            .collect();
        let mut magic = raw.clone();
        magic[..blob::ZSTD_BLOB_MAGIC.len()].copy_from_slice(blob::ZSTD_BLOB_MAGIC);
        for (name, payload, compressed) in [
            ("empty", Vec::new(), false),
            ("raw-small", b"small payload".to_vec(), false),
            ("raw-large", raw, false),
            ("compressed", vec![b'x'; COPY_BUFFER * 2 + 7], true),
            ("magic", magic, true),
        ] {
            let digest = cas.put_stream(payload.as_slice()).await.unwrap();
            let stored = fs::read(cas.blob_path(&digest)).await.unwrap();
            assert_eq!(stored.starts_with(blob::ZSTD_BLOB_MAGIC), compressed);
            let destination = tmp.path().join(name);
            cas.copy_blob_to_file(&digest, &destination).await.unwrap();
            assert_eq!(fs::read(destination).await.unwrap(), payload);
        }
    }

    #[tokio::test]
    async fn decode_failure_keeps_existing_output_and_cleans_partial_copy() {
        for fault in ["truncated", "wrong-size"] {
            let tmp = tempfile::tempdir().unwrap();
            let cas = Cas::open(tmp.path().join("cas"));
            let payload = vec![b'x'; COPY_BUFFER * 2 + 7];
            let digest = cas.put_blob(&payload).await.unwrap();
            let stored_path = cas.blob_path(&digest);
            let mut stored = fs::read(&stored_path).await.unwrap();
            assert!(stored.starts_with(blob::ZSTD_BLOB_MAGIC));
            if fault == "truncated" {
                stored.truncate(blob::ZSTD_BLOB_HEADER_LEN + 1);
            } else {
                let size = u64::try_from(payload.len() + 1).unwrap();
                stored[blob::ZSTD_BLOB_MAGIC.len()..blob::ZSTD_BLOB_HEADER_LEN]
                    .copy_from_slice(&size.to_le_bytes());
            }
            fs::write(&stored_path, stored).await.unwrap();
            let outputs = tmp.path().join("outputs");
            fs::create_dir(&outputs).await.unwrap();
            let destination = outputs.join("existing-output");
            fs::write(&destination, b"keep this output").await.unwrap();
            assert!(cas.copy_blob_to_file(&digest, &destination).await.is_err());
            assert_eq!(fs::read(&destination).await.unwrap(), b"keep this output");
            assert_eq!(std::fs::read_dir(outputs).unwrap().count(), 1);
        }
    }
}
