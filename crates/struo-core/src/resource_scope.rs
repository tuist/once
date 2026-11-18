use std::future::Future;
use std::sync::Arc;

use crate::ResourcePermit;

tokio::task_local! {
    static PERMIT: Arc<ResourcePermit>;
}

/// Retain an admitted reservation for execution and any owned blocking work.
///
/// Blocking workers explicitly clone the reservation before spawning. Dropping
/// this future cancels the caller without releasing resources still in use.
pub async fn with_resource_permit<F: Future>(permit: ResourcePermit, future: F) -> F::Output {
    PERMIT.scope(Arc::new(permit), future).await
}

/// Scope filesystem workers to an existing executor lease without acquiring again.
/// Owned workers also retain the lease's domain reservations and lifetime owners.
pub async fn with_execution_permit<F: Future>(
    permit: crate::ExecutionPermit,
    future: F,
) -> F::Output {
    with_resource_permit(permit.scoped_host(), future).await
}

pub(crate) fn current() -> Option<Arc<ResourcePermit>> {
    PERMIT.try_with(Arc::clone).ok()
}

pub(crate) fn owned_blocking<F, O>(operation: F) -> impl FnOnce() -> O
where
    F: FnOnce() -> O,
{
    let owner = current();
    move || {
        let _owner = owner;
        operation()
    }
}

