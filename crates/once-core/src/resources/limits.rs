use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use sysinfo::System;

use super::DEFAULT_ACTION_MEMORY_BYTES;

/// The budget every action is admitted against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceLimits {
    /// Total CPU slots available, normally the host's core count.
    pub cpu_slots: usize,
    /// Total memory the pool may hand out. Zero disables the memory axis.
    pub memory_bytes: u64,
    /// Reservation charged to an action that declares no estimate. Zero
    /// means the generic default; an undeclared action never reserves nothing.
    pub default_action_memory_bytes: u64,
    /// Per-name pool size for shared slots. A name absent from this
    /// map is treated as if its limit were 1, so even un-configured
    /// slots still serialize correctly rather than allowing unbounded
    /// concurrency.
    pub slot_limits: BTreeMap<String, usize>,
}

impl ResourceLimits {
    /// Budget of `cpu_slots` and `memory_bytes`, with the per-action
    /// default clamped so a single action can never exceed the total.
    pub fn new(cpu_slots: usize, memory_bytes: u64) -> Self {
        Self {
            cpu_slots: if cpu_slots == 0 { 1 } else { cpu_slots },
            memory_bytes,
            default_action_memory_bytes: if memory_bytes == 0 {
                0
            } else {
                DEFAULT_ACTION_MEMORY_BYTES.min(memory_bytes)
            },
            slot_limits: BTreeMap::new(),
        }
    }

    /// Replace the memory budget, re-clamping the per-action default.
    #[must_use]
    pub fn with_memory_bytes(mut self, memory_bytes: u64) -> Self {
        self.memory_bytes = memory_bytes;
        self.default_action_memory_bytes = if memory_bytes == 0 {
            0
        } else {
            DEFAULT_ACTION_MEMORY_BYTES.min(memory_bytes)
        };
        self
    }

    /// Set the reservation charged to actions that declare no estimate,
    /// capped by the total memory budget.
    #[must_use]
    pub fn with_default_action_memory_bytes(mut self, memory_bytes: u64) -> Self {
        self.default_action_memory_bytes = if self.memory_bytes == 0 {
            memory_bytes
        } else {
            memory_bytes.min(self.memory_bytes)
        };
        self
    }

    /// Publish a pool size for a named slot. Plugins call this when
    /// constructing the Runner so the scheduler knows how many of
    /// that slot's actions can run concurrently. A `count` of 0 is
    /// rounded up to 1 to keep the bounded-pool invariant.
    #[must_use]
    pub fn with_slot_limit(mut self, name: impl Into<String>, count: usize) -> Self {
        self.slot_limits
            .insert(name.into(), if count == 0 { 1 } else { count });
        self
    }

    /// Pool size for a named slot. Returns the configured value, or
    /// `default_value` when the slot isn't published.
    pub fn slot_limit(&self, name: &str, default_value: usize) -> usize {
        self.slot_limits.get(name).copied().unwrap_or(default_value)
    }

    /// Capacity admission uses for a named slot. Unpublished slots have a
    /// pool of one so unknown names never permit unbounded concurrency.
    pub(super) fn slot_capacity(&self, name: &str) -> usize {
        self.slot_limit(name, 1).max(1)
    }

    /// Memory reserved for an action that declares no estimate. Never zero,
    /// so an undeclared action cannot bypass memory admission or the buffer
    /// bounds derived from its reservation. A zero configured default falls
    /// back to the generic estimate, capped by a memory budget when one is set.
    pub(crate) fn undeclared_memory_bytes(&self) -> u64 {
        let configured = match self.default_action_memory_bytes {
            0 => DEFAULT_ACTION_MEMORY_BYTES,
            configured => configured,
        };
        match self.memory_bytes {
            0 => configured,
            budget => configured.min(budget),
        }
    }

    /// How many default-sized actions fit at once, taking the tighter of
    /// the CPU and memory axes. Always at least one, so a budget smaller
    /// than a single action still makes progress rather than deadlocking.
    pub fn max_parallel_actions(&self) -> usize {
        if self.memory_bytes == 0 {
            return self.cpu_slots;
        }
        let memory_slots = self.memory_bytes / self.undeclared_memory_bytes();
        self.cpu_slots
            .min(usize::try_from(memory_slots).unwrap_or(usize::MAX))
            .max(1)
    }
}

impl Default for ResourceLimits {
    fn default() -> Self {
        let cpu_slots = std::thread::available_parallelism().map_or(8, NonZeroUsize::get);
        let memory_bytes = detected_memory_bytes()
            .unwrap_or(1024 * 1024 * 1024)
            .saturating_mul(2)
            / 3;
        Self::new(cpu_slots, memory_bytes)
    }
}

fn detected_memory_bytes() -> Option<u64> {
    let mut system = System::new();
    system.refresh_memory();
    let host = system.total_memory();
    #[cfg(target_os = "linux")]
    let constrained = {
        let pid = sysinfo::Pid::from_u32(std::process::id());
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[pid]),
            true,
            sysinfo::ProcessRefreshKind::nothing().without_tasks(),
        );
        system
            .process(pid)
            .and_then(sysinfo::Process::cgroup_limits)
    };
    #[cfg(not(target_os = "linux"))]
    let constrained = system.cgroup_limits();
    let constrained = constrained
        .map(|limits| limits.total_memory)
        .filter(|memory| *memory > 0);
    [Some(host).filter(|memory| *memory > 0), constrained]
        .into_iter()
        .flatten()
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limits_reserve_memory_for_the_host() {
        let limits = ResourceLimits::default();

        assert!(limits.memory_bytes > 0);
        assert!(limits.default_action_memory_bytes > 0);
        assert!(limits.default_action_memory_bytes <= limits.memory_bytes);
    }

    #[test]
    fn an_undeclared_action_always_reserves_memory() {
        let budget = 4 * DEFAULT_ACTION_MEMORY_BYTES;
        for limits in [
            ResourceLimits::new(8, budget).with_default_action_memory_bytes(0),
            ResourceLimits {
                default_action_memory_bytes: 0,
                ..ResourceLimits::new(8, budget)
            },
        ] {
            assert_eq!(
                limits.undeclared_memory_bytes(),
                DEFAULT_ACTION_MEMORY_BYTES
            );
            assert_eq!(limits.max_parallel_actions(), 4);
        }
        let tiny = ResourceLimits::new(8, 100).with_default_action_memory_bytes(0);
        assert_eq!(tiny.undeclared_memory_bytes(), 100);
        assert_eq!(tiny.max_parallel_actions(), 1);
        let unbounded = ResourceLimits::new(8, 0);
        assert_eq!(
            unbounded.undeclared_memory_bytes(),
            DEFAULT_ACTION_MEMORY_BYTES
        );
        assert_eq!(unbounded.max_parallel_actions(), 8);
        let configured = ResourceLimits::new(8, 1_000).with_default_action_memory_bytes(10);
        assert_eq!(configured.undeclared_memory_bytes(), 10);
        assert_eq!(configured.max_parallel_actions(), 8);
    }
}
