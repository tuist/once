//! Client-side event stream state machine.
//!
//! Owns the ring buffer, the mutable loss interval set, and the
//! client's mirror of the server's `expected_next_seq`. Produces
//! ready-to-send `RunEventBatch` values and interprets `BatchAck`
//! responses into concrete actions the transport should take.
//!
//! Transport is intentionally out of scope for this module; a
//! separate live-gRPC layer wraps this state machine and pumps
//! batches over the wire. Keeping the state machine pure makes the
//! delivery semantics from RFC 0008 testable in isolation.

use crate::buffer::{PendingEvent, RingBuffer, RingPushOutcome};
use crate::loss::LossIntervals;
use crate::proto::{
    run_event::Payload, AckDisposition as WireAckDisposition, BatchAck, RunCompleted, RunEvent,
    RunEventAck, RunEventBatch, RunFinalizing,
};
use prost::Message;
use std::collections::HashMap;

/// Batching bounds used by the session.
#[derive(Clone, Copy, Debug)]
pub struct SessionLimits {
    pub ordinary_capacity: usize,
    pub max_events_per_batch: usize,
    pub max_event_bytes: usize,
    pub max_batch_bytes: usize,
    pub max_log_chunk_bytes: usize,
    pub max_unacked_bytes: usize,
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            ordinary_capacity: 4096,
            max_events_per_batch: 512,
            max_event_bytes: 64 * 1024,
            max_batch_bytes: 256 * 1024,
            max_log_chunk_bytes: 16 * 1024,
            max_unacked_bytes: 8 * 1024 * 1024,
        }
    }
}

/// Client-side session for one run.
pub struct EventSession {
    run_id: String,
    next_seq: u64,
    /// Client mirror of the server's `expected_next_seq`. Updated on
    /// every `BatchAck` and on any `RunEventAck` polled via
    /// `GetRunAck` after reconnect.
    server_expected_next_seq: u64,
    ring: RingBuffer,
    started: Option<RunEvent>,
    loss: LossIntervals,
    limits: SessionLimits,
    finalized_locally: bool,
    batch_counter: u64,
    producer_dropped_events: u64,
    producer_loss_dirty: bool,
    last_sent_producer_loss: u64,
    log_offsets: HashMap<(String, i32), i64>,
}

/// What the transport should do after handling an ack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AckAction {
    /// Ack advanced the durable frontier. Client may send the next
    /// pending batch. Optional throttle advice from the server.
    Continue { retry_after_ms: u32 },
    /// Server rejected the batch as stale (retry of already-acked
    /// prefix). The batch is safely dropped by the client; the
    /// session already knows the server is past it. Transport moves
    /// on.
    StaleDropped,
    /// Server rejected the batch as structurally invalid. The
    /// transport should surface an error; this indicates a client
    /// bug.
    InvalidRejected,
    /// Server told the client to resync. Transport should call
    /// `GetRunAck` and pass the response to `handle_get_run_ack`
    /// before sending anything else.
    NeedsResync,
}

