use std::collections::BTreeSet;
use std::future::Future;
#[cfg(test)]
use std::path::Path;
#[cfg(test)]
use std::sync::Arc;

use anyhow::Result;
use futures::stream::FuturesUnordered;
#[cfg(test)]
use futures::FutureExt;
use futures::StreamExt;
use struo_cas::ActionResult;
#[cfg(test)]
use struo_cas::CacheProvider;
#[cfg(test)]
use struo_core::{ResourcePermit, ResourcePool, ResourceRequest};

use super::paths;
#[cfg(test)]
use super::{admission, SourceDigestCache};

pub(super) async fn restore_recovering(
    results: &[ActionResult],
    repair: super::Repair<'_>,
) -> Result<()> {
    let limit = repair.resources.max_parallel_actions().max(1);
    let mut running = FuturesUnordered::new();
    let mut active = BTreeSet::new();
    let mut next = 0;
    let mut failure = None;
    while !running.is_empty() || (failure.is_none() && next < results.len()) {
        if active.is_empty() && failure.is_none() {
            let count = paths::batch_len(&results[next..], repair.workspace, limit);
            for (index, result) in results.iter().enumerate().skip(next).take(count) {
                active.insert(index);
                running.push(indexed(index, repair.restore(result)));
            }
            next += count;
        }
        if failure.is_none()
            && next < results.len()
            && active.len() < limit
            && paths::disjoint_from_results(
                repair.workspace,
                &results[next],
                active.iter().map(|index| &results[*index]),
            )
        {
            active.insert(next);
            running.push(indexed(next, repair.restore(&results[next])));
            next += 1;
            continue;
        }
        let (index, outcome) = running.next().await.expect("scheduled restoration window");
        active.remove(&index);
        if let Err(error) = outcome {
            failure.get_or_insert(error);
        }
    }
    failure.map_or(Ok(()), Err)
}

#[cfg(test)]
pub(super) async fn restore(
    results: &[ActionResult],
    workspace: &Path,
    cache: &CacheProvider,
    digests: Option<&SourceDigestCache>,
    resources: &Arc<ResourcePool>,
) -> Result<()> {
    run(results, workspace, resources, |index, permit| {
        struo_core::with_resource_permit(
            permit,
            super::restore_one(&results[index], workspace, cache, digests),
        )
    })
    .await
}

#[cfg(test)]
async fn run<F, W>(
    results: &[ActionResult],
    workspace: &Path,
    resources: &Arc<ResourcePool>,
    execute: F,
) -> Result<()>
where
    F: Fn(usize, ResourcePermit) -> W,
    W: Future<Output = Result<()>>,
{
    let mut running = FuturesUnordered::new();
    let mut active = BTreeSet::new();
    let mut next = 0;
    let mut failure = None;
    while !running.is_empty() || (failure.is_none() && next < results.len()) {
        if active.is_empty() && failure.is_none() {
            let batch = admission::acquire(&results[next..], workspace, resources).await;
            for (index, permit) in (next..next + batch.count).zip(batch.permits) {
                active.insert(index);
                running.push(indexed(index, execute(index, permit)));
            }
            next += batch.count;
        }
        if failure.is_none() && next < results.len() {
            if let Some(permit) = resources.acquire(ResourceRequest::default()).now_or_never() {
                let disjoint = paths::disjoint_from_results(
                    workspace,
                    &results[next],
                    active.iter().map(|index| &results[*index]),
                );
                if disjoint {
                    active.insert(next);
                    running.push(indexed(next, execute(next, permit)));
                    next += 1;
                    continue;
                }
            }
        }
        let (index, outcome) = running.next().await.expect("admitted restoration window");
        active.remove(&index);
        if let Err(error) = outcome {
            failure.get_or_insert(error);
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

async fn indexed<F: Future>(index: usize, future: F) -> (usize, F::Output) {
    (index, future.await)
}

#[cfg(all(test, not(windows)))]
#[path = "window_tests.rs"]
mod tests;
