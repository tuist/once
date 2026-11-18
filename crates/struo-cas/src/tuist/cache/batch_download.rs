use std::collections::VecDeque;

use futures::StreamExt;
use tokio::sync::{oneshot, OwnedMutexGuard, OwnedSemaphorePermit};

use super::*;

mod enqueue;
mod rpc;
use rpc::BatchResponse;

const MAX_PENDING: usize = 128;
const FRAMING_BYTES: u64 = 64 * 1024;

pub(super) struct BatchDownloads {
    pending: Mutex<VecDeque<PendingDownload>>,
    slots: Arc<Semaphore>,
    unsupported: std::sync::atomic::AtomicBool,
}

impl Default for BatchDownloads {
    fn default() -> Self {
        Self {
            pending: Mutex::default(),
            slots: Arc::new(Semaphore::new(MAX_PENDING)),
            unsupported: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl std::fmt::Debug for BatchDownloads {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BatchDownloads")
            .finish_non_exhaustive()
    }
}

struct PendingDownload {
    digest: Digest,
    remote: reapi::Digest,
    response: oneshot::Sender<Result<()>>,
    observer: Option<Arc<dyn transfer::TransferObserver>>,
    started: Instant,
    _flight: OwnedMutexGuard<()>,
    _slot: OwnedSemaphorePermit,
    owner: Arc<std::sync::Mutex<Box<dyn Send>>>,
}

impl PendingDownload {
    fn complete(self, result: Result<()>) {
        if result.is_ok() {
            if let Some(observer) = self.observer {
                observer.observe(transfer::Transfer {
                    direction: TransferDirection::Download,
                    digest: self.digest,
                    size_bytes: u64::try_from(self.remote.size_bytes).unwrap_or_default(),
                    duration: self.started.elapsed(),
                });
            }
        }
        let _ = self.response.send(result);
    }
}

impl TuistCache {
    pub(super) async fn batch_download_limit(&self) -> Option<u64> {
        if self.batch_downloads.unsupported.load(Ordering::Relaxed) {
            return None;
        }
        let advertised = self.remote_capabilities().await.batch_read_limit?;
        let server = if advertised == 0 {
            u64::MAX
        } else {
            advertised
        };
        let memory = self
            .budget
            .memory_bytes()
            .saturating_sub(stream_limits::CODEC_MEMORY + FRAMING_BYTES)
            / 4;
        let staging = self.budget.staging_bytes().saturating_sub(FRAMING_BYTES) / 2;
        Some(server.min(BATCH_BLOB_LIMIT as u64).min(memory).min(staging))
            .filter(|limit| *limit > 0)
    }

    fn start_download_batch(&self, limit: u64) {
        let cache = self.clone();
        tokio::spawn(async move {
            // Collect already-ready readers without adding a timer to the critical path.
            tokio::task::yield_now().await;
            let mut pending = cache.batch_downloads.pending.lock().await;
            let mut requests = Vec::new();
            let mut bytes = 0;
            while let Some(next) = pending.front() {
                if next.response.is_closed() {
                    pending.pop_front();
                    continue;
                }
                let size =
                    u64::try_from(next.remote.size_bytes).expect("admitted batch digest size");
                if bytes + size > limit {
                    break;
                }
                bytes += size;
                requests.push(pending.pop_front().expect("queued download"));
            }
            if !pending.is_empty() {
                cache.start_download_batch(limit);
            }
            drop(pending);
            if !requests.is_empty() {
                cache.download_batch(requests, bytes).await;
            }
        });
    }

    async fn download_batch(&self, requests: Vec<PendingDownload>, bytes: u64) {
        let digests = requests
            .iter()
            .map(|request| request.remote.clone())
            .collect::<Vec<_>>();
        let mut requests = requests.into_iter().map(Some).collect::<Vec<_>>();
        let fetched = self.read_download_batch(digests, bytes);
        tokio::pin!(fetched);
        let fetched = loop {
            tokio::select! {
                result = &mut fetched => break result,
                index = next_cancelled(&mut requests) => {
                    requests[index] = None;
                    if requests.iter().all(Option::is_none) {
                        return;
                    }
                }
            }
        };
        let batch = match fetched {
            Ok(Some(batch)) => batch,
            Ok(None) => {
                self.retry_downloads(
                    requests
                        .into_iter()
                        .flatten()
                        .map(|request| (request, None))
                        .collect(),
                )
                .await;
                return;
            }
            Err(error) => {
                for request in requests.into_iter().flatten() {
                    request.complete(Err(status::remote(
                        "get blob",
                        error.kind(),
                        error.to_string(),
                    )));
                }
                return;
            }
        };
        let mut retries = Vec::new();
        let BatchResponse {
            blobs,
            memory,
            staging,
            permit,
        } = batch;
        let ingest_started = Instant::now();
        let count = blobs.len();
        for (request, blob) in requests.into_iter().zip(blobs) {
            let Some(request) = request else {
                continue;
            };
            if request.response.is_closed() {
                continue;
            }
            let result = match download::blob_read_outcome(blob, &request.remote, "get blob") {
                Ok(Some(bytes)) => match self.local.mirror_blob(&request.digest, &bytes).await {
                    Ok(digest) if digest != request.digest => Err(status::remote(
                        "get blob",
                        ErrorKind::Corrupt,
                        "remote blob did not match requested native digest",
                    )),
                    result => result.map(|_| ()),
                },
                Ok(None) => Err(Error::BlobNotFound(request.digest)),
                Err(retry::Failure::Local(error)) => Err(error),
                Err(retry::Failure::Rpc(status)) => {
                    retries.push((request, Some(status)));
                    continue;
                }
            };
            request.complete(result);
        }
        tracing::debug!(
            blobs = count,
            ingest_seconds = ingest_started.elapsed().as_secs_f64(),
            "ingested remote blob batch"
        );
        drop((memory, staging, permit));
        self.retry_downloads(retries).await;
    }

    async fn retry_downloads(&self, requests: Vec<(PendingDownload, Option<tonic::Status>)>) {
        // Poll every reader's cancellation; transfer admission still bounds physical I/O.
        stream::iter(requests)
            .for_each_concurrent(None, |(mut request, mut first)| async move {
                if request.response.is_closed() {
                    return;
                }
                let digest = &request.digest;
                let remote = &request.remote;
                let result = tokio::select! {
                    result = retry::grpc("get blob", || {
                        let first = first.take();
                        let owner = Arc::clone(&request.owner);
                        async move {
                            if let Some(status) = first {
                                return Err(status.into());
                            }
                            self.stream_remote_blob(digest, remote, owner).await
                        }
                    }) => result,
                    () = request.response.closed() => return,
                };
                request.complete(result);
            })
            .await;
    }
}

async fn next_cancelled(requests: &mut [Option<PendingDownload>]) -> usize {
    std::future::poll_fn(|context| {
        for (index, request) in requests.iter_mut().enumerate() {
            if let Some(request) = request {
                if request.response.poll_closed(context).is_ready() {
                    return std::task::Poll::Ready(index);
                }
            }
        }
        std::task::Poll::Pending
    })
    .await
}

#[cfg(test)]
mod tests;
