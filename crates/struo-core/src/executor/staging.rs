use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{Error, RemoteIoLimits, Result};

const UNIT_BYTES: u64 = 64 * 1024;

/// Disk staged on the host for hosted actions, in two independent budgets.
///
/// Transfer staging covers input and output archives; each phase reserves its
/// size before it starts. Capture covers command output on its way into the
/// cache; each action reserves its whole capture allowance before the sandbox
/// is created. Keeping them apart means a queued archive transfer can never
/// make a running command's capture fail, and capture never has to grow while
/// it holds part of the pool.
#[derive(Debug, Clone)]
pub(crate) struct StagingPool {
    transfer: Units,
    capture: Units,
    capture_per_action: u64,
}

impl StagingPool {
    pub(crate) fn for_limits(limits: &RemoteIoLimits) -> Self {
        Self {
            transfer: Units::new(limits.max_staging_bytes),
            capture: Units::new(limits.max_capture_staging_bytes),
            capture_per_action: limits.max_capture_bytes,
        }
    }

    pub(crate) async fn acquire(&self, bytes: u64) -> Result<StagingPermit> {
        let permit = self.transfer.acquire(bytes, "staging bytes").await?;
        Ok(StagingPermit {
            _permit: Arc::new(permit),
        })
    }

    /// An unreserved capture allowance. Reserve it with
    /// [`CapturePermit::reserve_upfront`] before any paid work starts.
    pub(crate) fn capture(&self) -> CapturePermit {
        CapturePermit {
            pool: self.capture.clone(),
            limit: self.capture_per_action,
            state: Arc::new(Mutex::new(CaptureState::default())),
        }
    }
}

#[derive(Debug, Clone)]
struct Units {
    permits: Arc<Semaphore>,
    capacity: u64,
}

impl Units {
    fn new(bytes: u64) -> Self {
        let units = (bytes / UNIT_BYTES)
            .min(u64::from(u32::MAX))
            .min(Semaphore::MAX_PERMITS as u64);
        Self {
            permits: Arc::new(Semaphore::new(
                usize::try_from(units).expect("unit count fits in usize"),
            )),
            capacity: units * UNIT_BYTES,
        }
    }

