use std::io::{self, Read};
use std::path::Path;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::mpsc;

use crate::{Digest, Error, Result};

use super::STREAM_CHUNK;

#[cfg(test)]
#[path = "stream_decode_tests.rs"]
mod decode_tests;
#[cfg(test)]
#[path = "stream_guard_tests.rs"]
mod guard_tests;
#[path = "stream_hasher.rs"]
mod hasher;
#[path = "stream_input.rs"]
mod input;
#[path = "stream_writer.rs"]
mod output;
#[path = "stream_producer.rs"]
mod producer;
#[path = "stream_window.rs"]
mod window;
pub(super) use producer::produce;

pub(super) fn window_limit_exceeded(error: &Error) -> bool {
    window::exceeded(error)
}

#[cfg(test)]
pub(super) async fn ingest<R: AsyncRead + Unpin>(
    scratch: &Path,
    reader: R,
) -> Result<(tempfile::TempPath, Digest)> {
    let (path, digest, _) = ingest_impl(scratch, reader, None, (), false).await?;
    Ok((path, digest))
}

pub(super) async fn prepare<R: AsyncRead + Unpin>(
    scratch: &Path,
    reader: R,
) -> Result<(tempfile::TempPath, Digest, std::fs::File)> {
    ingest_impl(scratch, reader, None, (), true).await
}

pub(super) async fn ingest_remote<R: AsyncRead + Unpin, G: Send + 'static>(
    scratch: &Path,
    reader: R,
    expected: u64,
    compressed: bool,
    guard: G,
) -> Result<(tempfile::TempPath, Digest)> {
    ingest_remote_verified(scratch, reader, expected, compressed, None, guard).await
}

pub(super) async fn ingest_remote_verified<R: AsyncRead + Unpin, G: Send + 'static>(
    scratch: &Path,
    reader: R,
    expected: u64,
    compressed: bool,
    sha256: Option<String>,
    guard: G,
) -> Result<(tempfile::TempPath, Digest)> {
    let (path, digest, _) = ingest_impl(
        scratch,
        reader,
        Some((expected, compressed, sha256)),
        guard,
        false,
    )
    .await?;
    Ok((path, digest))
}

async fn ingest_impl<R: AsyncRead + Unpin, G: Send + 'static>(
    scratch: &Path,
    mut reader: R,
    remote: Option<(u64, bool, Option<String>)>,
    guard: G,
    deferred_sync: bool,
) -> Result<(tempfile::TempPath, Digest, std::fs::File)> {
    let (raw_file, raw_path) = temporary(scratch, "stream-")?.into_parts();
    let (encoded_file, encoded_path) = temporary(scratch, "zstd-")?.into_parts();
    let path = raw_path.to_path_buf();
    let (sender, receiver) = mpsc::channel(1);
    let worker = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        encode(raw_file, encoded_file, receiver, remote, deferred_sync)
    });
    let read = async {
        loop {
            let mut bytes = vec![0; STREAM_CHUNK];
            let count = reader.read(&mut bytes).await?;
            let chunk = if count == 0 {
                None
            } else {
                bytes.truncate(count);
                Some(bytes)
            };
            sender
                .send(chunk)
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "stream encoder stopped"))?;
            if count == 0 {
                return Ok::<_, io::Error>(());
            }
        }
    }
    .await;
    drop(sender);
    let encoded = worker.await.map_err(|source| Error::Io {
        path: path.clone(),
        source: io::Error::other(source.to_string()),
    })?;
    if let Err(source) = read {
        if source.kind() != io::ErrorKind::BrokenPipe || encoded.is_ok() {
            return Err(Error::Io { path, source });
        }
    }
    let (compressed, digest, file) = encoded.map_err(|source| Error::Io { path, source })?;
    Ok((
        if compressed { encoded_path } else { raw_path },
        digest,
        file,
    ))
}

fn temporary(scratch: &Path, prefix: &str) -> Result<tempfile::NamedTempFile> {
    tempfile::Builder::new()
        .prefix(prefix)
        .make_in(scratch, |path| {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(path)
        })
        .map_err(|source| Error::Io {
            path: scratch.to_path_buf(),
            source,
        })
}

fn encode(
    raw: std::fs::File,
    encoded: std::fs::File,
    mut receiver: mpsc::Receiver<Option<Vec<u8>>>,
    remote: Option<(u64, bool, Option<String>)>,
    deferred_sync: bool,
) -> io::Result<(bool, Digest, std::fs::File)> {
    let expected = remote.as_ref().map(|(size, _, _)| *size);
    let compressed = remote
        .as_ref()
        .is_some_and(|(_, compressed, _)| *compressed);
    let sha256 = remote.and_then(|(_, _, sha256)| sha256);
    if compressed {
        let mut output = output::Writer::raw(raw, expected).with_sha256(sha256);
        let mut decoder = zstd::stream::read::Decoder::new(input::Chunks::new(receiver, encoded)?)?;
        decoder.window_log_max(window::MAX_WINDOW_LOG)?;
        let mut buffer = vec![0_u8; STREAM_CHUNK * 2];
        let mut filled = 0;
        loop {
            let count = decoder
                .read(&mut buffer[filled..])
                .map_err(window::decode_error)?;
            filled += count;
            if count == 0 || filled == buffer.len() {
                output.push(&buffer[..filled])?;
                output.consider_captured(decoder.get_ref().get_ref().received());
                filled = 0;
            }
            if count == 0 {
                break;
            }
        }
        drop(buffer);
        let captured = decoder.finish().into_inner().finish()?;
        finish(output, Some(captured), deferred_sync)
    } else {
        let mut output = output::Writer::new(raw, encoded, expected)?.with_sha256(sha256);
        loop {
            let chunk = receiver.blocking_recv().ok_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "blob stream interrupted")
            })?;
            let Some(chunk) = chunk else { break };
            output.push(&chunk)?;
        }
        finish(output, None, deferred_sync)
    }
}

fn finish(
    output: output::Writer,
    captured: Option<std::fs::File>,
    deferred_sync: bool,
) -> io::Result<(bool, Digest, std::fs::File)> {
    let (compressed, digest, file) = output.prepare(captured)?;
    if !deferred_sync {
        file.sync_all()?;
    }
    Ok((compressed, digest, file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn fragmented_magic_prefix_and_empty_stream_roundtrip() {
        for bytes in [blob::ZSTD_BLOB_MAGIC.to_vec(), Vec::new()] {
            let temp = tempfile::tempdir().unwrap();
            let expected = bytes.clone();
            let (mut writer, reader) = tokio::io::duplex(1);
            let producer = async move {
                for byte in bytes {
                    writer.write_all(&[byte]).await.unwrap();
                }
            };
            let ((), stored) = tokio::join!(producer, ingest(temp.path(), reader));
            let (path, digest) = stored.unwrap();
            assert_eq!(digest, Digest::of_bytes(&expected));
            assert_eq!(
                blob::decode_bytes(std::fs::read(&path).unwrap()).unwrap(),
                expected
            );
        }
    }

    #[tokio::test]
    async fn cancellation_discards_both_temporary_representations() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let (mut writer, reader) = tokio::io::duplex(64);
        let task = tokio::spawn(async move { ingest(&root, reader).await });
        writer.write_all(&[42; 64]).await.unwrap();
        tokio::task::yield_now().await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}
