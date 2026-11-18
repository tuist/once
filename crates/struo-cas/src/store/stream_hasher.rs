use std::io;
use std::sync::mpsc::{self, SyncSender};
use std::thread::JoinHandle;

pub(super) const PIPELINE_CHUNK: usize = 512 * 1024;

pub(super) enum Hasher {
    Inline(Box<blake3::Hasher>),
    Pipelined(Worker),
}

impl Hasher {
    pub(super) fn new() -> Self {
        Self::Inline(Box::new(blake3::Hasher::new()))
    }

    pub(super) fn pipelined() -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Option<Vec<u8>>>(1);
        let thread = std::thread::Builder::new()
            .name("struo-cas-digest".into())
            .spawn(move || {
                let mut hasher = blake3::Hasher::new();
                while let Ok(chunk) = receiver.recv() {
                    match chunk {
                        Some(bytes) => {
                            hasher.update(&bytes);
                        }
                        None => return Ok(hasher.finalize()),
                    }
                }
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "digest producer ended without EOF",
                ))
            })?;
        Ok(Self::Pipelined(Worker {
            sender: Some(sender),
            thread: Some(thread),
        }))
    }

    pub(super) fn update(&mut self, bytes: &[u8]) -> io::Result<()> {
        match self {
            Self::Inline(hasher) => {
                hasher.update(bytes);
                Ok(())
            }
            Self::Pipelined(worker) => {
                for chunk in bytes.chunks(PIPELINE_CHUNK) {
                    worker
                        .sender
                        .as_ref()
                        .unwrap()
                        .send(Some(chunk.to_vec()))
                        .map_err(|_| {
                            io::Error::new(io::ErrorKind::BrokenPipe, "digest worker stopped")
                        })?;
                }
                Ok(())
            }
        }
    }

    pub(super) fn finalize(self) -> io::Result<blake3::Hash> {
        match self {
            Self::Inline(hasher) => Ok(hasher.finalize()),
            Self::Pipelined(mut worker) => {
                worker.sender.take().unwrap().send(None).map_err(|_| {
                    io::Error::new(io::ErrorKind::BrokenPipe, "digest worker stopped")
                })?;
                worker
                    .thread
                    .take()
                    .unwrap()
                    .join()
                    .map_err(|_| io::Error::other("digest worker panicked"))?
            }
        }
    }
}

pub(super) struct Worker {
    sender: Option<SyncSender<Option<Vec<u8>>>>,
    thread: Option<JoinHandle<io::Result<blake3::Hash>>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        drop(self.sender.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipelined_hash_matches_inline_at_chunk_boundaries() {
        for size in [
            0,
            1,
            PIPELINE_CHUNK - 1,
            PIPELINE_CHUNK,
            PIPELINE_CHUNK + 1,
            3 * PIPELINE_CHUNK + 7,
        ] {
            let bytes = vec![19; size];
            let mut hasher = Hasher::pipelined().unwrap();
            hasher.update(&bytes).unwrap();
            assert_eq!(hasher.finalize().unwrap(), blake3::hash(&bytes));
        }
    }

    #[test]
    fn missing_explicit_eof_is_not_success() {
        let Hasher::Pipelined(mut worker) = Hasher::pipelined().unwrap() else {
            unreachable!()
        };
        worker
            .sender
            .as_ref()
            .unwrap()
            .send(Some(vec![3; PIPELINE_CHUNK]))
            .unwrap();
        drop(worker.sender.take());
        assert_eq!(
            worker
                .thread
                .take()
                .unwrap()
                .join()
                .unwrap()
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}
