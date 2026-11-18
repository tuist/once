use super::*;
use crate::{Executor, ExecutorResources, ResourceLimits, ResourcePool, ResourceRequest};
use futures::FutureExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

struct Owner(Arc<AtomicBool>);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[tokio::test]
async fn extra_worker_retains_execution_owners_and_the_original_reservation() {
    let pool = Arc::new(ResourcePool::new(
        ResourceLimits::new(2, 1024 * 1024 * 1024).with_slot_limit("OWNED", 1),
    ));
    let executors = ExecutorResources::new(Arc::clone(&pool));
    let request = ResourceRequest::default().with_slot("OWNED", 1);
    let released = Arc::new(AtomicBool::new(false));
    let lease = executors
        .acquire(Executor::Local, &request)
        .await
        .with_lifetime_owner(Owner(Arc::clone(&released)));
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (finished, done) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(with_execution_permit(lease, async move {
        let extra = current().unwrap().try_additional_default().unwrap();
        tokio::task::spawn_blocking(move || {
            entered.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(5)).unwrap();
            drop(extra);
            finished.send(()).unwrap();
        })
        .await
        .unwrap();
    }));
    tokio::time::timeout(Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(!released.load(Ordering::Acquire));
    assert!(pool.acquire(request).now_or_never().is_none());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), done)
        .await
        .unwrap()
        .unwrap();
    assert!(released.load(Ordering::Acquire));
    assert!(pool
        .acquire(ResourceRequest::new(2, 1024 * 1024 * 1024))
        .now_or_never()
        .is_some());
}
