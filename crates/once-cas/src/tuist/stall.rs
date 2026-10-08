use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::{Stream, StreamExt};
use tokio::time::Instant;
use tonic::Status;

/// When a request stream last handed the transport a message. HTTP/2 only
/// pulls the next message once flow control has room for the previous one,
/// so this moves with the bytes the peer accepts, not with local buffering.
pub(super) struct Progress {
    started: Instant,
    last_ms: AtomicU64,
}

impl Progress {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Instant::now(),
            last_ms: AtomicU64::new(0),
        })
    }

    fn touch(&self) {
        let elapsed = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.last_ms.store(elapsed, Ordering::Relaxed);
    }

    fn idle_deadline(&self, timeout: Duration) -> Instant {
        self.started + Duration::from_millis(self.last_ms.load(Ordering::Relaxed)) + timeout
    }
}

pub(super) fn tracked<S: Stream>(
    stream: S,
    progress: Arc<Progress>,
) -> impl Stream<Item = S::Item> {
    stream.inspect(move |_| progress.touch())
}

/// Runs `call` until it finishes or its request stream stops making progress
/// for `timeout`. Unlike a deadline on the whole call, this does not cap how
/// large a blob a slow but healthy link can upload.
pub(super) async fn unless_stalled<T>(
    progress: &Progress,
    timeout: Duration,
    call: impl Future<Output = std::result::Result<T, Status>>,
) -> std::result::Result<T, Status> {
    tokio::pin!(call);
    loop {
        tokio::select! {
            result = &mut call => return result,
            () = tokio::time::sleep_until(progress.idle_deadline(timeout)) => {
                if progress.idle_deadline(timeout) <= Instant::now() {
                    return Err(Status::deadline_exceeded(format!(
                        "upload made no progress for {timeout:?}"
                    )));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn fails_once_the_stream_stops_advancing() {
        let progress = Progress::new();
        let call = std::future::pending::<std::result::Result<(), Status>>();
        let status = unless_stalled(&progress, Duration::from_secs(30), call)
            .await
            .unwrap_err();
        assert_eq!(status.code(), tonic::Code::DeadlineExceeded);
        assert_eq!(progress.started.elapsed(), Duration::from_secs(30));
    }

    #[tokio::test(start_paused = true)]
    async fn a_stream_that_keeps_advancing_outlives_the_timeout() {
        let progress = Progress::new();
        let messages = tracked(
            futures::stream::iter(0..10).then(|message| async move {
                tokio::time::sleep(Duration::from_secs(20)).await;
                message
            }),
            Arc::clone(&progress),
        );
        let call = async { Ok::<_, Status>(messages.collect::<Vec<_>>().await.len()) };
        let sent = unless_stalled(&progress, Duration::from_secs(30), call)
            .await
            .unwrap();
        assert_eq!(sent, 10);
        assert_eq!(progress.started.elapsed(), Duration::from_secs(200));
    }
}
