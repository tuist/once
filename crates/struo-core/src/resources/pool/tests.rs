use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Waker};
use std::time::Duration;

use super::*;
use crate::Error;

fn pool(limits: ResourceLimits) -> Arc<ResourcePool> {
    Arc::new(ResourcePool::new(limits))
}

async fn pending<F: Future>(future: F) {
    assert!(
        tokio::time::timeout(Duration::from_millis(50), future)
            .await
            .is_err(),
        "expected the request to keep waiting"
    );
}

async fn ready<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("expected the request to be admitted")
}

#[tokio::test]
async fn cpu_slots_block_until_released() {
    let pool = pool(ResourceLimits::new(1, 0));
    let first = pool.acquire(ResourceRequest::default()).await;
    let waiting_pool = Arc::clone(&pool);
    let waiting = tokio::spawn(async move {
        let _permit = waiting_pool.acquire(ResourceRequest::default()).await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!waiting.is_finished());

    drop(first);
    ready(waiting).await.unwrap();
}

#[tokio::test]
async fn memory_budget_blocks_until_released() {
    let pool = pool(ResourceLimits::new(4, 100));
    let first = pool.acquire(ResourceRequest::new(1, 80)).await;
    let waiting_pool = Arc::clone(&pool);
    let waiting = tokio::spawn(async move {
        let _permit = waiting_pool.acquire(ResourceRequest::new(1, 40)).await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!waiting.is_finished());

    drop(first);
    ready(waiting).await.unwrap();
}

#[tokio::test]
async fn unspecified_memory_uses_the_default_action_estimate() {
    let limits = ResourceLimits::new(4, 100).with_default_action_memory_bytes(60);
    let pool = pool(limits);
    let first = pool.acquire(ResourceRequest::default()).await;
    pending(pool.acquire(ResourceRequest::default())).await;
    drop(first);
    ready(pool.acquire(ResourceRequest::default())).await;
}

#[tokio::test]
async fn oversized_unspecified_estimate_is_clamped_without_deadlock() {
    let limits = ResourceLimits::new(2, 10).with_default_action_memory_bytes(100);
    ready(pool(limits).acquire(ResourceRequest::default())).await;
}

#[tokio::test]
async fn oversized_request_is_clamped_to_pool_limits() {
    let pool = pool(ResourceLimits::new(2, 100));
    let _permit = ready(pool.acquire(ResourceRequest::new(8, 1_000))).await;
}

#[tokio::test]
async fn named_slot_blocks_when_pool_is_saturated() {
    let pool = pool(ResourceLimits::new(8, 0).with_slot_limit("exclusive_slot", 1));
    let request = ResourceRequest::default().with_slot("exclusive_slot", 1);
    let first = pool.acquire(request.clone()).await;
    pending(pool.acquire(request.clone())).await;
    drop(first);
    ready(pool.acquire(request)).await;
}

#[tokio::test]
async fn unrelated_actions_dont_compete_for_named_slots() {
    let pool = pool(ResourceLimits::new(8, 0).with_slot_limit("exclusive_slot", 1));
    let _named = pool
        .acquire(ResourceRequest::default().with_slot("exclusive_slot", 1))
        .await;
    ready(pool.acquire(ResourceRequest::default())).await;
}

#[tokio::test]
async fn a_flood_of_small_requests_cannot_starve_an_older_large_one() {
    let pool = pool(ResourceLimits::new(4, 0));
    let mut running = Vec::new();
    for _ in 0..4 {
        running.push(pool.acquire(ResourceRequest::new(1, 0)).await);
    }
    let large_pool = Arc::clone(&pool);
    let large = tokio::spawn(async move { large_pool.acquire(ResourceRequest::new(4, 0)).await });
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Small requests keep arriving after the large one queued. Each one that
    // got in would take a CPU the large request is waiting for.
    let mut small = Vec::new();
    for _ in 0..16 {
        let small_pool = Arc::clone(&pool);
        small.push(tokio::spawn(async move {
            small_pool.acquire(ResourceRequest::new(1, 0)).await
        }));
    }
    for permit in running.drain(..) {
        drop(permit);
        tokio::task::yield_now().await;
    }

    let large = ready(large).await.unwrap();
    assert!(small.iter().all(|task| !task.is_finished()));
    drop(large);
    for task in small {
        drop(ready(task).await.unwrap());
    }
    assert!(pool.is_idle());
}

#[tokio::test]
async fn work_on_another_axis_runs_beside_a_blocked_large_request() {
    let pool = pool(ResourceLimits::new(2, 1_000));
    let _running = pool.acquire(ResourceRequest::new(1, 100)).await;
    let large_pool = Arc::clone(&pool);
    let large = tokio::spawn(async move { large_pool.acquire(ResourceRequest::new(2, 100)).await });
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Client and restore work reserves memory but no CPU.
    let _client = ready(pool.acquire_client(100, BTreeMap::new())).await;
    assert!(!large.is_finished());
    large.abort();
}

#[tokio::test]
async fn cancelling_the_queue_head_lets_the_next_request_in() {
    let pool = pool(ResourceLimits::new(2, 0));
    let _running = pool.acquire(ResourceRequest::new(1, 0)).await;
    let head_pool = Arc::clone(&pool);
    let head = tokio::spawn(async move { head_pool.acquire(ResourceRequest::new(2, 0)).await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    pending(pool.acquire(ResourceRequest::new(1, 0))).await;

    head.abort();
    assert!(head.await.unwrap_err().is_cancelled());
    ready(pool.acquire(ResourceRequest::new(1, 0))).await;
}

#[tokio::test]
async fn a_grant_that_races_with_dropping_the_waiter_is_returned() {
    let pool = pool(ResourceLimits::new(1, 0));
    let running = pool.acquire(ResourceRequest::default()).await;
    {
        let mut waiting = pin!(pool.acquire(ResourceRequest::default()));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(waiting.as_mut().poll(&mut cx).is_pending());
        // The release grants the queued request before it is polled again.
        drop(running);
        assert_eq!(pool.used(), (1, super::super::DEFAULT_ACTION_MEMORY_BYTES));
    }
    assert!(pool.is_idle());
    ready(pool.acquire(ResourceRequest::default())).await;
}

#[tokio::test]
async fn a_request_that_is_never_polled_takes_nothing() {
    let pool = pool(ResourceLimits::new(1, 0));
    drop(pool.acquire(ResourceRequest::default()));
    assert!(pool.is_idle());
}

#[tokio::test]
async fn required_requests_fail_instead_of_clamping() {
    let pool = pool(ResourceLimits::new(2, 100));
    assert!(matches!(
        pool.acquire_required(ResourceRequest::new(3, 10)).await,
        Err(Error::ResourceRequirementUnsatisfied { .. })
    ));
    assert!(matches!(
        pool.acquire_required(ResourceRequest::new(1, 101)).await,
        Err(Error::ResourceRequirementUnsatisfied { .. })
    ));
    assert!(pool.is_idle());

    let exact = ready(pool.acquire_required(ResourceRequest::new(2, 100)))
        .await
        .unwrap();
    assert_eq!(exact.request().memory_bytes, 100);
    pending(pool.acquire_required(ResourceRequest::new(1, 1))).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn memory_budget_stress_never_exceeds_the_limit() {
    use std::sync::atomic::{AtomicU64, Ordering};

    const BUDGET: u64 = 4 * 1024 * 1024;
    const ALLOCATION: u64 = 1024 * 1024;

    let pool = pool(ResourceLimits::new(32, BUDGET));
    let live = Arc::new(AtomicU64::new(0));
    let peak = Arc::new(AtomicU64::new(0));
    let mut tasks = Vec::new();
    for _ in 0..64 {
        let pool = Arc::clone(&pool);
        let live = Arc::clone(&live);
        let peak = Arc::clone(&peak);
        tasks.push(tokio::spawn(async move {
            let _permit = pool.acquire(ResourceRequest::new(1, ALLOCATION)).await;
            let bytes = vec![0x5a; usize::try_from(ALLOCATION).unwrap()];
            let current = live.fetch_add(ALLOCATION, Ordering::SeqCst) + ALLOCATION;
            peak.fetch_max(current, Ordering::SeqCst);
            tokio::task::yield_now().await;
            assert_eq!(bytes[0], 0x5a);
            live.fetch_sub(ALLOCATION, Ordering::SeqCst);
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }

    assert!(peak.load(Ordering::SeqCst) <= BUDGET);
    assert_eq!(live.load(Ordering::SeqCst), 0);
    assert!(pool.is_idle());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_at_any_point_leaves_the_pool_idle() {
    let pool = pool(
        ResourceLimits::new(3, 300)
            .with_default_action_memory_bytes(50)
            .with_slot_limit("device", 1),
    );
    let mut tasks = Vec::new();
    for index in 0_u64..200 {
        let pool = Arc::clone(&pool);
        let request = match index % 4 {
            0 => ResourceRequest::new(3, 0),
            1 => ResourceRequest::new(1, 120),
            2 => ResourceRequest::new(1, 0).with_slot("device", 1),
            _ => ResourceRequest::new(2, 60),
        };
        let deadline = Duration::from_micros(50 * (index % 7));
        tasks.push(tokio::spawn(async move {
            if let Ok(permit) = tokio::time::timeout(deadline, pool.acquire(request)).await {
                let (cpu_slots, memory_bytes) = pool.used();
                assert!(cpu_slots <= 3 && memory_bytes <= 300);
                tokio::task::yield_now().await;
                drop(permit);
            }
        }));
    }
    for (index, task) in tasks.into_iter().enumerate() {
        if index % 5 == 0 {
            task.abort();
        }
        let _ = task.await;
    }
    assert!(pool.is_idle());
}

#[tokio::test]
async fn undeclared_requests_never_reserve_zero_memory() {
    use super::super::DEFAULT_ACTION_MEMORY_BYTES;

    // Without a memory axis the reservation still records a real estimate,
    // which bounds the buffers derived from it.
    let unbounded = pool(ResourceLimits::new(2, 0));
    let local = unbounded.acquire(ResourceRequest::default()).await;
    assert_eq!(local.request().memory_bytes, DEFAULT_ACTION_MEMORY_BYTES);
    let client = unbounded.acquire_client(0, BTreeMap::new()).await;
    assert_eq!(client.request().memory_bytes, DEFAULT_ACTION_MEMORY_BYTES);

    // A configured default of zero still admits undeclared work by memory.
    let limits =
        ResourceLimits::new(8, 2 * DEFAULT_ACTION_MEMORY_BYTES).with_default_action_memory_bytes(0);
    assert_eq!(limits.max_parallel_actions(), 2);
    let bounded = pool(limits);
    let _first = bounded.acquire(ResourceRequest::default()).await;
    let _second = bounded.acquire_client(0, BTreeMap::new()).await;
    pending(bounded.acquire(ResourceRequest::default())).await;
}
