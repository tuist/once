//! Resource accounting for local and remote action scheduling.

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRequest {
    pub cpu_slots: usize,
    pub memory_bytes: u64,
}

impl ResourceRequest {
    pub const fn new(cpu_slots: usize, memory_bytes: u64) -> Self {
        Self {
            cpu_slots: if cpu_slots == 0 { 1 } else { cpu_slots },
            memory_bytes,
        }
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    fn bounded_by(self, limits: ResourceLimits) -> Self {
        let cpu_slots = self.cpu_slots.clamp(1, limits.cpu_slots);
        let memory_bytes = if limits.memory_bytes == 0 {
            self.memory_bytes
        } else {
            self.memory_bytes.min(limits.memory_bytes)
        };
        Self {
            cpu_slots,
            memory_bytes,
        }
    }
}

impl Default for ResourceRequest {
    fn default() -> Self {
        Self {
            cpu_slots: 1,
            memory_bytes: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceLimits {
    pub cpu_slots: usize,
    pub memory_bytes: u64,
}

impl ResourceLimits {
    pub const fn new(cpu_slots: usize, memory_bytes: u64) -> Self {
        Self {
            cpu_slots: if cpu_slots == 0 { 1 } else { cpu_slots },
            memory_bytes,
        }
    }
}

impl Default for ResourceLimits {
    fn default() -> Self {
        let cpu_slots = std::thread::available_parallelism()
            .map(NonZeroUsize::get)
            .unwrap_or(8);
        Self::new(cpu_slots, 0)
    }
}

#[derive(Debug, Default)]
struct State {
    cpu_slots: usize,
    memory_bytes: u64,
}

#[derive(Debug)]
pub struct ResourcePool {
    limits: ResourceLimits,
    state: Mutex<State>,
    notify: Notify,
}

impl ResourcePool {
    pub fn new(limits: ResourceLimits) -> Self {
        Self {
            limits: ResourceLimits::new(limits.cpu_slots, limits.memory_bytes),
            state: Mutex::new(State::default()),
            notify: Notify::new(),
        }
    }

    pub fn limits(&self) -> ResourceLimits {
        self.limits
    }

    pub async fn acquire(self: &Arc<Self>, request: ResourceRequest) -> ResourcePermit {
        let request = request.bounded_by(self.limits);
        loop {
            let notified = self.notify.notified();
            {
                let mut state = self.state.lock().expect("resource pool lock poisoned");
                if can_acquire(&state, request, self.limits) {
                    state.cpu_slots += request.cpu_slots;
                    state.memory_bytes = state.memory_bytes.saturating_add(request.memory_bytes);
                    return ResourcePermit {
                        pool: Arc::clone(self),
                        request,
                    };
                }
            }
            notified.await;
        }
    }
}

fn can_acquire(state: &State, request: ResourceRequest, limits: ResourceLimits) -> bool {
    let cpu_ok = state.cpu_slots <= limits.cpu_slots.saturating_sub(request.cpu_slots);
    let memory_ok = limits.memory_bytes == 0
        || state.memory_bytes <= limits.memory_bytes.saturating_sub(request.memory_bytes);
    cpu_ok && memory_ok
}

#[derive(Debug)]
pub struct ResourcePermit {
    pool: Arc<ResourcePool>,
    request: ResourceRequest,
}

impl Drop for ResourcePermit {
    fn drop(&mut self) {
        {
            let mut state = self.pool.state.lock().expect("resource pool lock poisoned");
            state.cpu_slots = state.cpu_slots.saturating_sub(self.request.cpu_slots);
            state.memory_bytes = state.memory_bytes.saturating_sub(self.request.memory_bytes);
        }
        self.pool.notify.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn cpu_slots_block_until_released() {
        let pool = Arc::new(ResourcePool::new(ResourceLimits::new(1, 0)));
        let first = pool.acquire(ResourceRequest::default()).await;
        let waiting_pool = Arc::clone(&pool);
        let waiting = tokio::spawn(async move {
            let _permit = waiting_pool.acquire(ResourceRequest::default()).await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiting.is_finished());

        drop(first);
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn memory_budget_blocks_until_released() {
        let pool = Arc::new(ResourcePool::new(ResourceLimits::new(4, 100)));
        let first = pool.acquire(ResourceRequest::new(1, 80)).await;
        let waiting_pool = Arc::clone(&pool);
        let waiting = tokio::spawn(async move {
            let _permit = waiting_pool.acquire(ResourceRequest::new(1, 40)).await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiting.is_finished());

        drop(first);
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn oversized_request_is_clamped_to_pool_limits() {
        let pool = Arc::new(ResourcePool::new(ResourceLimits::new(2, 100)));
        let _permit = pool.acquire(ResourceRequest::new(8, 1_000)).await;
    }
}
