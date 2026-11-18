use std::path::Path;
use std::sync::Arc;

use futures::FutureExt;
use struo_cas::ActionResult;
use struo_core::{ResourcePermit, ResourcePool, ResourceRequest};

use super::paths;

pub(super) struct Batch {
    pub(super) count: usize,
    pub(super) permits: Vec<ResourcePermit>,
}

pub(super) async fn acquire(
    results: &[ActionResult],
    workspace: &Path,
    resources: &Arc<ResourcePool>,
) -> Batch {
    let first = resources.acquire(ResourceRequest::default()).await;
    let maximum = paths::batch_len(results, workspace, resources.max_parallel_actions());
    let mut permits = vec![first];
    while permits.len() < maximum {
        let Some(permit) = resources.acquire(ResourceRequest::default()).now_or_never() else {
            break;
        };
        permits.push(permit);
    }
    Batch {
        count: permits.len(),
        permits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::time::Duration;
    use struo_cas::Digest;
    use struo_core::ResourceLimits;

    fn result(path: &str) -> ActionResult {
        ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs: BTreeMap::from([(path.into(), Digest::of_bytes(path.as_bytes()))]),
        }
    }

    fn pool() -> Arc<ResourcePool> {
        Arc::new(ResourcePool::new(ResourceLimits::new(
            4,
            1024 * 1024 * 1024,
        )))
    }

    #[tokio::test]
    async fn every_batch_member_has_a_reservation() {
        let workspace = tempfile::tempdir().unwrap();
        let resources = pool();
        let results = [result("a"), result("b"), result("c"), result("d")];
        let expected = paths::batch_len(&results, workspace.path(), 4);
        let batch = acquire(&results, workspace.path(), &resources).await;
        assert_eq!(batch.count, expected);
        assert!(resources
            .acquire(ResourceRequest::new(5 - batch.count, 0))
            .now_or_never()
            .is_none());
        drop(batch);
        assert!(resources
            .acquire(ResourceRequest::new(4, 1024 * 1024 * 1024))
            .now_or_never()
            .is_some());
    }

    #[tokio::test]
    async fn a_partially_busy_pool_still_allows_serial_progress() {
        let workspace = tempfile::tempdir().unwrap();
        let resources = pool();
        let memory = resources.limits().default_action_memory_bytes;
        let _held = resources.acquire(ResourceRequest::new(3, memory * 3)).await;
        let results = [result("a"), result("b")];
        let batch = tokio::time::timeout(
            Duration::from_secs(2),
            acquire(&results, workspace.path(), &resources),
        )
        .await
        .unwrap();
        assert_eq!(batch.count, 1);
        assert!(resources
            .acquire(ResourceRequest::default())
            .now_or_never()
            .is_none());
        drop(batch);
        assert!(resources
            .acquire(ResourceRequest::default())
            .now_or_never()
            .is_some());
    }

    #[tokio::test]
    async fn a_busy_pool_uses_smaller_disjoint_batches() {
        let workspace = tempfile::tempdir().unwrap();
        let results = [result("a"), result("b"), result("c"), result("d")];
        let maximum = paths::batch_len(&results, workspace.path(), 4);
        for occupied in [1, 2] {
            let resources = pool();
            let memory = resources.limits().default_action_memory_bytes;
            let _held = resources
                .acquire(ResourceRequest::new(
                    occupied,
                    memory * u64::try_from(occupied).unwrap(),
                ))
                .await;
            let batch = acquire(&results, workspace.path(), &resources).await;
            assert_eq!(batch.count, maximum.min(4 - occupied));
            assert!(resources
                .acquire(ResourceRequest::new(5 - occupied - batch.count, 0))
                .now_or_never()
                .is_none());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn aliases_created_while_waiting_for_admission_are_serialized() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("tree")).unwrap();
        let resources = pool();
        let held = resources
            .acquire(ResourceRequest::new(4, 1024 * 1024 * 1024))
            .await;
        let results = [result("tree"), result("alias/child")];
        let batch = acquire(&results, workspace.path(), &resources);
        tokio::pin!(batch);
        tokio::select! {
            _ = &mut batch => panic!("batch did not wait for admission"),
            () = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
        std::os::unix::fs::symlink("tree", workspace.path().join("alias")).unwrap();
        drop(held);
        assert_eq!(batch.await.count, 1);
    }
}