impl EventSession {
    pub fn new(run_id: impl Into<String>, limits: SessionLimits) -> Self {
        Self {
            run_id: run_id.into(),
            next_seq: 1,
            server_expected_next_seq: 1,
            ring: RingBuffer::with_byte_capacity(
                limits.ordinary_capacity,
                limits.max_unacked_bytes,
            ),
            started: None,
            loss: LossIntervals::new(),
            limits,
            finalized_locally: false,
            batch_counter: 0,
            producer_dropped_events: 0,
            producer_loss_dirty: false,
            last_sent_producer_loss: 0,
            log_offsets: HashMap::new(),
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    pub fn record_producer_loss(&mut self, count: u64) {
        self.producer_dropped_events = self.producer_dropped_events.saturating_add(count);
        self.producer_loss_dirty = true;
    }

    pub fn producer_dropped_events(&self) -> u64 {
        self.producer_dropped_events
    }

    /// Push a non-terminal event onto the ring. Overflow records the
    /// dropped range in the loss interval set (data on the client,
    /// not an event) so the next batch's `gap_advances` carries it.
    pub fn push_ordinary(&mut self, payload: Payload, epoch_ms: i64, mono_ns: i64) -> u64 {
        if let Payload::LogChunk(mut chunk) = payload {
            if self.finalized_locally {
                return self.next_seq - 1;
            }
            let key = (format!("{:?}", chunk.scope), chunk.stream);
            let offset = self.log_offsets.entry(key).or_default();
            chunk.offset = *offset;
            *offset = offset.saturating_add(i64::try_from(chunk.bytes.len()).unwrap_or(i64::MAX));
            let bytes = std::mem::take(&mut chunk.bytes);
            let mut last = self.next_seq - 1;
            for (index, part) in bytes
                .chunks(self.limits.max_log_chunk_bytes.max(1))
                .enumerate()
            {
                let mut fragment = chunk.clone();
                let byte_offset = index.saturating_mul(self.limits.max_log_chunk_bytes.max(1));
                fragment.offset = chunk
                    .offset
                    .saturating_add(i64::try_from(byte_offset).unwrap_or(i64::MAX));
                fragment.bytes = part.to_vec();
                last = self.push_event(Payload::LogChunk(fragment), epoch_ms, mono_ns);
            }
            return last;
        }
        self.push_event(payload, epoch_ms, mono_ns)
    }

    fn push_event(&mut self, payload: Payload, epoch_ms: i64, mono_ns: i64) -> u64 {
        // A straggler event can arrive after the session was
        // finalized (system sampler ticks that raced past
        // RunCompleted, late `TargetPhase`s, etc.). Dropping them
        // silently is preferable to panicking the CLI at exit.
        if self.finalized_locally {
            return self.next_seq - 1;
        }
        assert!(
            !matches!(payload, Payload::RunCompleted(_)),
            "use push_terminal for RunCompleted",
        );
        let seq = self.next_seq;
        self.next_seq += 1;
        let event = RunEvent {
            seq,
            epoch_ms,
            mono_ns,
            payload: Some(payload),
        };
        if seq == 1 && matches!(event.payload, Some(Payload::RunStarted(_))) {
            self.started = Some(event);
            return seq;
        }
        if event.encoded_len() > self.limits.max_event_bytes {
            self.loss.record(seq, seq, "oversized_event");
            return seq;
        }
        match self.ring.push_ordinary(PendingEvent { seq, event }) {
            RingPushOutcome::Accepted => {}
            RingPushOutcome::OverflowDropped { first, last } => {
                self.loss.record(first, last, "buffer_overflow");
            }
            RingPushOutcome::OversizedDropped { seq } => {
                self.loss.record(seq, seq, "buffer_byte_limit");
            }
        }
        seq
    }

    /// Push the terminal `run.completed` event into the reserved
    /// slot. Sets the finalized flag; further pushes leave the terminal sequence unchanged.
    pub fn push_terminal(&mut self, result: RunCompleted, epoch_ms: i64, mono_ns: i64) -> u64 {
        if self.finalized_locally {
            return self.next_seq - 1;
        }
        let finalizing_seq = self.next_seq;
        self.next_seq += 1;
        self.ring.place_finalizing(PendingEvent {
            seq: finalizing_seq,
            event: RunEvent {
                seq: finalizing_seq,
                epoch_ms,
                mono_ns,
                payload: Some(Payload::RunFinalizing(RunFinalizing {
                    declared_drain_ms: 0,
                })),
            },
        });
        let seq = self.next_seq;
        self.next_seq += 1;
        self.finalized_locally = true;
        let event = RunEvent {
            seq,
            epoch_ms,
            mono_ns,
            payload: Some(Payload::RunCompleted(result)),
        };
        self.ring.place_terminal(PendingEvent { seq, event });
        seq
    }

    /// Compose the next batch to send. Returns None when there is
    /// nothing to send (no queued events, no loss to declare, no
    /// terminal pending).
    pub fn next_batch(&mut self) -> Option<RunEventBatch> {
        if self.next_seq == 1 {
            return None;
        }
        if let Some(started) = &self.started {
            self.batch_counter += 1;
            self.last_sent_producer_loss = self.producer_dropped_events;
            return Some(RunEventBatch {
                run_id: self.run_id.clone(),
                batch_id: format!("{}-b{}", self.run_id, self.batch_counter),
                seq_from: 1,
                events: vec![started.clone()],
                gap_advances: Vec::new(),
                producer_dropped_events: self.producer_dropped_events,
            });
        }
        let snapshot = self
            .ring
            .snapshot_up_to(self.limits.max_events_per_batch.max(1));
        let first_seq = snapshot.first().map_or(self.next_seq, |event| event.seq);
        let gap_advances = self.loss.snapshot_before(first_seq);
        if snapshot.is_empty() && gap_advances.is_empty() && !self.producer_loss_dirty {
            return None;
        }
        self.batch_counter += 1;
        self.last_sent_producer_loss = self.producer_dropped_events;
        let batch_id = format!("{}-b{}", self.run_id, self.batch_counter);

        // Compose canonical shape: all gap_advances strictly before seq_from.
        // seq_from is either the first snapshot event's seq or, for a
        // control-only batch, the client's assertion of the next event seq.
        let (seq_from, events): (u64, Vec<RunEvent>) = if snapshot.is_empty() {
            // Control-only batch: seq_from asserts the client's expected
            // next event seq. That is `next_seq` (nothing minted yet).
            (self.next_seq, Vec::new())
        } else {
            let mut evs = Vec::with_capacity(snapshot.len());
            for pending in snapshot
                .into_iter()
                .take(self.limits.max_events_per_batch.max(1))
            {
                if evs
                    .last()
                    .is_some_and(|event: &RunEvent| event.seq + 1 != pending.seq)
                {
                    break;
                }
                evs.push(pending.event);
            }
            (evs[0].seq, evs)
        };

        let mut batch = RunEventBatch {
            run_id: self.run_id.clone(),
            batch_id,
            gap_advances: Vec::new(),
            seq_from,
            events: Vec::new(),
            producer_dropped_events: self.producer_dropped_events,
        };
        for gap in gap_advances {
            batch.gap_advances.push(gap);
            if batch.encoded_len() > self.limits.max_batch_bytes {
                batch.gap_advances.pop();
                break;
            }
        }
        if batch.gap_advances.len() < self.loss.snapshot_before(first_seq).len() {
            batch.seq_from = batch
                .gap_advances
                .last()
                .map_or(self.server_expected_next_seq, |gap| {
                    gap.last_dropped_seq + 1
                });
            return Some(batch);
        }
        for event in events {
            batch.events.push(event);
            if batch.encoded_len() > self.limits.max_batch_bytes {
                batch.events.pop();
                break;
            }
        }
        Some(batch)
    }

    /// Apply a `BatchAck`. Returns the action the transport should
    /// take next.
    pub fn handle_ack(&mut self, ack: &BatchAck) -> AckAction {
        if ack.run_id != self.run_id
            || ack.expected_next_seq != ack.acked_seq.saturating_add(1)
            || ack.expected_next_seq > self.next_seq
            || ack.expected_next_seq < self.server_expected_next_seq
        {
            return AckAction::InvalidRejected;
        }
        match LocalDisposition::from(ack.disposition) {
            LocalDisposition::Accepted => {
                self.server_expected_next_seq = ack.expected_next_seq;
                self.acknowledge_up_to(ack.acked_seq);
                self.producer_loss_dirty =
                    self.producer_dropped_events > self.last_sent_producer_loss;

                AckAction::Continue {
                    retry_after_ms: ack.retry_after_ms,
                }
            }
            LocalDisposition::RejectedStale => {
                // Whole batch was a duplicate the server already had.
                // Trust the server's expected_next_seq and drop the
                // resent events from the ring.
                self.server_expected_next_seq = ack.expected_next_seq;
                if ack.expected_next_seq > 0 {
                    self.acknowledge_up_to(ack.expected_next_seq - 1);
                }
                AckAction::StaleDropped
            }
            LocalDisposition::RejectedInvalid | LocalDisposition::Unspecified => {
                AckAction::InvalidRejected
            }
            LocalDisposition::NeedsResync => AckAction::NeedsResync,
        }
    }

    /// Apply a `RunEventAck` (from a `GetRunAck` reconnect probe).
    /// The client aligns its ring and `next_seq` mirror with the
    /// server's durable state before sending anything else.
    pub fn handle_get_run_ack(&mut self, ack: &RunEventAck) -> AckAction {
        if ack.run_id != self.run_id
            || ack.expected_next_seq != ack.acked_seq.saturating_add(1)
            || ack.expected_next_seq > self.next_seq
            || ack.expected_next_seq < self.server_expected_next_seq
        {
            return AckAction::InvalidRejected;
        }
        self.server_expected_next_seq = ack.expected_next_seq;
        self.producer_loss_dirty = self.producer_dropped_events > 0;
        if ack.acked_seq > 0 {
            self.acknowledge_up_to(ack.acked_seq);
        }
        AckAction::Continue { retry_after_ms: 0 }
    }

    fn acknowledge_up_to(&mut self, seq: u64) {
        if seq >= 1 {
            self.started = None;
        }
        self.ring.acknowledge_up_to(seq);
        self.loss.acknowledge_up_to(seq);
    }

    pub fn is_drained(&self) -> bool {
        self.started.is_none()
            && self.ring.is_empty()
            && self.loss.is_empty()
            && !self.producer_loss_dirty
    }

    pub fn finalized_locally(&self) -> bool {
        self.finalized_locally
    }

    pub fn server_expected_next_seq(&self) -> u64 {
        self.server_expected_next_seq
    }
}

/// Local mirror of the wire `AckDisposition` enum so tests do not
/// have to reach into `i32` constants at their call sites.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocalDisposition {
    Unspecified,
    Accepted,
    RejectedStale,
    RejectedInvalid,
    NeedsResync,
}

impl From<i32> for LocalDisposition {
    fn from(value: i32) -> Self {
        match value {
            v if v == WireAckDisposition::Accepted as i32 => Self::Accepted,
            v if v == WireAckDisposition::RejectedStale as i32 => Self::RejectedStale,
            v if v == WireAckDisposition::RejectedInvalid as i32 => Self::RejectedInvalid,
            v if v == WireAckDisposition::NeedsResync as i32 => Self::NeedsResync,
            _ => Self::Unspecified,
        }
    }
}

/// Re-export of the generated `AckDisposition` enum so tests and
/// downstream callers can construct dispositions without importing
/// the deeper `crate::proto` path.
pub use crate::proto::AckDisposition;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{RunFinalization, RunHeartbeat, RunStarted};

