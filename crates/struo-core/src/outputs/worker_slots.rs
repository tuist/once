use std::sync::Arc;

use crate::ResourcePermit;

pub(super) struct WorkerSlots {
    owners: Vec<Option<Arc<ResourcePermit>>>,
    available: Vec<usize>,
}

pub(super) struct WorkerSlot {
    pub(super) index: usize,
    pub(super) owner: Option<Arc<ResourcePermit>>,
}

impl WorkerSlots {
    pub(super) fn new(limit: usize) -> Self {
        let mut owners = Vec::new();
        if limit > 0 {
            if let Some(base) = crate::resource_scope::current() {
                owners.push(Some(Arc::clone(&base)));
                while owners.len() < limit {
                    let Some(extra) = base.try_additional_default() else {
                        break;
                    };
                    owners.push(Some(Arc::new(extra)));
                }
            } else {
                owners.resize_with(limit, || None);
            }
        }
        let available = (0..owners.len()).rev().collect();
        Self { owners, available }
    }

    pub(super) fn take(&mut self) -> Option<WorkerSlot> {
        let index = self.available.pop()?;
        Some(WorkerSlot {
            index,
            owner: self.owners[index].clone(),
        })
    }

    pub(super) fn release(&mut self, index: usize) {
        self.available.push(index);
    }

    pub(super) fn retire(&mut self, index: usize) {
        self.owners[index] = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{with_resource_permit, ResourceLimits, ResourcePool, ResourceRequest};
    use futures::FutureExt;

    #[tokio::test]
    async fn occupied_pool_never_waits_for_additional_validation_workers() {
        let pool = Arc::new(ResourcePool::new(ResourceLimits::new(
            2,
            1024 * 1024 * 1024,
        )));
        let held = pool.acquire(ResourceRequest::default()).await;
        let base = pool.acquire(ResourceRequest::default()).await;
        with_resource_permit(base, async {
            let mut slots = WorkerSlots::new(16);
            let slot = slots.take().unwrap();
            assert!(slot.owner.is_some());
            assert!(slots.take().is_none());
            slots.release(slot.index);
            assert!(slots.take().is_some());
        })
        .await;
        drop(held);
        assert!(pool
            .acquire(ResourceRequest::new(2, 1024 * 1024 * 1024))
            .now_or_never()
            .is_some());
    }

    #[tokio::test]
    async fn extra_workers_hold_only_admitted_capacity_after_coordinator_drop() {
        let pool = Arc::new(ResourcePool::new(ResourceLimits::new(
            3,
            1024 * 1024 * 1024,
        )));
        let base = pool.acquire(ResourceRequest::default()).await;
        let extra = with_resource_permit(base, async {
            let mut slots = WorkerSlots::new(16);
            let base_slot = slots.take().unwrap();
            let extra = slots.take().unwrap();
            assert!(slots.take().is_some());
            assert!(slots.take().is_none());
            drop(slots);
            drop(base_slot);
            extra
        })
        .await;
        assert!(pool
            .acquire(ResourceRequest::new(3, 1024 * 1024 * 1024))
            .now_or_never()
            .is_none());
        drop(extra);
        assert!(pool
            .acquire(ResourceRequest::new(3, 1024 * 1024 * 1024))
            .now_or_never()
            .is_some());
    }
}
