pub(super) fn run<F: FnOnce() -> O, O>(operation: F) -> O {
    if tokio::runtime::Handle::try_current()
        .is_ok_and(|handle| handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)
    {
        tokio::task::block_in_place(operation)
    } else {
        operation()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{with_resource_permit, ResourceLimits, ResourcePool, ResourceRequest};
    use futures::FutureExt;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn synchronous_commits_leave_the_executor_available() {
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let coordinator = tokio::spawn(async move {
            ready.await.unwrap();
            release.send(()).unwrap();
        });
        let commit = tokio::spawn(async move {
            run(move || {
                entered.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
                7
            })
        });
        assert_eq!(commit.await.unwrap(), 7);
        coordinator.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn cancellation_cannot_drop_an_in_progress_commit_or_its_reservation() {
        let pool = Arc::new(ResourcePool::new(ResourceLimits::new(
            1,
            1024 * 1024 * 1024,
        )));
        let permit = pool.acquire(ResourceRequest::default()).await;
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let marker = Arc::clone(&finished);
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let task = tokio::spawn(with_resource_permit(permit, async move {
            run(move || {
                entered.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
                marker.store(true, std::sync::atomic::Ordering::SeqCst);
            });
        }));
        ready.await.unwrap();
        task.abort();
        assert!(pool
            .acquire(ResourceRequest::default())
            .now_or_never()
            .is_none());
        assert!(!finished.load(std::sync::atomic::Ordering::SeqCst));
        release.send(()).unwrap();
        let _ = task.await;
        assert!(finished.load(std::sync::atomic::Ordering::SeqCst));
        assert!(pool
            .acquire(ResourceRequest::default())
            .now_or_never()
            .is_some());
    }

    #[tokio::test]
    async fn current_thread_callers_keep_synchronous_commit_semantics() {
        assert_eq!(run(|| 7), 7);
    }

    #[test]
    fn callers_without_a_runtime_keep_synchronous_commit_semantics() {
        assert_eq!(run(|| 7), 7);
    }
}
