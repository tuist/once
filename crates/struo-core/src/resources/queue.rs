use std::collections::{BTreeMap, VecDeque};
use std::task::{Poll, Waker};

use super::{ResourceLimits, ResourceRequest};

/// Capacity currently granted, across every axis the pool admits.
#[derive(Debug, Default)]
struct Usage {
    cpu_slots: usize,
    memory_bytes: u64,
    /// Entries are created on first use and kept to avoid churn under load.
    slots: BTreeMap<String, usize>,
}

impl Usage {
    fn add(&mut self, request: &ResourceRequest) {
        self.cpu_slots += request.cpu_slots;
        self.memory_bytes = self.memory_bytes.saturating_add(request.memory_bytes);
        for (name, count) in &request.slots {
            *self.slots.entry(name.clone()).or_insert(0) += count;
        }
    }

    fn remove(&mut self, request: &ResourceRequest) {
        self.cpu_slots = self.cpu_slots.saturating_sub(request.cpu_slots);
        self.memory_bytes = self.memory_bytes.saturating_sub(request.memory_bytes);
        for (name, count) in &request.slots {
            if let Some(used) = self.slots.get_mut(name) {
                *used = used.saturating_sub(*count);
            }
        }
    }
}

#[derive(Debug)]
struct Waiter {
    id: u64,
    request: ResourceRequest,
    granted: bool,
    waker: Option<Waker>,
}

/// FIFO admission with reservations.
///
/// Waiters are considered in arrival order. A waiter that does not fit
/// reserves what it needs from the capacity that is free, so a later
/// waiter can only use capacity no earlier waiter is waiting for. A small
/// request can still start beside a blocked large one when they need
/// different axes, but it can never delay an earlier request. Grants are
/// recorded here before the waiter observes them, so a waiter dropped
/// between grant and observation hands its capacity back.
#[derive(Debug, Default)]
pub(super) struct Queue {
    used: Usage,
    waiters: VecDeque<Waiter>,
    next_id: u64,
}

impl Queue {
    pub(super) fn enqueue(&mut self, request: ResourceRequest) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.waiters.push_back(Waiter {
            id,
            request,
            granted: false,
            waker: None,
        });
        id
    }

    /// `Ready` once the waiter was granted, removing it from the queue.
    pub(super) fn poll(&mut self, id: u64, waker: &Waker) -> Poll<()> {
        let Some(index) = self.position(id) else {
            unreachable!("resource waiter {id} polled after it left the queue");
        };
        if self.waiters[index].granted {
            self.waiters.remove(index);
            return Poll::Ready(());
        }
        let waiter = &mut self.waiters[index];
        if !waiter
            .waker
            .as_ref()
            .is_some_and(|known| known.will_wake(waker))
        {
            waiter.waker = Some(waker.clone());
        }
        Poll::Pending
    }

    /// Abandon a waiter. Capacity it was granted but never observed
    /// returns to the pool. Returns whether anything changed.
    pub(super) fn cancel(&mut self, id: u64) -> bool {
        let Some(index) = self.position(id) else {
            return false;
        };
        let waiter = self.waiters.remove(index).expect("waiter index is valid");
        if waiter.granted {
            self.used.remove(&waiter.request);
        }
        true
    }

    pub(super) fn release(&mut self, request: &ResourceRequest) {
        self.used.remove(request);
    }

    /// Grant every waiter that fits without taking capacity an earlier
    /// waiter is waiting for. Returns the wakers to call after unlocking.
    pub(super) fn dispatch(&mut self, limits: &ResourceLimits) -> Vec<Waker> {
        let mut free = Free::new(limits, &self.used);
        let mut granted = Vec::new();
        let mut wakers = Vec::new();
        for waiter in &mut self.waiters {
            if waiter.granted {
                continue;
            }
            let fits = free.fits(&waiter.request);
            // Either way the request's share leaves the free capacity:
            // granted, or reserved for this waiter ahead of later ones.
            free.take(&waiter.request);
            if fits {
                waiter.granted = true;
                granted.push(&waiter.request);
                wakers.extend(waiter.waker.take());
            }
        }
        for request in granted {
            self.used.add(request);
        }
        wakers
    }

    pub(super) fn waiting(&self) -> usize {
        self.waiters.iter().filter(|waiter| !waiter.granted).count()
    }

    fn position(&self, id: u64) -> Option<usize> {
        self.waiters.iter().position(|waiter| waiter.id == id)
    }

    #[cfg(test)]
    pub(super) fn is_idle(&self) -> bool {
        self.waiters.is_empty()
            && self.used.cpu_slots == 0
            && self.used.memory_bytes == 0
            && self.used.slots.values().all(|count| *count == 0)
    }

    #[cfg(test)]
    pub(super) fn used(&self) -> (usize, u64) {
        (self.used.cpu_slots, self.used.memory_bytes)
    }
}

