use std::path::Path;
#[cfg(test)]
use std::sync::Arc;

use anyhow::Result;
use struo_cas::ActionResult;
#[cfg(test)]
use struo_cas::CacheProvider;
#[cfg(test)]
use struo_core::ResourcePool;

use super::Repair;
#[cfg(test)]
use super::SourceDigestCache;

#[cfg(test)]
mod admission;
mod paths;
mod window;

#[cfg(test)]
#[path = "materialization/benchmarks.rs"]
mod benchmarks;

#[cfg(test)]
#[path = "materialization/stale_benchmarks.rs"]
mod stale_benchmarks;

pub(super) fn disjoint_paths(
    workspace: &Path,
    left: &std::collections::BTreeSet<String>,
    right: &std::collections::BTreeSet<String>,
) -> bool {
    paths::disjoint(workspace, left, right)
}

pub(super) fn independent_paths<'a>(
    workspace: &Path,
    paths: &std::collections::BTreeSet<String>,
    candidates: impl Iterator<Item = &'a str>,
) -> std::collections::BTreeSet<&'a str> {
    self::paths::independent_members(workspace, paths, candidates)
}

pub(super) fn creation_conflicts(
    workspace: &Path,
    created: &str,
    pending: &std::collections::BTreeSet<String>,
) -> bool {
    paths::creation_conflicts(workspace, created, pending)
}

pub(super) async fn restore_recovering(results: &[ActionResult], repair: Repair<'_>) -> Result<()> {
    window::restore_recovering(results, repair).await
}

#[cfg(test)]
pub(super) async fn restore(
    results: &[ActionResult],
    workspace: &Path,
    cache: &CacheProvider,
    source_digest_cache: Option<&SourceDigestCache>,
    resources: &Arc<ResourcePool>,
) -> Result<()> {
    window::restore(results, workspace, cache, source_digest_cache, resources).await
}

#[cfg(test)]
async fn restore_one(
    result: &ActionResult,
    workspace: &Path,
    cache: &CacheProvider,
    source_digest_cache: Option<&SourceDigestCache>,
) -> Result<()> {
    match source_digest_cache {
        Some(digests) => digests
            .materialize_outputs(result, workspace, cache, None)
            .await
            .map_err(anyhow::Error::from),
        None => struo_core::materialize_outputs(result, workspace, cache)
            .await
            .map_err(anyhow::Error::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use struo_cas::{Cas, Digest};
    use struo_core::{Action, CopyPathMode, ResourceLimits, ResourceRequest, WorkspacePath};

    fn result(path: &str, digest: Digest) -> ActionResult {
        ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs: BTreeMap::from([(path.into(), digest)]),
        }
    }

    #[tokio::test]
    async fn restores_wait_for_the_shared_resource_budget() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(Cas::open(workspace.path().join("cas")));
        let digest = cache.put_blob(b"output").await.unwrap();
        let results = [result("a", digest), result("b", digest)];
        let resources = Arc::new(ResourcePool::new(ResourceLimits::new(4, 1024 * 1024)));
        let held = resources.acquire(ResourceRequest::default()).await;
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(20),
            restore(&results, workspace.path(), &cache, None, &resources)
        )
        .await
        .is_err());
        assert!(!workspace.path().join("a").exists());
        assert!(!workspace.path().join("b").exists());
        drop(held);
        restore(&results, workspace.path(), &cache, None, &resources)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(workspace.path().join("a")).unwrap(),
            b"output"
        );
        assert_eq!(
            std::fs::read(workspace.path().join("b")).unwrap(),
            b"output"
        );
    }

    #[tokio::test]
    async fn a_failed_batch_finishes_independent_restores_before_returning() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(Cas::open(workspace.path().join("cas")));
        let digest = cache.put_blob(b"output").await.unwrap();
        let results = [
            result("missing", Digest::of_bytes(b"absent")),
            result("available", digest),
        ];
        let resources = Arc::new(ResourcePool::new(ResourceLimits::new(
            4,
            1024 * 1024 * 1024,
        )));
        let batched =
            paths::batch_len(&results, workspace.path(), resources.max_parallel_actions()) > 1;
        assert!(
            restore(&results, workspace.path(), &cache, None, &resources)
                .await
                .is_err()
        );
        if batched {
            assert_eq!(
                std::fs::read(workspace.path().join("available")).unwrap(),
                b"output"
            );
        } else {
            assert!(!workspace.path().join("available").exists());
        }
        assert!(!workspace.path().join("missing").exists());
    }

    #[tokio::test]
    async fn parent_restoration_cannot_invalidate_a_child_prepared_early() {
        let source = tempfile::tempdir().unwrap();
        std::fs::create_dir(source.path().join("source")).unwrap();
        std::fs::write(source.path().join("source/child"), b"old").unwrap();
        let cache = CacheProvider::Local(Cas::open(source.path().join("cas")));
        let action = Action::CopyPath {
            sources: vec![WorkspacePath::try_from("source").unwrap()],
            destination: WorkspacePath::try_from("tree").unwrap(),
            mode: CopyPathMode::Tree,
            input_digest: None,
        };
        let parent = struo_core::run_uncached(&action, source.path(), &cache, false)
            .await
            .unwrap();
        let child = result("tree/child", cache.put_blob(b"new").await.unwrap());
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join("tree")).unwrap();
        std::fs::write(workspace.path().join("tree/child"), b"new").unwrap();
        let resources = Arc::new(ResourcePool::new(ResourceLimits::new(
            4,
            1024 * 1024 * 1024,
        )));
        restore(&[parent, child], workspace.path(), &cache, None, &resources)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(workspace.path().join("tree/child")).unwrap(),
            b"new"
        );
    }
}
