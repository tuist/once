//! Ring buffer with reserved terminal capacity.
//!
//! Ordinary events fill an ordinary ring; the terminal `run.completed`
//! event has its own reserved slot that ordinary events cannot
//! consume, so a burst of log chunks cannot suppress the run's
//! terminal state. Overflow drops the oldest unacknowledged ordinary
//! events and records their sequence numbers as a loss interval on
//! the caller (see [`crate::loss::LossIntervals`]).

use std::collections::VecDeque;

use crate::proto::RunEvent;

/// A single unacknowledged event awaiting send or ack.
#[derive(Clone, Debug)]
pub struct PendingEvent {
    pub seq: u64,
    pub event: RunEvent,
}

/// Outcome of pushing a non-terminal event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RingPushOutcome {
    Accepted,
    /// Buffer was full; the caller must add the dropped sequence
    /// range to its loss intervals.
    OverflowDropped {
        first: u64,
        last: u64,
    },
}

/// Bounded ring holding unacknowledged non-terminal events plus a
/// reserved slot for the terminal `run.completed`.
///
/// The ring does not itself emit `gap_advance` records; the caller
/// composes them from the returned loss ranges via
/// [`crate::loss::LossIntervals`]. The two concerns stay separate so
/// each can be unit-tested independently.
#[derive(Debug)]
pub struct RingBuffer {
    events: VecDeque<PendingEvent>,
    ordinary_capacity: usize,
    terminal_slot: Option<PendingEvent>,
}

impl RingBuffer {
    /// Create a ring with the given non-terminal capacity. The
    /// terminal slot is always exactly one; supplying zero is treated
    /// as one so producers can never wedge on an empty ring.
    pub fn new(ordinary_capacity: usize) -> Self {
        Self {
            events: VecDeque::with_capacity(ordinary_capacity.max(1)),
            ordinary_capacity: ordinary_capacity.max(1),
            terminal_slot: None,
        }
    }

    /// Push a non-terminal event. If the ring is at capacity, drop the
    /// oldest queued event, record it in the returned outcome so the
    /// caller can extend its loss interval, and then place the new
    /// event.
    pub fn push_ordinary(&mut self, event: PendingEvent) -> RingPushOutcome {
        if self.events.len() < self.ordinary_capacity {
            self.events.push_back(event);
            return RingPushOutcome::Accepted;
        }
        let mut first_dropped = u64::MAX;
        let mut last_dropped = 0;
        while self.events.len() >= self.ordinary_capacity {
            let dropped = self
                .events
                .pop_front()
                .expect("ordinary_capacity > 0 by construction");
            if dropped.seq < first_dropped {
                first_dropped = dropped.seq;
            }
            if dropped.seq > last_dropped {
                last_dropped = dropped.seq;
            }
        }
        self.events.push_back(event);
        RingPushOutcome::OverflowDropped {
            first: first_dropped,
            last: last_dropped,
        }
    }

    /// Place the terminal event into its reserved slot. Idempotent:
    /// the second call replaces the first, since a client that emits
    /// two terminals has a bug the server should still see rendered
    /// as the later state.
    pub fn place_terminal(&mut self, event: PendingEvent) {
        self.terminal_slot = Some(event);
    }

    /// Snapshot the current ordinary queue (in order) plus the
    /// terminal slot (last), for a send pass. The caller is expected
    /// to acknowledge them via [`Self::acknowledge_up_to`] once the
    /// server confirms.
    pub fn snapshot(&self) -> Vec<PendingEvent> {
        let mut out: Vec<PendingEvent> = self.events.iter().cloned().collect();
        if let Some(term) = &self.terminal_slot {
            out.push(term.clone());
        }
        out
    }

    /// Drop everything with `seq <= acked_seq` from both the queue
    /// and the terminal slot.
    pub fn acknowledge_up_to(&mut self, acked_seq: u64) {
        while let Some(head) = self.events.front() {
            if head.seq <= acked_seq {
                self.events.pop_front();
            } else {
                break;
            }
        }
        if let Some(term) = &self.terminal_slot {
            if term.seq <= acked_seq {
                self.terminal_slot = None;
            }
        }
    }

