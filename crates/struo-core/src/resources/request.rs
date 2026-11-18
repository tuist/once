use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::ResourceLimits;
use crate::{Error, Result};

/// Memory an action is assumed to need when it declares no estimate.
///
/// Admission control needs a number for every action, and guessing too
/// low is what lets a wide graph oversubscribe the host.
pub const DEFAULT_ACTION_MEMORY_BYTES: u64 = 250 * 1024 * 1024;

/// What one action reserves before it is allowed to start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceRequest {
    pub cpu_slots: usize,
    pub memory_bytes: u64,
    /// Named "shared resource" slots the action needs to hold for the
    /// duration of its run. Plugins use this to declare that they
    /// compete for a typed, host-wide resource that the cpu/memory
    /// axes can't model. These slots belong to the host domain, including
    /// when the command itself runs remotely.
    ///
    /// Skipped from JSON when empty so existing actions (which don't
    /// request any named slots) keep their action digest stable.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub slots: BTreeMap<String, usize>,
}

impl ResourceRequest {
    /// Request `cpu_slots` and `memory_bytes`. A request for zero CPU
    /// slots is raised to one, since every action occupies at least one.
    pub fn new(cpu_slots: usize, memory_bytes: u64) -> Self {
        Self {
            cpu_slots: if cpu_slots == 0 { 1 } else { cpu_slots },
            memory_bytes,
            slots: BTreeMap::new(),
        }
    }

    /// Attach a named-slot requirement. Repeated keys overwrite to
    /// match how a plugin would tighten its own resource declaration;
    /// passing 0 removes the entry so callers can clear an inherited
    /// default.
    #[must_use]
    pub fn with_slot(mut self, name: impl Into<String>, count: usize) -> Self {
        let name = name.into();
        if count == 0 {
            self.slots.remove(&name);
        } else {
            self.slots.insert(name, count);
        }
        self
    }

    /// True when nothing was customised, which lets serialization skip
    /// the field and keeps action digests stable.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Treat the request as an estimate: oversized dimensions are clamped
    /// to the pool so a single action can always eventually run alone.
    pub(super) fn bounded_by(self, limits: &ResourceLimits) -> Self {
        let cpu_slots = self.cpu_slots.clamp(1, limits.cpu_slots);
        let requested_memory = self.estimated_memory(limits);
        let memory_bytes = if limits.memory_bytes == 0 {
            requested_memory
        } else {
            requested_memory.min(limits.memory_bytes)
        };
        // Clamp each named slot request to whatever the pool publishes
        // for that name; missing pool entries default to 1 so an
        // unknown slot doesn't deadlock. Plugins that need a slot
        // should configure the pool to publish it explicitly.
        let mut slots = self.slots;
        for (name, requested) in &mut slots {
            *requested = (*requested).clamp(1, limits.slot_capacity(name));
        }
        Self {
            cpu_slots,
            memory_bytes,
            slots,
        }
    }

    /// Treat the request as a minimum: a dimension the pool can never
    /// provide is an error rather than being clamped into apparent
    /// compliance. An undeclared memory estimate still uses the default.
    pub(super) fn required_within(self, limits: &ResourceLimits) -> Result<Self> {
        let unsatisfied = |resource: String, requested: u64, capacity: u64| {
            Err(Error::ResourceRequirementUnsatisfied {
                resource,
                requested,
                capacity,
            })
        };
        let cpu_slots = self.cpu_slots.max(1);
        if cpu_slots > limits.cpu_slots {
            return unsatisfied(
                "cpu slots".to_string(),
                cpu_slots as u64,
                limits.cpu_slots as u64,
            );
        }
        let memory_bytes = self.estimated_memory(limits);
        if limits.memory_bytes != 0 && memory_bytes > limits.memory_bytes {
            return unsatisfied(
                "memory bytes".to_string(),
                memory_bytes,
                limits.memory_bytes,
            );
        }
        for (name, requested) in &self.slots {
            let capacity = limits.slot_capacity(name);
            if *requested > capacity {
                return unsatisfied(format!("slot `{name}`"), *requested as u64, capacity as u64);
            }
        }
        Ok(Self {
            cpu_slots,
            memory_bytes,
            slots: self.slots,
        })
    }

    fn estimated_memory(&self, limits: &ResourceLimits) -> u64 {
        if self.memory_bytes == 0 {
            limits.undeclared_memory_bytes()
        } else {
            self.memory_bytes
        }
    }
}

impl Default for ResourceRequest {
    fn default() -> Self {
        Self {
            cpu_slots: 1,
            memory_bytes: 0,
            slots: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_request_serializes_without_slots_field() {
        // Action-digest stability: requests that don't use named
        // slots must omit the field entirely so existing actions keep
        // the same JSON encoding (and the same cache key). The check
        // looks for the quoted `"slots":` key to avoid matching the
        // `cpu_slots` substring.
        let r = ResourceRequest::default();
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("\"slots\":"), "got {json}");
    }

    #[test]
    fn request_with_slot_round_trips_through_json() {
        let r = ResourceRequest::default().with_slot("exclusive_slot", 1);
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("exclusive_slot"));
        let decoded: ResourceRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.slots.get("exclusive_slot"), Some(&1));
    }

    #[test]
    fn slot_with_zero_count_clears_the_entry() {
        let r = ResourceRequest::default()
            .with_slot("exclusive_slot", 1)
            .with_slot("exclusive_slot", 0);
        assert!(r.slots.is_empty());
    }

    #[test]
    fn required_requests_are_never_clamped() {
        let limits = ResourceLimits::new(2, 100).with_slot_limit("device", 1);
        for (request, resource, requested, capacity) in [
            (ResourceRequest::new(3, 10), "cpu slots", 3, 2),
            (ResourceRequest::new(1, 101), "memory bytes", 101, 100),
            (
                ResourceRequest::new(1, 10).with_slot("device", 2),
                "slot `device`",
                2,
                1,
            ),
            (
                ResourceRequest::new(1, 10).with_slot("unpublished", 2),
                "slot `unpublished`",
                2,
                1,
            ),
        ] {
            match request.required_within(&limits) {
                Err(Error::ResourceRequirementUnsatisfied {
                    resource: actual,
                    requested: actual_requested,
                    capacity: actual_capacity,
                }) => {
                    assert_eq!(actual, resource);
                    assert_eq!(actual_requested, requested);
                    assert_eq!(actual_capacity, capacity);
                }
                other => panic!("expected an unsatisfied requirement, got {other:?}"),
            }
        }
    }

    #[test]
    fn satisfiable_requirements_keep_their_exact_size() {
        let limits = ResourceLimits::new(4, 1_000).with_default_action_memory_bytes(10);
        let required = ResourceRequest::new(4, 1_000)
            .required_within(&limits)
            .unwrap();
        assert_eq!((required.cpu_slots, required.memory_bytes), (4, 1_000));
        let undeclared = ResourceRequest::default().required_within(&limits).unwrap();
        assert_eq!(undeclared.memory_bytes, 10);
        let unbounded_memory = ResourceRequest::new(1, u64::MAX)
            .required_within(&ResourceLimits::new(1, 0))
            .unwrap();
        assert_eq!(unbounded_memory.memory_bytes, u64::MAX);
    }
}
