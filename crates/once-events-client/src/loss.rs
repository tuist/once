//! Mutable set of loss intervals awaiting delivery as `gap_advances`.
//!
//! The client accumulates dropped-sequence ranges here in real time.
//! Loss stays pending until the durable acknowledgement passes it. Sending
//! a batch only snapshots these intervals so reconnects can replay them.
//!
//! No sequence number is consumed by loss: the interval is data on
//! the client until the moment it is serialized into a batch, at
//! which point it becomes a batch-level control record.

use std::collections::BTreeMap;

use crate::proto::GapAdvance;

/// Outcome of recording a loss range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LossPushOutcome {
    /// A new interval was inserted.
    Inserted,
    /// The new range extended, or merged with, an existing interval.
    Merged,
}

/// A sorted, non-overlapping set of loss intervals.
///
/// Invariants:
/// - Every interval satisfies `first <= last`.
/// - Intervals never overlap. Touching intervals coalesce when their
///   loss reasons match.
#[derive(Debug, Default)]
pub struct LossIntervals {
    // Keyed by `first`; each value contains `last` and the loss reason.
    intervals: BTreeMap<u64, (u64, String)>,
}

impl LossIntervals {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a loss range `[first, last]`. Panics if `first > last`.
    pub fn record(&mut self, first: u64, last: u64, reason: &str) -> LossPushOutcome {
        assert!(first <= last, "loss range must be non-empty");
        let mut new_first = first;
        let mut new_last = last;
        let mut merged = false;

        // Sequence loss is recorded once. Distinct causes may touch but
        // must not claim the same sequence.
        if let Some((&lo, &(hi, ref previous_reason))) = self.left_neighbour(new_first) {
            assert!(
                hi < new_first || previous_reason == reason,
                "overlapping loss causes"
            );
            if hi.saturating_add(1) >= new_first && previous_reason == reason {
                new_first = new_first.min(lo);
                new_last = new_last.max(hi);
                self.intervals.remove(&lo);
                merged = true;
            }
        }

        // Merge with any interval that overlaps or touches to the right.
        let overlapping: Vec<u64> = self
            .intervals
            .range(new_first..=new_last.saturating_add(1))
            .filter_map(|(key, (_, prior_reason))| {
                assert!(
                    *key > new_last || prior_reason == reason,
                    "overlapping loss causes"
                );
                (prior_reason == reason).then_some(*key)
            })
            .collect();
        for key in overlapping {
            if let Some((hi, _)) = self.intervals.remove(&key) {
                new_first = new_first.min(key);
                new_last = new_last.max(hi);
                merged = true;
            }
        }

        self.intervals
            .insert(new_first, (new_last, reason.to_string()));
        if merged {
            LossPushOutcome::Merged
        } else {
            LossPushOutcome::Inserted
        }
    }

    /// Drain the current set into a canonical sorted non-overlapping
    /// list of `GapAdvance` records. Leaves the set empty.
    pub fn drain(&mut self) -> Vec<GapAdvance> {
        let intervals = std::mem::take(&mut self.intervals);
        intervals
            .into_iter()
            .map(|(first, (last, reason))| GapAdvance {
                first_dropped_seq: first,
                last_dropped_seq: last,
                reason,
            })
            .collect()
    }

    pub fn snapshot(&self) -> Vec<GapAdvance> {
        self.intervals
            .iter()
            .map(|(&first, (last, ref reason))| GapAdvance {
                first_dropped_seq: first,
                last_dropped_seq: *last,
                reason: reason.clone(),
            })
            .collect()
    }

    pub fn snapshot_before(&self, before: u64) -> Vec<GapAdvance> {
        self.intervals
            .range(..before)
            .filter(|(_, (last, _))| *last < before)
            .map(|(&first, (last, ref reason))| GapAdvance {
                first_dropped_seq: first,
                last_dropped_seq: *last,
                reason: reason.clone(),
            })
            .collect()
    }

