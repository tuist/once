//! Observation of the blobs a cache moves to and from its remote tier.
//!
//! Only a remote provider knows whether a blob it hands back came over the
//! network or was already in the local store, and whether an upload sent
//! bytes or found the blob already there. Callers that report those numbers
//! scope an observer around the work that touches the cache, and the
//! provider records each transfer that actually happened into it.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use crate::Digest;

/// Which way a blob moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferDirection {
    /// Pulled from the remote tier into the local store.
    Download,
    /// Pushed from the local store into the remote tier.
    Upload,
}

/// One blob moved between the local store and the remote tier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
    /// Which way the blob moved.
    pub direction: TransferDirection,
    /// The blob's local content digest.
    pub digest: Digest,
    /// The blob's uncompressed size.
    pub size_bytes: u64,
    /// Wall-clock time the transfer took.
    pub duration: Duration,
}

/// Receives the transfers made while it is in scope.
pub trait TransferObserver: Send + Sync {
    /// Called once for every blob that crossed the network.
    fn observe(&self, transfer: Transfer);
}

tokio::task_local! {
    static OBSERVER: Arc<dyn TransferObserver>;
}

/// Run `future` with `observer` receiving every remote transfer it causes.
///
/// The observer follows the task, not the call stack: work the future hands
/// to a spawned task only reports into it if the spawn goes through
/// [`propagate`].
pub async fn observe<F: Future>(observer: Arc<dyn TransferObserver>, future: F) -> F::Output {
    OBSERVER.scope(observer, future).await
}

/// Carry the current observer, if any, into a future that is about to be
/// spawned onto its own task.
pub fn propagate<F>(future: F) -> impl Future<Output = F::Output> + Send
where
    F: Future + Send,
{
    let observer = OBSERVER.try_with(Arc::clone).ok();
    async move {
        match observer {
            Some(observer) => OBSERVER.scope(observer, future).await,
            None => future.await,
        }
    }
}

pub(crate) fn record(
    direction: TransferDirection,
    digest: Digest,
    size_bytes: u64,
    duration: Duration,
) {
    let _ = OBSERVER.try_with(|observer| {
        observer.observe(Transfer {
            direction,
            digest,
            size_bytes,
            duration,
        });
    });
}

/// Collects every transfer it observes, for tests.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct Recorder(std::sync::Mutex<Vec<Transfer>>);

#[cfg(test)]
impl Recorder {
    pub(crate) fn transfers(&self) -> Vec<Transfer> {
        self.0.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl TransferObserver for Recorder {
    fn observe(&self, transfer: Transfer) {
        self.0.lock().unwrap().push(transfer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn download(bytes: &[u8]) {
        record(
            TransferDirection::Download,
            Digest::of_bytes(bytes),
            bytes.len() as u64,
            Duration::ZERO,
        );
    }

    #[tokio::test]
    async fn transfers_outside_a_scope_are_dropped() {
        download(b"nobody is listening");
    }

    #[tokio::test]
    async fn transfers_reach_the_observer_in_scope_and_spawned_through_propagate() {
        let recorder = Arc::new(Recorder::default());
        observe(recorder.clone(), async {
            download(b"inline");
            tokio::spawn(propagate(async { download(b"propagated") }))
                .await
                .unwrap();
            tokio::spawn(async { download(b"detached") }).await.unwrap();
        })
        .await;

        let digests: Vec<Digest> = recorder.transfers().iter().map(|t| t.digest).collect();
        assert_eq!(
            digests,
            vec![Digest::of_bytes(b"inline"), Digest::of_bytes(b"propagated")]
        );
    }
}
