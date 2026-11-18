use super::*;

struct PreparedDownload {
    digest: Digest,
    remote: reapi::Digest,
    flight: OwnedMutexGuard<()>,
}

impl TuistCache {
    pub(crate) async fn prefetch_blobs_guarded<G: Clone + Send + 'static>(
        &self,
        digests: &[Digest],
        guard: G,
    ) -> Result<()> {
        for window in digests.chunks(MAX_PENDING) {
            let mut window = window.to_vec();
            // Overlapping collections must acquire flights in the same order.
            window.sort_unstable_by_key(|digest| *digest.as_bytes());
            window.dedup();
            let mut prepared = Vec::new();
            for digest in window {
                if self.local.has_blob(&digest).await? {
                    continue;
                }
                if is_empty_blob(&digest) {
                    self.local.mirror_blob(&digest, &[]).await?;
                    continue;
                }
                let flight = self.download_flights.acquire(digest).await;
                if self.local.has_blob(&digest).await? {
                    continue;
                }
                prepared.push(PreparedDownload {
                    digest,
                    remote: self.remote_blob_digest(&digest).await?,
                    flight,
                });
            }
            if !prepared.is_empty() {
                self.enqueue_downloads(prepared, guard.clone(), self.batch_download_limit().await)
                    .await?;
            }
        }
        Ok(())
    }

    pub(in crate::tuist::cache) async fn enqueue_download<G: Send + 'static>(
        &self,
        digest: Digest,
        remote: reapi::Digest,
        flight: OwnedMutexGuard<()>,
        guard: G,
        limit: u64,
    ) -> Result<()> {
        self.enqueue_downloads(
            vec![PreparedDownload {
                digest,
                remote,
                flight,
            }],
            guard,
            Some(limit),
        )
        .await
    }

    async fn enqueue_downloads<G: Send + 'static>(
        &self,
        prepared: Vec<PreparedDownload>,
        guard: G,
        limit: Option<u64>,
    ) -> Result<()> {
        // Reserve the entire window atomically, never partial capacity while waiting.
        let mut slots = Arc::clone(&self.batch_downloads.slots)
            .acquire_many_owned(u32::try_from(prepared.len()).expect("bounded download window"))
            .await
            .expect("batch queue remains open");
        let owner: Arc<std::sync::Mutex<Box<dyn Send>>> =
            Arc::new(std::sync::Mutex::new(Box::new(guard)));
        let mut receivers = Vec::with_capacity(prepared.len());
        let mut streamed = Vec::new();
        let mut pending = self.batch_downloads.pending.lock().await;
        let start = pending.is_empty();
        for blob in prepared {
            let (response, ready) = oneshot::channel();
            receivers.push(ready);
            let request = PendingDownload {
                digest: blob.digest,
                remote: blob.remote,
                response,
                observer: transfer::current_observer(),
                started: Instant::now(),
                _flight: blob.flight,
                _slot: slots.split(1).expect("reserved download slot"),
                owner: Arc::clone(&owner),
            };
            if limit.is_some_and(|limit| {
                u64::try_from(request.remote.size_bytes).is_ok_and(|size| size <= limit)
            }) {
                pending.push_back(request);
            } else {
                streamed.push((request, None));
            }
        }
        if start && !pending.is_empty() {
            self.start_download_batch(limit.expect("batched downloads have a limit"));
        }
        drop(pending);
        if !streamed.is_empty() {
            let cache = self.clone();
            tokio::spawn(async move { cache.retry_downloads(streamed).await });
        }
        try_join_all(receivers.into_iter().map(|ready| async {
            ready.await.map_err(|_| {
                status::remote(
                    "get blob",
                    ErrorKind::Unavailable,
                    "batch download worker stopped",
                )
            })?
        }))
        .await?;
        Ok(())
    }
}