/// Capacity not yet granted or reserved during one dispatch pass.
struct Free<'a> {
    limits: &'a ResourceLimits,
    used: &'a Usage,
    cpu_slots: usize,
    memory_bytes: Option<u64>,
    slots: BTreeMap<String, usize>,
}

impl<'a> Free<'a> {
    fn new(limits: &'a ResourceLimits, used: &'a Usage) -> Self {
        Self {
            limits,
            used,
            cpu_slots: limits.cpu_slots.saturating_sub(used.cpu_slots),
            memory_bytes: (limits.memory_bytes != 0)
                .then(|| limits.memory_bytes.saturating_sub(used.memory_bytes)),
            slots: BTreeMap::new(),
        }
    }

    fn slot(&mut self, name: &str) -> &mut usize {
        if !self.slots.contains_key(name) {
            let used = self.used.slots.get(name).copied().unwrap_or(0);
            let free = self.limits.slot_capacity(name).saturating_sub(used);
            self.slots.insert(name.to_string(), free);
        }
        self.slots
            .get_mut(name)
            .expect("slot entry was just inserted")
    }

    fn fits(&mut self, request: &ResourceRequest) -> bool {
        request.cpu_slots <= self.cpu_slots
            && self
                .memory_bytes
                .is_none_or(|free| request.memory_bytes <= free)
            && request
                .slots
                .iter()
                .all(|(name, count)| *count <= *self.slot(name))
    }

