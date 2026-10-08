use std::fs::File;
use std::io::{self, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc;

pub(super) struct Chunk {
    pub(super) data: Vec<u8>,
    pub(super) last: bool,
}

/// Reads `file` on a blocking thread as `chunk_size` pieces of an upload
/// body, zstd-compressed when `zstd` is set. The channel holds one piece, so
/// an upload buffers a few chunks whatever the blob size, and compression
/// stays off the async runtime.
pub(super) fn chunks(
    file: File,
    size: u64,
    zstd: bool,
    chunk_size: usize,
) -> mpsc::Receiver<io::Result<Chunk>> {
    let (sender, receiver) = mpsc::channel(1);
    tokio::task::spawn_blocking(move || {
        if let Err(error) = produce(file, size, zstd, chunk_size, &sender) {
            let _ = sender.blocking_send(Err(error));
        }
    });
    receiver
}

fn produce(
    file: File,
    size: u64,
    zstd: bool,
    chunk_size: usize,
    sender: &mpsc::Sender<io::Result<Chunk>>,
) -> io::Result<()> {
    let read = Arc::new(AtomicU64::new(0));
    let source = Counted {
        inner: file,
        read: Arc::clone(&read),
    };
    let mut body: Box<dyn Read> = if zstd {
        Box::new(zstd::stream::read::Encoder::new(source, 0)?)
    } else {
        Box::new(source)
    };
    let mut current = fill(&mut body, chunk_size)?;
    loop {
        let next = fill(&mut body, chunk_size)?;
        let last = next.is_empty();
        if last && read.load(Ordering::Relaxed) != size {
            return Err(io::Error::other(
                "staged blob did not match its declared size",
            ));
        }
        if sender
            .blocking_send(Ok(Chunk {
                data: current,
                last,
            }))
            .is_err()
            || last
        {
            return Ok(());
        }
        current = next;
    }
}

fn fill(reader: &mut dyn Read, size: usize) -> io::Result<Vec<u8>> {
    let mut buffer = Vec::with_capacity(size);
    reader
        .take(u64::try_from(size).unwrap_or(u64::MAX))
        .read_to_end(&mut buffer)?;
    Ok(buffer)
}

struct Counted {
    inner: File,
    read: Arc<AtomicU64>,
}

impl Read for Counted {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.read.fetch_add(read as u64, Ordering::Relaxed);
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn collect(
        bytes: &[u8],
        size: u64,
        zstd: bool,
        chunk_size: usize,
    ) -> io::Result<Vec<Chunk>> {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("blob");
        std::fs::write(&path, bytes).unwrap();
        let mut receiver = chunks(File::open(&path).unwrap(), size, zstd, chunk_size);
        let mut collected = Vec::new();
        while let Some(chunk) = receiver.recv().await {
            collected.push(chunk?);
        }
        Ok(collected)
    }

    fn joined(chunks: &[Chunk]) -> Vec<u8> {
        chunks
            .iter()
            .flat_map(|chunk| chunk.data.iter().copied())
            .collect()
    }

    #[tokio::test]
    async fn only_the_final_chunk_finishes_the_body() {
        let bytes: Vec<u8> = (0..10_000_u32)
            .map(|index| u8::try_from(index % 7).unwrap())
            .collect();
        let chunks = collect(&bytes, 10_000, false, 4_096).await.unwrap();
        let sizes: Vec<_> = chunks.iter().map(|chunk| chunk.data.len()).collect();
        assert_eq!(sizes, [4_096, 4_096, 1_808]);
        let finished: Vec<_> = chunks.iter().map(|chunk| chunk.last).collect();
        assert_eq!(finished, [false, false, true]);
        assert_eq!(joined(&chunks), bytes);
    }

    #[tokio::test]
    async fn compressed_body_decodes_to_the_file() {
        let bytes: Vec<u8> = (0..1_000_000_u32)
            .map(|index| u8::try_from(index % 13).unwrap())
            .collect();
        let chunks = collect(&bytes, 1_000_000, true, 64).await.unwrap();
        assert!(chunks.len() > 1);
        assert!(chunks.last().unwrap().last);
        let compressed = joined(&chunks);
        assert!(compressed.len() < bytes.len() / 10);
        assert_eq!(
            zstd::stream::decode_all(compressed.as_slice()).unwrap(),
            bytes
        );
    }

    #[tokio::test]
    async fn a_file_that_does_not_match_its_size_fails() {
        for (size, zstd) in [(11, false), (9, false), (11, true)] {
            let error = collect(b"ten bytes!", size, zstd, 4).await.err().unwrap();
            assert!(error.to_string().contains("declared size"), "{error}");
        }
    }
}