    fn started() -> Payload {
        Payload::RunStarted(RunStarted::default())
    }

    fn heartbeat() -> Payload {
        Payload::RunHeartbeat(RunHeartbeat::default())
    }

    fn ack(run_id: &str, seq: u64, disposition: AckDisposition) -> BatchAck {
        BatchAck {
            run_id: run_id.into(),
            batch_id: format!("{run_id}-b1"),
            disposition: disposition as i32,
            acked_seq: seq,
            expected_next_seq: seq + 1,
            observed_high_water_seq: 0,
            retry_after_ms: 0,
            max_in_flight_batches: 0,
            finalization: RunFinalization::Active as i32,
            dashboard_url: String::new(),
        }
    }

    #[test]
    fn loss_survives_failed_send_and_partial_acknowledgement() {
        let mut s = EventSession::new(
            "r1",
            SessionLimits {
                ordinary_capacity: 1,
                max_events_per_batch: 1,
                ..SessionLimits::default()
            },
        );
        for _ in 0..4 {
            s.push_ordinary(heartbeat(), 0, 0);
        }
        let first = s.next_batch().unwrap();
        assert_eq!(s.next_batch().unwrap().gap_advances, first.gap_advances);
        s.handle_ack(&ack("r1", 2, AckDisposition::Accepted));
        let retry = s.next_batch().unwrap();
        assert_eq!(retry.gap_advances[0].first_dropped_seq, 3);
        assert_eq!(retry.gap_advances[0].last_dropped_seq, 3);
        s.handle_ack(&ack("r1", 4, AckDisposition::Accepted));
        assert!(s.is_drained());
    }