    fn take(&mut self, request: &ResourceRequest) {
        self.cpu_slots = self.cpu_slots.saturating_sub(request.cpu_slots);
        if let Some(free) = &mut self.memory_bytes {
            *free = free.saturating_sub(request.memory_bytes);
        }
        for (name, count) in &request.slots {
            let free = self.slot(name);
            *free = free.saturating_sub(*count);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::task::Waker;

    use super::*;

    fn request(cpu_slots: usize, memory_bytes: u64) -> ResourceRequest {
        ResourceRequest {
            cpu_slots,
            memory_bytes,
            slots: BTreeMap::new(),
        }
    }

    fn granted(queue: &mut Queue, id: u64) -> bool {
        queue.poll(id, Waker::noop()).is_ready()
    }

    fn admit(queue: &mut Queue, limits: &ResourceLimits, request: ResourceRequest) -> u64 {
        let id = queue.enqueue(request);
        queue.dispatch(limits);
        id
    }

    #[test]
    fn grants_follow_arrival_order() {
        let limits = ResourceLimits::new(2, 0);
        let mut queue = Queue::default();
        let first = admit(&mut queue, &limits, request(2, 0));
        assert!(granted(&mut queue, first));
        let large = admit(&mut queue, &limits, request(2, 0));
        let small = admit(&mut queue, &limits, request(1, 0));
        assert!(!granted(&mut queue, large));
        assert!(!granted(&mut queue, small));

        queue.release(&request(2, 0));
        queue.dispatch(&limits);
        assert!(granted(&mut queue, large));
        assert!(
            !granted(&mut queue, small),
            "a later request must not take the head's CPUs"
        );
    }

    #[test]
    fn later_requests_cannot_take_capacity_an_earlier_waiter_needs() {
        let limits = ResourceLimits::new(4, 100);
        let mut queue = Queue::default();
        let running = admit(&mut queue, &limits, request(2, 10));
        assert!(granted(&mut queue, running));
        // The head waits for CPUs and will need 50 bytes of memory.
        let head = admit(&mut queue, &limits, request(4, 50));
        // Needs a CPU the head is waiting for. Being earlier, it would also
        // hold back any memory it asked for.
        let cpu = admit(&mut queue, &limits, request(1, 0));
        // Needs no CPU, and memory beyond what the head will need.
        let memory_only = admit(&mut queue, &limits, request(0, 40));
        // Needs no CPU, but memory the head will need.
        let too_much_memory = admit(&mut queue, &limits, request(0, 41));

        assert!(!granted(&mut queue, head));
        assert!(!granted(&mut queue, cpu));
        assert!(granted(&mut queue, memory_only));
        assert!(!granted(&mut queue, too_much_memory));

        queue.release(&request(2, 10));
        queue.dispatch(&limits);
        assert!(granted(&mut queue, head));
    }

    #[test]
    fn named_slots_queue_independently() {
        let limits = ResourceLimits::new(8, 0)
            .with_slot_limit("device", 1)
            .with_slot_limit("license", 1);
        let mut queue = Queue::default();
        let device = request(1, 0).with_slot("device", 1);
        let license = request(1, 0).with_slot("license", 1);
        let holder = admit(&mut queue, &limits, device.clone());
        assert!(granted(&mut queue, holder));
        let waiting_device = admit(&mut queue, &limits, device.clone());
        let other_device = admit(&mut queue, &limits, device.clone());
        let other_slot = admit(&mut queue, &limits, license);

        assert!(!granted(&mut queue, waiting_device));
        assert!(!granted(&mut queue, other_device));
        assert!(granted(&mut queue, other_slot));

        queue.release(&device);
        queue.dispatch(&limits);
        assert!(granted(&mut queue, waiting_device));
        assert!(!granted(&mut queue, other_device));
    }

    #[test]
    fn cancelling_a_blocked_head_unblocks_later_waiters() {
        let limits = ResourceLimits::new(2, 0);
        let mut queue = Queue::default();
        let running = admit(&mut queue, &limits, request(1, 0));
        assert!(granted(&mut queue, running));
        let head = admit(&mut queue, &limits, request(2, 0));
        let next = admit(&mut queue, &limits, request(1, 0));
        assert!(!granted(&mut queue, next));

        assert!(queue.cancel(head));
        queue.dispatch(&limits);
        assert!(granted(&mut queue, next));
    }

    #[test]
    fn a_grant_nobody_observed_returns_its_capacity() {
        let limits = ResourceLimits::new(1, 0);
        let mut queue = Queue::default();
        let running = admit(&mut queue, &limits, request(1, 0));
        assert!(granted(&mut queue, running));
        let waiter = admit(&mut queue, &limits, request(1, 0));
        assert!(!granted(&mut queue, waiter));

        queue.release(&request(1, 0));
        queue.dispatch(&limits);
        // Granted, then abandoned before it was polled again.
        assert!(queue.cancel(waiter));
        queue.dispatch(&limits);
        assert!(queue.is_idle());
    }

    #[test]
    fn cancelling_after_observation_changes_nothing() {
        let limits = ResourceLimits::new(1, 0);
        let mut queue = Queue::default();
        let id = admit(&mut queue, &limits, request(1, 0));
        assert!(granted(&mut queue, id));
        assert!(!queue.cancel(id));
        assert_eq!(queue.used(), (1, 0));
    }

    #[test]
    fn a_disabled_memory_axis_never_blocks() {
        let limits = ResourceLimits::new(1, 0);
        let mut queue = Queue::default();
        let id = admit(&mut queue, &limits, request(1, u64::MAX));
        assert!(granted(&mut queue, id));
    }

    #[test]
    fn grants_wake_only_their_own_waiters() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use std::task::Wake;

        struct Count(AtomicUsize);
        impl Wake for Count {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let limits = ResourceLimits::new(1, 0);
        let mut queue = Queue::default();
        let running = admit(&mut queue, &limits, request(1, 0));
        assert!(granted(&mut queue, running));
        let counts: Vec<_> = (0..3)
            .map(|_| Arc::new(Count(AtomicUsize::new(0))))
            .collect();
        let ids: Vec<_> = counts
            .iter()
            .map(|count| {
                let id = queue.enqueue(request(1, 0));
                assert!(queue.poll(id, &Waker::from(Arc::clone(count))).is_pending());
                id
            })
            .collect();

        queue.release(&request(1, 0));
        for waker in queue.dispatch(&limits) {
            waker.wake();
        }
        let woken: Vec<_> = counts
            .iter()
            .map(|count| count.0.load(Ordering::SeqCst))
            .collect();
        assert_eq!(woken, vec![1, 0, 0]);
        assert!(granted(&mut queue, ids[0]));
        assert_eq!(queue.waiting(), 2);
    }
}