#[cfg(test)]
mod execution_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ResourceLimits, ResourcePool, ResourceRequest};
    use futures::FutureExt;

    #[tokio::test]
    async fn cancelled_validation_keeps_its_owned_reservation_until_worker_completion() {
        use std::time::Duration;
        let pool = Arc::new(ResourcePool::new(
            ResourceLimits::new(1, 1024 * 1024 * 1024).with_slot_limit("VALIDATE", 1),
        ));
        let request = ResourceRequest::default().with_slot("VALIDATE", 1);
        let permit = pool.acquire(request.clone()).await;
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let task = tokio::spawn(with_resource_permit(permit, async move {
            tokio::task::spawn_blocking(owned_blocking(move || {
                started.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
            }))
            .await
            .unwrap();
        }));
        tokio::time::timeout(Duration::from_secs(5), ready)
            .await
            .unwrap()
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(pool.acquire(request.clone()).now_or_never().is_none());
        release.send(()).unwrap();
        drop(
            tokio::time::timeout(Duration::from_secs(5), pool.acquire(request))
                .await
                .unwrap(),
        );
    }

    #[tokio::test]
    async fn cancelling_producer_keeps_cpu_memory_and_named_slots_reserved() {
        use std::time::Duration;
        let temp = tempfile::tempdir().unwrap();
        let cache = struo_cas::CacheProvider::open_local(temp.path());
        let pool = Arc::new(ResourcePool::new(
            ResourceLimits::new(1, 1024 * 1024 * 1024).with_slot_limit("CAPTURE", 1),
        ));
        let request = ResourceRequest::default().with_slot("CAPTURE", 1);
        let permit = pool.acquire(request.clone()).await;
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let task = tokio::spawn(with_resource_permit(permit, async move {
            cache
                .put_produced(current().unwrap(), move |writer| {
                    writer.write_all(&vec![3; 512 * 1024])?;
                    started.send(()).unwrap();
                    wait.recv_timeout(Duration::from_secs(5)).unwrap();
                    writer.write_all(b"cancelled")
                })
                .await
        }));
        tokio::time::timeout(Duration::from_secs(5), ready)
            .await
            .unwrap()
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(pool.acquire(request.clone()).now_or_never().is_none());
        release.send(()).unwrap();
        let next = tokio::time::timeout(Duration::from_secs(5), pool.acquire(request))
            .await
            .unwrap();
        drop(next);
    }

    #[tokio::test]
    async fn cancelling_pipeline_retains_both_worker_reservations() {
        use std::time::Duration;
        let temp = tempfile::tempdir().unwrap();
        let cache = struo_cas::CacheProvider::open_local(temp.path());
        let pool = Arc::new(ResourcePool::new(ResourceLimits::new(
            2,
            1024 * 1024 * 1024,
        )));
        let permit = pool.acquire(ResourceRequest::default()).await;
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let task = tokio::spawn(with_resource_permit(permit, async move {
            let base = current().unwrap();
            let extra = base.try_additional_default().unwrap();
            cache
                .put_produced_pipelined((base, extra), move |writer| {
                    writer.write_all(&vec![5; 1024 * 1024])?;
                    started.send(()).unwrap();
                    wait.recv_timeout(Duration::from_secs(5)).unwrap();
                    writer.write_all(b"cancelled")
                })
                .await
        }));
        tokio::time::timeout(Duration::from_secs(5), ready)
            .await
            .unwrap()
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(pool
            .acquire(ResourceRequest::default())
            .now_or_never()
            .is_none());
        release.send(()).unwrap();
        let first = tokio::time::timeout(
            Duration::from_secs(5),
            pool.acquire(ResourceRequest::default()),
        )
        .await
        .unwrap();
        let second = tokio::time::timeout(
            Duration::from_secs(5),
            pool.acquire(ResourceRequest::default()),
        )
        .await
        .unwrap();
        drop((first, second));
    }

    #[tokio::test]
    async fn cancelled_staging_keeps_the_decoder_reservation_through_worker_drop() {
        struct Guard {
            _permit: Arc<ResourcePermit>,
            ready: Option<tokio::sync::oneshot::Sender<()>>,
            wait: std::sync::mpsc::Receiver<()>,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                self.ready.take().unwrap().send(()).unwrap();
                self.wait
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let cache = struo_cas::CacheProvider::open_local(temp.path().join("cas"));
        let digest = cache.put_blob(&vec![b'x'; 1024 * 1024]).await.unwrap();
        let pool = Arc::new(ResourcePool::new(ResourceLimits::new(
            1,
            1024 * 1024 * 1024,
        )));
        let permit = pool.acquire(ResourceRequest::default()).await;
        let (ready, entered) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let parent = temp.path().to_path_buf();
        let task = tokio::spawn(with_resource_permit(permit, async move {
            cache
                .stage_blob_guarded(
                    &digest,
                    &parent,
                    Guard {
                        _permit: current().unwrap(),
                        ready: Some(ready),
                        wait,
                    },
                )
                .await
        }));
        tokio::time::timeout(std::time::Duration::from_secs(5), entered)
            .await
            .unwrap()
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(pool
            .acquire(ResourceRequest::default())
            .now_or_never()
            .is_none());
        release.send(()).unwrap();
        let next = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            pool.acquire(ResourceRequest::default()),
        )
        .await
        .unwrap();
        drop(next);
    }

    #[tokio::test]
    async fn additional_reservation_never_waits_or_bypasses_memory_capacity() {
        let pool = Arc::new(ResourcePool::new(ResourceLimits::new(
            2,
            crate::resources::DEFAULT_ACTION_MEMORY_BYTES,
        )));
        let permit = pool.acquire(ResourceRequest::default()).await;
        assert!(permit.try_additional_default().is_none());
        drop(permit);
        assert!(pool
            .acquire(ResourceRequest::default())
            .now_or_never()
            .is_some());
    }

    #[tokio::test]
    async fn scope_restores_outer_reservation_after_nested_execution() {
        let pool = Arc::new(ResourcePool::new(ResourceLimits::new(
            2,
            1024 * 1024 * 1024,
        )));
        let outer = pool.acquire(ResourceRequest::default()).await;
        with_resource_permit(outer, async {
            let original = current().unwrap();
            let inner = pool.acquire(ResourceRequest::default()).await;
            with_resource_permit(inner, async {
                assert!(!Arc::ptr_eq(&original, &current().unwrap()));
            })
            .await;
            assert!(Arc::ptr_eq(&original, &current().unwrap()));
        })
        .await;
        assert!(current().is_none());
    }
}
