use super::{CacheProvider, TUIST_STREAM_REMOTE_UPLOAD_LIMIT};
use crate::{Digest, Result};
use std::io::{self, Write};

impl CacheProvider {
    /// Store bytes from an owned synchronous producer without an intermediate file.
    ///
    /// The producer runs on a blocking worker and must finish successfully before
    /// publication. `guard` stays with that worker through encoding and durable
    /// local publication, including after caller cancellation. Use `()` when no
    /// additional lifetime reservation is needed. Writes notice cancellation;
    /// scratch paths are removed immediately when the future is dropped.
    /// Remote mirroring follows the same bounded policy as [`put_stream`](Self::put_stream).
    pub async fn put_produced<W, G>(&self, guard: G, produce: W) -> Result<Digest>
    where
        W: FnOnce(&mut dyn Write) -> io::Result<()> + Send + 'static,
        G: Send + 'static,
    {
        let digest = match self {
            Self::Local(cas) => cas.put_produced(guard, produce).await?,
            Self::Tuist(cache) => cache.local().put_produced(guard, produce).await?,
        };
        self.mirror_produced(digest).await
    }

    /// Overlap digest calculation with an owned synchronous blob producer.
    ///
    /// The caller supplies a guard covering two worker reservations. One worker
    /// produces/encodes bytes; a second hashes a bounded one-chunk queue. Both
    /// workers finish before the guard is released, even on error or cancellation.
    /// Use [`put_produced`](Self::put_produced) when no additional capacity is available.
    /// Publication, digest validation and remote mirroring semantics are unchanged.
    pub async fn put_produced_pipelined<W, G>(&self, guard: G, produce: W) -> Result<Digest>
    where
        W: FnOnce(&mut dyn Write) -> io::Result<()> + Send + 'static,
        G: Send + 'static,
    {
        let digest = match self {
            Self::Local(cas) => cas.put_produced_pipelined(guard, produce).await?,
            Self::Tuist(cache) => cache.local().put_produced_pipelined(guard, produce).await?,
        };
        self.mirror_produced(digest).await
    }

    async fn mirror_produced(&self, digest: Digest) -> Result<Digest> {
        if let Self::Tuist(cache) = self {
            if cache.local().blob_size(&digest).await? <= TUIST_STREAM_REMOTE_UPLOAD_LIMIT {
                let bytes = cache.local().get_blob(&digest).await?;
                let _ = cache.put_blob(&bytes).await?;
            }
        }
        Ok(digest)
    }
}