    /// Wait for `bytes`, all or nothing. A request the pool can never hold
    /// fails at once instead of waiting forever.
    async fn acquire(&self, bytes: u64, resource: &'static str) -> Result<OwnedSemaphorePermit> {
        let units = bytes.div_ceil(UNIT_BYTES);
        let unsatisfied = || Error::ResourceRequirementUnsatisfied {
            resource: resource.to_string(),
            requested: units.saturating_mul(UNIT_BYTES),
            capacity: self.capacity,
        };
        if units > self.capacity / UNIT_BYTES {
            return Err(unsatisfied());
        }
        let units = u32::try_from(units).map_err(|_| unsatisfied())?;
        Ok(Arc::clone(&self.permits)
            .acquire_many_owned(units)
            .await
            .expect("staging pools are never closed"))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StagingPermit {
    _permit: Arc<OwnedSemaphorePermit>,
}

#[derive(Debug, Default)]
struct CaptureState {
    grant: Option<Arc<OwnedSemaphorePermit>>,
    used: u64,
}

/// One action's command-output allowance. Clones share the reservation,
/// which returns to the pool when the last clone drops.
#[derive(Debug, Clone)]
pub(crate) struct CapturePermit {
    pool: Units,
    limit: u64,
    state: Arc<Mutex<CaptureState>>,
}

impl CapturePermit {
    /// Reserve the action's whole allowance, waiting in arrival order. Call
    /// this before creating the sandbox so waiting never costs a paid machine
    /// and capture never has to acquire more while the command runs.
    pub(crate) async fn reserve_upfront(&self) -> Result<()> {
        if self.lock().grant.is_some() {
            return Ok(());
        }
        let grant = self
            .pool
            .acquire(self.limit, "capture staging bytes")
            .await?;
        let mut state = self.lock();
        if state.grant.is_none() {
            state.grant = Some(Arc::new(grant));
        }
        Ok(())
    }

    /// Account `bytes` of captured output against the reserved allowance.
    /// Exceeding it is a deterministic per-action limit, never contention.
    pub(crate) fn reserve(&self, bytes: u64) -> Result<()> {
        let mut state = self.lock();
        if state.grant.is_none() {
            return Err(Error::ExecutorResourceLimit {
                executor: "hosted".to_string(),
                resource: "unreserved capture bytes",
                limit: 0,
            });
        }
        let used = state.used.saturating_add(bytes);
        if used > self.limit {
            return Err(Error::ExecutorResourceLimit {
                executor: "hosted".to_string(),
                resource: "captured output bytes",
                limit: self.limit,
            });
        }
        state.used = used;
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CaptureState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn pool(transfer: u64, capture: u64, per_action: u64) -> StagingPool {
        StagingPool::for_limits(&RemoteIoLimits {
            max_staging_bytes: transfer,
            max_capture_staging_bytes: capture,
            max_capture_bytes: per_action,
            ..RemoteIoLimits::default()
        })
    }

    async fn pending<F: std::future::Future>(future: F) {
        assert!(tokio::time::timeout(Duration::from_millis(20), future)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn phase_leases_share_capacity_and_survive_cancellation() {
        let pool = pool(2 * UNIT_BYTES, UNIT_BYTES, UNIT_BYTES);
        let first = pool.acquire(UNIT_BYTES + 1).await.unwrap();
        let detached = first.clone();
        drop(first);
        pending(pool.acquire(1)).await;
        drop(detached);
        pool.acquire(2 * UNIT_BYTES).await.unwrap();
    }

    #[tokio::test]
    async fn oversized_phase_fails_instead_of_waiting_forever() {
        let pool = pool(UNIT_BYTES, UNIT_BYTES, UNIT_BYTES);
        assert!(matches!(
            pool.acquire(UNIT_BYTES + 1).await,
            Err(Error::ResourceRequirementUnsatisfied { .. })
        ));
    }

    #[tokio::test]
    async fn running_capture_progresses_while_transfers_are_queued() {
        let pool = pool(2 * UNIT_BYTES, 2 * UNIT_BYTES, UNIT_BYTES);
        let capture = pool.capture();
        capture.reserve_upfront().await.unwrap();
        // Transfer staging is exhausted and a large phase is queued behind
        // it, which used to absorb every unit a capture tried to grow into.
        let held = pool.acquire(2 * UNIT_BYTES).await.unwrap();
        let queued_pool = pool.clone();
        let queued = tokio::spawn(async move { queued_pool.acquire(2 * UNIT_BYTES).await });
        tokio::time::sleep(Duration::from_millis(10)).await;

        for _ in 0..64 {
            capture.reserve(UNIT_BYTES / 64).unwrap();
        }
        assert!(!queued.is_finished());
        drop(held);
        queued.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn concurrent_captures_each_progress_within_their_allowance() {
        let pool = pool(UNIT_BYTES, 4 * UNIT_BYTES, UNIT_BYTES);
        let mut tasks = Vec::new();
        for _ in 0..4 {
            let capture = pool.capture();
            tasks.push(tokio::spawn(async move {
                capture.reserve_upfront().await.unwrap();
                for _ in 0..16 {
                    capture.reserve(UNIT_BYTES / 16).unwrap();
                    tokio::task::yield_now().await;
                }
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_full_capture_pool_queues_new_actions_before_they_start() {
        let pool = pool(UNIT_BYTES, UNIT_BYTES, UNIT_BYTES);
        let running = pool.capture();
        running.reserve_upfront().await.unwrap();
        let next = pool.capture();
        pending(next.reserve_upfront()).await;
        drop(running);
        tokio::time::timeout(Duration::from_secs(1), next.reserve_upfront())
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn capture_beyond_the_allowance_is_a_deterministic_limit() {
        let pool = pool(UNIT_BYTES, 4 * UNIT_BYTES, UNIT_BYTES);
        let capture = pool.capture();
        assert!(matches!(
            capture.reserve(1),
            Err(Error::ExecutorResourceLimit {
                resource: "unreserved capture bytes",
                ..
            })
        ));
        capture.reserve_upfront().await.unwrap();
        capture.reserve(UNIT_BYTES).unwrap();
        assert!(matches!(
            capture.reserve(1),
            Err(Error::ExecutorResourceLimit {
                resource: "captured output bytes",
                limit,
                ..
            }) if limit == UNIT_BYTES
        ));
    }

    #[tokio::test]
    async fn an_allowance_larger_than_the_pool_fails_before_any_work() {
        let pool = pool(UNIT_BYTES, UNIT_BYTES, 2 * UNIT_BYTES);
        assert!(matches!(
            pool.capture().reserve_upfront().await,
            Err(Error::ResourceRequirementUnsatisfied { .. })
        ));
    }

    #[tokio::test]
    async fn the_allowance_returns_when_the_last_clone_drops() {
        let pool = pool(UNIT_BYTES, UNIT_BYTES, UNIT_BYTES);
        let capture = pool.capture();
        capture.reserve_upfront().await.unwrap();
        let clone = capture.clone();
        clone.reserve_upfront().await.unwrap();
        drop(capture);
        pending(pool.capture().reserve_upfront()).await;
        drop(clone);
        tokio::time::timeout(Duration::from_secs(1), pool.capture().reserve_upfront())
            .await
            .unwrap()
            .unwrap();
    }
}