    pub fn acknowledge_up_to(&mut self, seq: u64) {
        while let Some((&first, &(last, _))) = self.intervals.first_key_value() {
            if first > seq {
                break;
            }
            let (_, reason) = self.intervals.remove(&first).expect("loss interval exists");
            if last > seq {
                self.intervals.insert(seq + 1, (last, reason));
                break;
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.intervals.is_empty()
    }

    pub fn len(&self) -> usize {
        self.intervals.len()
    }

    fn left_neighbour(&self, key: u64) -> Option<(&u64, &(u64, String))> {
        self.intervals.range(..key).next_back()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges(set: &LossIntervals) -> Vec<(u64, u64)> {
        set.intervals.iter().map(|(a, (b, _))| (*a, *b)).collect()
    }

    #[test]
    fn records_a_first_interval() {
        let mut set = LossIntervals::new();
        assert_eq!(set.record(5, 10, "x"), LossPushOutcome::Inserted);
        assert_eq!(ranges(&set), vec![(5, 10)]);
    }

    #[test]
    fn disjoint_intervals_stay_separate() {
        let mut set = LossIntervals::new();
        set.record(1, 3, "x");
        assert_eq!(set.record(10, 20, "x"), LossPushOutcome::Inserted);
        assert_eq!(ranges(&set), vec![(1, 3), (10, 20)]);
    }

    #[test]
    fn touching_intervals_coalesce() {
        let mut set = LossIntervals::new();
        set.record(1, 3, "x");
        assert_eq!(set.record(4, 6, "x"), LossPushOutcome::Merged);
        assert_eq!(ranges(&set), vec![(1, 6)]);
    }

    #[test]
    fn adjacent_losses_keep_their_distinct_causes() {
        let mut set = LossIntervals::new();
        set.record(3, 3, "oversized_event");
        set.record(4, 5, "buffer_overflow");
        let gaps = set.drain();
        assert_eq!(gaps.len(), 2);
        assert_eq!(gaps[0].reason, "oversized_event");
        assert_eq!(gaps[1].reason, "buffer_overflow");
    }

    #[test]
    #[should_panic(expected = "overlapping loss causes")]
    fn conflicting_loss_cannot_duplicate_a_sequence() {
        let mut set = LossIntervals::new();
        set.record(3, 5, "oversized_event");
        set.record(4, 6, "buffer_overflow");
    }

    #[test]
    fn overlapping_intervals_coalesce_left_and_right() {
        let mut set = LossIntervals::new();
        set.record(1, 5, "x");
        set.record(20, 30, "x");
        assert_eq!(set.record(3, 21, "x"), LossPushOutcome::Merged);
        assert_eq!(ranges(&set), vec![(1, 30)]);
    }

    #[test]
    fn many_touching_intervals_collapse_to_one() {
        let mut set = LossIntervals::new();
        for i in 0..5 {
            set.record(i * 3 + 1, i * 3 + 3, "x");
        }
        set.record(4, 15, "x");
        assert_eq!(ranges(&set), vec![(1, 15)]);
    }

    #[test]
    fn drain_emits_sorted_non_overlapping_records() {
        let mut set = LossIntervals::new();
        set.record(100, 200, "x");
        set.record(1, 5, "x");
        set.record(50, 60, "y");
        let out = set.drain();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].first_dropped_seq, 1);
        assert_eq!(out[0].last_dropped_seq, 5);
        assert_eq!(out[1].first_dropped_seq, 50);
        assert_eq!(out[1].last_dropped_seq, 60);
        assert_eq!(out[2].first_dropped_seq, 100);
        assert_eq!(out[2].last_dropped_seq, 200);
        for w in out.windows(2) {
            assert!(w[0].last_dropped_seq < w[1].first_dropped_seq);
        }
        assert!(set.is_empty());
    }

    #[test]
    #[should_panic(expected = "loss range must be non-empty")]
    fn record_rejects_inverted_range() {
        let mut set = LossIntervals::new();
        set.record(10, 5, "x");
    }
}