    #[test]
    fn creation_survives_overflow_before_first_ack() {
        let mut s = EventSession::new(
            "r1",
            SessionLimits {
                ordinary_capacity: 1,
                max_events_per_batch: 1,
                ..SessionLimits::default()
            },
        );
        s.push_ordinary(started(), 0, 0);
        for _ in 0..4 {
            s.push_ordinary(heartbeat(), 0, 0);
        }
        let batch = s.next_batch().unwrap();
        assert_eq!(batch.seq_from, 1);
        assert!(batch.gap_advances.is_empty());
        assert!(matches!(
            batch.events[0].payload,
            Some(Payload::RunStarted(_))
        ));
        s.handle_ack(&ack("r1", 1, AckDisposition::Accepted));
        let batch = s.next_batch().unwrap();
        assert_eq!(batch.gap_advances[0].first_dropped_seq, 2);
        assert_eq!(batch.gap_advances[0].last_dropped_seq, 4);
        assert_eq!(batch.seq_from, 5);
    }

    #[test]
    fn late_events_and_duplicate_terminal_do_not_change_sequence_space() {
        let mut s = EventSession::new("r1", SessionLimits::default());
        s.push_terminal(RunCompleted::default(), 0, 0);
        s.push_ordinary(heartbeat(), 0, 0);
        s.push_terminal(RunCompleted::default(), 0, 0);
        let batch = s.next_batch().unwrap();
        assert_eq!(batch.events.len(), 2);
        assert!(matches!(
            batch.events[0].payload,
            Some(Payload::RunFinalizing(_))
        ));
        assert!(matches!(
            batch.events[1].payload,
            Some(Payload::RunCompleted(_))
        ));
        assert_eq!(s.next_seq(), 3);
        assert!(batch.gap_advances.is_empty());
        s.handle_ack(&ack("r1", 2, AckDisposition::Accepted));
        assert!(s.is_drained());
    }

