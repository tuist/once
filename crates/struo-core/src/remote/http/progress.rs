use crate::{Error, RemoteIoLimits, Result};
use futures::{Stream, StreamExt};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;
use tokio::time::Instant;

#[derive(Clone)]
pub(in crate::remote) struct Progress(Arc<Inner>);

struct Inner {
    state: Mutex<State>,
    ended: tokio::sync::Notify,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct State {
    latest: Instant,
    finished: Option<Instant>,
}

impl Progress {
    pub(in crate::remote) fn new() -> Self {
        Self(Arc::new(Inner {
            state: Mutex::new(State {
                latest: Instant::now(),
                finished: None,
            }),
            ended: tokio::sync::Notify::new(),
        }))
    }

    fn state(&self) -> State {
        *self
            .0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn note(&self) {
        self.0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .latest = Instant::now();
    }

    fn finish(&self) {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.finished.is_none() {
            state.finished = Some(Instant::now());
            drop(state);
            self.0.ended.notify_one();
        }
    }

    pub(in crate::remote) fn track<S: Stream>(&self, stream: S) -> impl Stream<Item = S::Item> {
        let progress = self.clone();
        let mut stream = Box::pin(stream);
        futures::stream::poll_fn(move |cx| {
            let result = stream.as_mut().poll_next(cx);
            match &result {
                Poll::Ready(Some(_)) => progress.note(),
                Poll::Ready(None) => progress.finish(),
                Poll::Pending => {}
            }
            result
        })
    }

    pub(in crate::remote) fn track_bytes<S, T, E>(
        &self,
        stream: S,
        length: u64,
    ) -> impl Stream<Item = std::result::Result<T, E>>
    where
        S: Stream<Item = std::result::Result<T, E>>,
        T: AsRef<[u8]>,
    {
        let progress = self.clone();
        let mut remaining = length;
        if remaining == 0 {
            progress.finish();
        }
        self.track(stream).map(move |chunk| {
            if let Ok(bytes) = &chunk {
                remaining = remaining
                    .saturating_sub(u64::try_from(bytes.as_ref().len()).unwrap_or(u64::MAX));
                if remaining == 0 {
                    progress.finish();
                }
            }
            chunk
        })
    }

    pub(in crate::remote) async fn watch_upload<T>(
        &self,
        executor: &'static str,
        limits: RemoteIoLimits,
        future: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        self.watch_inner(
            executor,
            "upload",
            limits.stall_timeout,
            Some(limits.upload_ack_timeout),
            future,
        )
        .await
    }

    pub(in crate::remote) async fn watch<T>(
        &self,
        executor: &'static str,
        phase: &'static str,
        timeout: Duration,
        future: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        self.watch_inner(executor, phase, timeout, None, future)
            .await
    }

    async fn watch_inner<T>(
        &self,
        executor: &'static str,
        phase: &'static str,
        stall: Duration,
        acknowledgement: Option<Duration>,
        future: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        tokio::pin!(future);
        loop {
            let state = self.state();
            let (start, timeout, phase) = match (state.finished, acknowledgement) {
                (Some(start), Some(timeout)) => (start, timeout, "upload acknowledgement"),
                _ => (state.latest, stall, phase),
            };
            tokio::select! {
                biased;
                result = &mut future => return result,
                () = self.0.ended.notified() => {},
                () = tokio::time::sleep_until(start + timeout) => {
                    if self.state() == state {
                        return Err(Error::ExecutorStalled { executor: executor.into(), phase, timeout });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test(start_paused = true)]
    async fn progress_allows_a_transfer_longer_than_the_stall_deadline() {
        let progress = Progress::new();
        let writer = progress.clone();
        progress
            .watch("test", "upload", Duration::from_secs(2), async move {
                for _ in 0..8 {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    writer.note();
                }
                Ok(())
            })
            .await
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_upload_body_has_a_bound() {
        let error = Progress::new()
            .watch(
                "test",
                "upload",
                Duration::from_secs(2),
                std::future::pending::<Result<()>>(),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            Error::ExecutorStalled {
                phase: "upload",
                ..
            }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn download_response_headers_have_a_stall_bound() {
        let error = Progress::new()
            .watch(
                "test",
                "download headers",
                Duration::from_secs(2),
                std::future::pending::<Result<()>>(),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            Error::ExecutorStalled {
                phase: "download headers",
                ..
            }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn known_length_completion_does_not_require_hyper_to_poll_eof() {
        let progress = Progress::new();
        let stream = futures::stream::iter([Ok::<_, std::io::Error>(b"data".to_vec())]);
        let mut body = Box::pin(progress.track_bytes(stream, 4));
        assert_eq!(body.next().await.unwrap().unwrap(), b"data");
        let limits = RemoteIoLimits {
            stall_timeout: Duration::from_secs(2),
            upload_ack_timeout: Duration::from_secs(30),
            ..RemoteIoLimits::default()
        };
        progress
            .watch_upload("test", limits, async {
                tokio::time::sleep(Duration::from_secs(20)).await;
                Ok(())
            })
            .await
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn upload_eof_starts_a_separate_acknowledgement_bound() {
        let progress = Progress::new();
        let writer = progress.clone();
        let limits = RemoteIoLimits {
            stall_timeout: Duration::from_secs(2),
            upload_ack_timeout: Duration::from_secs(30),
            ..RemoteIoLimits::default()
        };
        progress
            .watch_upload("test", limits, async move {
                let mut body = Box::pin(writer.track(futures::stream::iter([1, 2])));
                assert_eq!(body.next().await, Some(1));
                tokio::time::sleep(Duration::from_secs(1)).await;
                assert_eq!(body.next().await, Some(2));
                assert_eq!(body.next().await, None);
                tokio::time::sleep(Duration::from_secs(20)).await;
                Ok(())
            })
            .await
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn acknowledgement_deadline_resets_even_when_shorter_than_body_stall() {
        let progress = Progress::new();
        let writer = progress.clone();
        let limits = RemoteIoLimits {
            stall_timeout: Duration::from_secs(30),
            upload_ack_timeout: Duration::from_secs(1),
            ..RemoteIoLimits::default()
        };
        let error = progress
            .watch_upload("test", limits, async move {
                let mut body = Box::pin(writer.track(futures::stream::empty::<u8>()));
                assert_eq!(body.next().await, None);
                tokio::time::sleep(Duration::from_secs(2)).await;
                Ok(())
            })
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            Error::ExecutorStalled {
                phase: "upload acknowledgement",
                ..
            }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn missing_upload_acknowledgement_remains_bounded() {
        let progress = Progress::new();
        let writer = progress.clone();
        let limits = RemoteIoLimits {
            stall_timeout: Duration::from_secs(2),
            upload_ack_timeout: Duration::from_secs(30),
            ..RemoteIoLimits::default()
        };
        let error = progress
            .watch_upload("test", limits, async move {
                let mut body = Box::pin(writer.track(futures::stream::empty::<u8>()));
                assert_eq!(body.next().await, None);
                std::future::pending::<Result<()>>().await
            })
            .await
            .unwrap_err();
        assert!(
            matches!(error, Error::ExecutorStalled { phase: "upload acknowledgement", timeout, .. } if timeout == Duration::from_secs(30))
        );
    }
}
