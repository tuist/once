use std::collections::VecDeque;
use std::future::Future;
use std::sync::Arc;

use struo_cas::{ActionResult, CacheProvider, Digest};
use tokio::task::JoinSet;

use crate::{Error, ResourcePermit, Result};

use super::{PrefetchedOutput, StagingDir, WorkerSlots, RESTORE_PREFETCH_CONCURRENCY};

pub(super) async fn restore(
    result: &ActionResult,
    cache: &CacheProvider,
    staging: &Arc<StagingDir>,
) -> Result<Vec<PrefetchedOutput>> {
    cache
        .prefetch_blobs_guarded(
            &result.outputs.values().copied().collect::<Vec<_>>(),
            (Arc::clone(staging), crate::resource_scope::current()),
        )
        .await?;
    run(result, |_, rel, digest, owner| {
        let cache = cache.clone();
        let staging = Arc::clone(staging);
        struo_cas::transfer::propagate(async move {
            let parent = staging.path().to_path_buf();
            let blob = cache
                .stage_blob_guarded(&digest, &parent, (staging, owner))
                .await?;
            Ok(PrefetchedOutput { rel, digest, blob })
        })
    })
    .await
}

async fn run<F, W, O>(result: &ActionResult, execute: F) -> Result<Vec<O>>
where
    F: Fn(usize, String, Digest, Option<Arc<ResourcePermit>>) -> W,
    W: Future<Output = Result<O>> + Send + 'static,
    O: Send + 'static,
{
    let mut pending = result
        .outputs
        .iter()
        .enumerate()
        .map(|(index, (path, digest))| (index, path.clone(), *digest))
        .collect::<VecDeque<_>>();
    let mut slots = WorkerSlots::new(pending.len().min(RESTORE_PREFETCH_CONCURRENCY));
    let mut tasks = JoinSet::new();
    let mut outputs = (0..pending.len()).map(|_| None).collect::<Vec<_>>();
    while let Some(slot) = slots.take() {
        let (index, path, digest) = pending.pop_front().expect("admitted prefetch output");
        let work = execute(index, path, digest, slot.owner);
        tasks.spawn(async move { work.await.map(|output| (slot.index, index, output)) });
    }
    while let Some(joined) = tasks.join_next().await {
        let (slot, index, output) = joined.map_err(|source| Error::RestoreOutput {
            path: "cached output prefetch".into(),
            source: std::io::Error::other(source.to_string()),
        })??;
        outputs[index] = Some(output);
        if let Some((index, path, digest)) = pending.pop_front() {
            slots.release(slot);
            let slot = slots.take().expect("completed prefetch slot");
            let work = execute(index, path, digest, slot.owner);
            tasks.spawn(async move { work.await.map(|output| (slot.index, index, output)) });
        } else {
            slots.retire(slot);
        }
    }
    outputs
        .into_iter()
        .enumerate()
        .map(|(index, output)| {
            output.ok_or_else(|| Error::RestoreOutput {
                path: format!("cached output prefetch[{index}]"),
                source: std::io::Error::other("prefetch task did not produce an output"),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{with_resource_permit, ResourceLimits, ResourcePool, ResourceRequest};
    use futures::FutureExt;
    use std::collections::BTreeMap;
    use std::time::Duration;

    fn result(count: usize) -> ActionResult {
        ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs: (0..count)
                .map(|index| {
                    (
                        format!("output-{index}"),
                        Digest::of_bytes(&index.to_le_bytes()),
                    )
                })
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[tokio::test]
    async fn saturated_pool_prefetches_serially_without_waiting_for_extra_capacity() {
        let pool = Arc::new(ResourcePool::new(ResourceLimits::new(
            2,
            1024 * 1024 * 1024,
        )));
        let held = pool.acquire(ResourceRequest::default()).await;
        let base = pool.acquire(ResourceRequest::default()).await;
        let (entered, mut ready) = tokio::sync::mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let gate = Arc::clone(&release);
        let task = tokio::spawn(with_resource_permit(base, async move {
            run(&result(4), move |index, _, _, owner| {
                let entered = entered.clone();
                let gate = Arc::clone(&gate);
                async move {
                    let _owner = owner.expect("prefetch retains an admitted owner");
                    entered.send(index).unwrap();
                    gate.acquire().await.unwrap().forget();
                    Ok(index)
                }
            })
            .await
            .unwrap()
        }));
        for index in 0..4 {
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(5), ready.recv())
                    .await
                    .unwrap(),
                Some(index)
            );
            assert!(ready.try_recv().is_err());
            release.add_permits(1);
        }
        assert_eq!(task.await.unwrap(), vec![0, 1, 2, 3]);
        drop(held);
        assert!(pool
            .acquire(ResourceRequest::new(2, 1024 * 1024 * 1024))
            .now_or_never()
            .is_some());
    }

    #[tokio::test]
    async fn cancellation_retains_the_slot_until_blocking_work_finishes() {
        let pool = Arc::new(ResourcePool::new(ResourceLimits::new(
            1,
            1024 * 1024 * 1024,
        )));
        let base = pool.acquire(ResourceRequest::default()).await;
        let (entered, ready) = tokio::sync::oneshot::channel();
        let entered = Arc::new(std::sync::Mutex::new(Some(entered)));
        let (release, wait) = std::sync::mpsc::channel();
        let wait = Arc::new(std::sync::Mutex::new(wait));
        let task = tokio::spawn(with_resource_permit(base, async move {
            run(&result(4), move |index, _, _, owner| {
                let entered = Arc::clone(&entered);
                let wait = Arc::clone(&wait);
                async move {
                    tokio::task::spawn_blocking(move || {
                        let _owner = owner.unwrap();
                        entered.lock().unwrap().take().unwrap().send(()).unwrap();
                        wait.lock()
                            .unwrap()
                            .recv_timeout(Duration::from_secs(5))
                            .unwrap();
                        index
                    })
                    .await
                    .map_err(|source| Error::RestoreOutput {
                        path: "prefetch test".into(),
                        source: std::io::Error::other(source.to_string()),
                    })
                }
            })
            .await
        }));
        ready.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(pool
            .acquire(ResourceRequest::default())
            .now_or_never()
            .is_none());
        release.send(()).unwrap();
        drop(
            tokio::time::timeout(
                Duration::from_secs(5),
                pool.acquire(ResourceRequest::default()),
            )
            .await
            .unwrap(),
        );
    }

    #[tokio::test]
    async fn completed_extra_slots_are_released_before_the_base_worker_finishes() {
        let pool = Arc::new(ResourcePool::new(ResourceLimits::new(
            2,
            1024 * 1024 * 1024,
        )));
        let base = pool.acquire(ResourceRequest::default()).await;
        let (entered, ready) = tokio::sync::oneshot::channel();
        let entered = Arc::new(std::sync::Mutex::new(Some(entered)));
        let release = Arc::new(tokio::sync::Notify::new());
        let gate = Arc::clone(&release);
        let task = tokio::spawn(with_resource_permit(base, async move {
            run(&result(2), move |index, _, _, owner| {
                let entered = Arc::clone(&entered);
                let gate = Arc::clone(&gate);
                async move {
                    let _owner = owner.unwrap();
                    if index == 0 {
                        entered.lock().unwrap().take().unwrap().send(()).unwrap();
                        gate.notified().await;
                    }
                    Ok(index)
                }
            })
            .await
            .unwrap()
        }));
        ready.await.unwrap();
        let permit = tokio::time::timeout(
            Duration::from_secs(5),
            pool.acquire(ResourceRequest::default()),
        )
        .await
        .unwrap();
        drop(permit);
        release.notify_one();
        assert_eq!(task.await.unwrap(), vec![0, 1]);
    }
}