    #[test]
    fn log_chunks_split_with_contiguous_offsets_and_byte_limits() {
        let mut session = EventSession::new(
            "run",
            SessionLimits {
                max_log_chunk_bytes: 3,
                ..SessionLimits::default()
            },
        );
        let chunk = crate::proto::LogChunk {
            scope: None,
            stream: 1,
            offset: 0,
            bytes: b"abcde".to_vec(),
        };
        session.push_ordinary(Payload::LogChunk(chunk.clone()), 0, 0);
        session.push_ordinary(Payload::LogChunk(chunk), 0, 0);
        let batch = session.next_batch().unwrap();
        let fragments: Vec<_> = batch
            .events
            .iter()
            .filter_map(|event| match &event.payload {
                Some(Payload::LogChunk(chunk)) => Some((chunk.offset, chunk.bytes.as_slice())),
                _ => None,
            })
            .collect();
        assert_eq!(
            fragments,
            vec![
                (0, &b"abc"[..]),
                (3, &b"de"[..]),
                (5, &b"abc"[..]),
                (8, &b"de"[..])
            ]
        );
    }

    #[test]
    fn encoded_batches_and_retained_events_stay_within_byte_limits() {
        let mut session = EventSession::new(
            "run",
            SessionLimits {
                max_batch_bytes: 128,
                max_unacked_bytes: 48,
                ..SessionLimits::default()
            },
        );
        for _ in 0..40 {
            session.push_ordinary(heartbeat(), 0, 0);
        }
        let batch = session.next_batch().unwrap();
        assert!(batch.encoded_len() <= 128);
        assert!(!batch.gap_advances.is_empty());
        assert!(batch
            .gap_advances
            .iter()
            .all(|gap| gap.reason == "buffer_overflow"));
    }

    #[test]
    fn invalid_ack_cannot_discard_pending_data() {
        let mut s = EventSession::new("r1", SessionLimits::default());
        s.push_ordinary(started(), 0, 0);
        assert_eq!(
            s.handle_ack(&ack("other", 1, AckDisposition::Accepted)),
            AckAction::InvalidRejected
        );
        assert_eq!(
            s.handle_ack(&ack("r1", 9, AckDisposition::Accepted)),
            AckAction::InvalidRejected
        );
        assert!(!s.is_drained());
    }

    #[test]
    fn empty_session_produces_no_batch() {
        let mut s = EventSession::new("r1", SessionLimits::default());
        assert!(s.next_batch().is_none());
    }

    #[test]
    fn single_event_batch_carries_expected_shape() {
        let mut s = EventSession::new("r1", SessionLimits::default());
        s.push_ordinary(started(), 100, 0);
        let batch = s.next_batch().expect("batch present");
        assert_eq!(batch.run_id, "r1");
        assert_eq!(batch.seq_from, 1);
        assert_eq!(batch.events.len(), 1);
        assert!(batch.gap_advances.is_empty());
    }