    /// Number of unacknowledged non-terminal events currently held.
    pub fn ordinary_len(&self) -> usize {
        self.events.len()
    }

    /// True when the reserved terminal slot holds an event.
    pub fn has_terminal(&self) -> bool {
        self.terminal_slot.is_some()
    }

    /// True when neither the queue nor the terminal slot hold
    /// anything: the run has drained.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty() && self.terminal_slot.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{run_event::Payload, RunEvent, RunStarted};

    fn seq_event(seq: u64) -> PendingEvent {
        PendingEvent {
            seq,
            event: RunEvent {
                seq,
                epoch_ms: 0,
                mono_ns: 0,
                payload: Some(Payload::RunStarted(RunStarted::default())),
            },
        }
    }

    #[test]
    fn accepts_up_to_ordinary_capacity() {
        let mut ring = RingBuffer::new(3);
        for i in 1..=3 {
            assert_eq!(ring.push_ordinary(seq_event(i)), RingPushOutcome::Accepted);
        }
        assert_eq!(ring.ordinary_len(), 3);
    }

    #[test]
    fn overflow_drops_oldest_and_reports_range() {
        let mut ring = RingBuffer::new(2);
        ring.push_ordinary(seq_event(1));
        ring.push_ordinary(seq_event(2));
        match ring.push_ordinary(seq_event(3)) {
            RingPushOutcome::OverflowDropped { first, last } => {
                assert_eq!(first, 1);
                assert_eq!(last, 1);
            }
            other @ RingPushOutcome::Accepted => panic!("expected overflow, got {other:?}"),
        }
        assert_eq!(ring.ordinary_len(), 2);
        let seqs: Vec<u64> = ring.snapshot().iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![2, 3]);
    }

    #[test]
    fn many_pushes_over_capacity_collapse_dropped_range() {
        let mut ring = RingBuffer::new(2);
        ring.push_ordinary(seq_event(1));
        ring.push_ordinary(seq_event(2));
        for seq in 3..=5 {
            let outcome = ring.push_ordinary(seq_event(seq));
            match outcome {
                RingPushOutcome::OverflowDropped { first, last } => {
                    assert!(first <= last);
                    assert!(first >= 1);
                }
                other @ RingPushOutcome::Accepted => panic!("expected overflow, got {other:?}"),
            }
        }
        let seqs: Vec<u64> = ring.snapshot().iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![4, 5]);
    }

    #[test]
    fn terminal_slot_is_independent_of_ordinary_capacity() {
        let mut ring = RingBuffer::new(1);
        ring.push_ordinary(seq_event(1));
        ring.place_terminal(seq_event(9));
        assert!(ring.has_terminal());
        assert_eq!(ring.ordinary_len(), 1);
        // A further ordinary push overflows the ordinary queue but does not
        // evict the terminal slot.
        assert!(matches!(
            ring.push_ordinary(seq_event(2)),
            RingPushOutcome::OverflowDropped { .. }
        ));
        assert!(ring.has_terminal());
    }

    #[test]
    fn ack_up_to_removes_prefix_from_queue_and_terminal() {
        let mut ring = RingBuffer::new(4);
        for seq in 1..=3 {
            ring.push_ordinary(seq_event(seq));
        }
        ring.place_terminal(seq_event(5));
        ring.acknowledge_up_to(2);
        let seqs: Vec<u64> = ring.snapshot().iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![3, 5]);
        ring.acknowledge_up_to(5);
        assert!(ring.is_empty());
    }

    #[test]
    fn zero_capacity_is_raised_to_one() {
        let mut ring = RingBuffer::new(0);
        assert_eq!(ring.push_ordinary(seq_event(1)), RingPushOutcome::Accepted);
    }
}
