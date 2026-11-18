use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Poll, Waker};
use std::time::Instant;

use tracing::debug;

use super::queue::Queue;
use super::{ResourceLimits, ResourceRequest};
use crate::Result;

/// Admission control over a [`ResourceLimits`] budget.
///
/// Requests are admitted in arrival order. A request that cannot start yet
/// keeps its claim on the capacity it needs, so a stream of small requests
/// can never starve an older large one; later requests only start beside it
/// when they need capacity it is not waiting for.
#[derive(Debug)]
pub struct ResourcePool {
    limits: ResourceLimits,
    queue: Mutex<Queue>,
}

impl ResourcePool {
    /// Build an empty pool over `limits`.
    pub fn new(limits: ResourceLimits) -> Self {
        let limits = ResourceLimits {
            cpu_slots: limits.cpu_slots.max(1),
            ..limits
        };
        Self {
            limits,
            queue: Mutex::new(Queue::default()),
        }
    }

    /// See [`ResourceLimits::max_parallel_actions`].
    pub fn max_parallel_actions(&self) -> usize {
        self.limits.max_parallel_actions()
    }

    /// Copy of the budget this pool admits against.
    pub fn limits(&self) -> ResourceLimits {
        self.limits.clone()
    }

    /// Wait for `request`, treated as an estimate: dimensions larger than
    /// the pool are clamped so the request can always eventually run alone.
    pub async fn acquire(self: &Arc<Self>, request: ResourceRequest) -> ResourcePermit {
        self.admit(request.bounded_by(&self.limits)).await
    }

    /// Wait for `request`, treated as a minimum. A dimension the pool can
    /// never provide fails immediately instead of being clamped.
    pub async fn acquire_required(
        self: &Arc<Self>,
        request: ResourceRequest,
    ) -> Result<ResourcePermit> {
        let request = request.required_within(&self.limits)?;
        Ok(self.admit(request).await)
    }

    pub(crate) async fn acquire_client(
        self: &Arc<Self>,
        memory_bytes: u64,
        slots: BTreeMap<String, usize>,
    ) -> ResourcePermit {
        let mut request = ResourceRequest {
            cpu_slots: 0,
            memory_bytes,
            slots,
        }
        .bounded_by(&self.limits);
        // Client requests reserve buffers and host-wide slots, not guest CPUs.
        request.cpu_slots = 0;
        self.admit(request).await
    }

    async fn admit(self: &Arc<Self>, request: ResourceRequest) -> ResourcePermit {
        let mut ticket = Ticket {
            pool: self,
            id: None,
            queued_at: Instant::now(),
        };
        let wakers = {
            let mut queue = self.lock();
            let id = queue.enqueue(request.clone());
            ticket.id = Some(id);
            let wakers = queue.dispatch(&self.limits);
            debug!(
                request_cpu_slots = request.cpu_slots,
                request_memory_bytes = request.memory_bytes,
                request_slots = ?request.slots,
                waiting = queue.waiting(),
                "resource request queued"
            );
            wakers
        };
        wake(wakers);
        std::future::poll_fn(|cx| ticket.poll(cx.waker())).await;
        ticket.id = None;
        debug!(
            request_cpu_slots = request.cpu_slots,
            request_memory_bytes = request.memory_bytes,
            request_slots = ?request.slots,
            waited_ms = u64::try_from(ticket.queued_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            "resource request acquired"
        );
        ResourcePermit {
            reservation: Arc::new(Reservation {
                pool: Arc::clone(self),
                request,
            }),
            owners: Vec::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Queue> {
        // Accounting stays consistent under a panic elsewhere, and a permit
        // released while unwinding must not panic again.
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn release(&self, request: &ResourceRequest) {
        let wakers = {
            let mut queue = self.lock();
            queue.release(request);
            queue.dispatch(&self.limits)
        };
        wake(wakers);
    }

    #[cfg(test)]
    pub(super) fn is_idle(&self) -> bool {
        self.lock().is_idle()
    }

    #[cfg(test)]
    pub(super) fn used(&self) -> (usize, u64) {
        self.lock().used()
    }
}

fn wake(wakers: Vec<Waker>) {
    for waker in wakers {
        waker.wake();
    }
}

/// A queued request. Dropping it before it is admitted withdraws the
/// request, and returns capacity granted after the caller stopped waiting.
struct Ticket<'a> {
    pool: &'a ResourcePool,
    id: Option<u64>,
    queued_at: Instant,
}

impl Ticket<'_> {
    fn poll(&mut self, waker: &Waker) -> Poll<()> {
        let id = self.id.expect("a polled ticket is queued");
        self.pool.lock().poll(id, waker)
    }
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        let Some(id) = self.id else {
            return;
        };
        let wakers = {
            let mut queue = self.pool.lock();
            if !queue.cancel(id) {
                return;
            }
            queue.dispatch(&self.pool.limits)
        };
        debug!(
            waited_ms = u64::try_from(self.queued_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            "resource request withdrawn"
        );
        wake(wakers);
    }
}

/// Proof that a request was admitted.
///
/// Releasing is tied to `Drop`, so a panicking or cancelled action cannot
/// leak its reservation and starve the pool.
#[derive(Clone)]
pub struct ResourcePermit {
    reservation: Arc<Reservation>,
    owners: Vec<Arc<dyn Send + Sync>>,
}

impl std::fmt::Debug for ResourcePermit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResourcePermit")
            .field("request", self.request())
            .finish_non_exhaustive()
    }
}

impl ResourcePermit {
    pub(crate) fn request(&self) -> &ResourceRequest {
        &self.reservation.request
    }

    pub(crate) fn with_lifetime_owner(mut self, owner: impl Send + Sync + 'static) -> Self {
        self.owners.push(Arc::new(owner));
        self
    }

    pub(crate) fn try_additional_default(&self) -> Option<Self> {
        use futures::FutureExt;
        let mut extra = self
            .reservation
            .pool
            .acquire(ResourceRequest::default())
            .now_or_never()?;
        extra.owners.clone_from(&self.owners);
        Some(extra)
    }
}

#[derive(Debug)]
struct Reservation {
    pool: Arc<ResourcePool>,
    request: ResourceRequest,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.pool.release(&self.request);
        debug!(request_cpu_slots = self.request.cpu_slots, request_memory_bytes = self.request.memory_bytes,
            request_slots = ?self.request.slots, "resource request released");
    }
}

#[cfg(test)]
mod tests;