    #[test]
    fn accepted_ack_drops_events_from_ring() {
        let mut s = EventSession::new("r1", SessionLimits::default());
        s.push_ordinary(started(), 1, 0);
        s.push_ordinary(heartbeat(), 2, 0);
        let _batch = s.next_batch().unwrap();
        let action = s.handle_ack(&ack("r1", 2, AckDisposition::Accepted));
        assert!(matches!(action, AckAction::Continue { .. }));
        assert!(s.is_drained());
        assert_eq!(s.server_expected_next_seq(), 3);
    }

    #[test]
    fn overflow_produces_gap_advance_on_next_batch() {
        let mut s = EventSession::new(
            "r1",
            SessionLimits {
                ordinary_capacity: 2,
                max_events_per_batch: 100,
                ..SessionLimits::default()
            },
        );
        for _ in 0..5 {
            s.push_ordinary(heartbeat(), 0, 0);
        }
        let batch = s.next_batch().unwrap();
        assert_eq!(batch.gap_advances.len(), 1);
        assert_eq!(batch.gap_advances[0].first_dropped_seq, 1);
        assert_eq!(batch.gap_advances[0].last_dropped_seq, 3);
        assert_eq!(batch.seq_from, 4);
        assert_eq!(batch.events.len(), 2);
    }

    #[test]
    fn stale_ack_drops_resent_prefix() {
        let mut s = EventSession::new("r1", SessionLimits::default());
        s.push_ordinary(started(), 1, 0);
        s.push_ordinary(heartbeat(), 2, 0);
        // Simulate: server already accepted 1..=2 but our ack was lost.
        // The client retries. Server responds RejectedStale with
        // expected_next_seq = 3.
        let stale = BatchAck {
            run_id: "r1".into(),
            batch_id: "r1-b1".into(),
            disposition: AckDisposition::RejectedStale as i32,
            acked_seq: 2,
            expected_next_seq: 3,
            observed_high_water_seq: 0,
            retry_after_ms: 0,
            max_in_flight_batches: 0,
            finalization: RunFinalization::Active as i32,
            dashboard_url: String::new(),
        };
        let action = s.handle_ack(&stale);
        assert_eq!(action, AckAction::StaleDropped);
        assert!(s.is_drained());
    }

    #[test]
    fn needs_resync_action_leaves_state_intact() {
        let mut s = EventSession::new("r1", SessionLimits::default());
        s.push_ordinary(started(), 1, 0);
        let resync = BatchAck {
            run_id: "r1".into(),
            batch_id: "r1-b1".into(),
            disposition: AckDisposition::NeedsResync as i32,
            acked_seq: 0,
            expected_next_seq: 1,
            observed_high_water_seq: 0,
            retry_after_ms: 0,
            max_in_flight_batches: 0,
            finalization: RunFinalization::Active as i32,
            dashboard_url: String::new(),
        };
        let action = s.handle_ack(&resync);
        assert_eq!(action, AckAction::NeedsResync);
        assert!(!s.is_drained());
    }

    #[test]
    fn get_run_ack_reconciles_client_mirror() {
        let mut s = EventSession::new("r1", SessionLimits::default());
        s.push_ordinary(started(), 1, 0);
        s.push_ordinary(heartbeat(), 2, 0);
        // Server tells us it durably has 1..=2.
        let ack = RunEventAck {
            run_id: "r1".into(),
            acked_seq: 2,
            expected_next_seq: 3,
            observed_high_water_seq: 0,
            finalization: RunFinalization::Active as i32,
            dashboard_url: String::new(),
        };
        s.handle_get_run_ack(&ack);
        assert!(s.is_drained());
        assert_eq!(s.server_expected_next_seq(), 3);
    }

    #[test]
    fn terminal_event_survives_prior_overflow() {
        let mut s = EventSession::new(
            "r1",
            SessionLimits {
                ordinary_capacity: 1,
                max_events_per_batch: 100,
                ..SessionLimits::default()
            },
        );
        // Overflow the ordinary queue before finalizing.
        for _ in 0..4 {
            s.push_ordinary(heartbeat(), 0, 0);
        }
        s.push_terminal(RunCompleted::default(), 0, 0);
        let batch = s.next_batch().unwrap();
        // The batch includes the terminal at the end even though the
        // ordinary queue overflowed earlier.
        let last = batch.events.last().unwrap();
        assert!(matches!(last.payload, Some(Payload::RunCompleted(_))));
        assert!(!batch.gap_advances.is_empty());
    }
}
