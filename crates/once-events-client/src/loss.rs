//! Mutable set of loss intervals awaiting delivery as `gap_advances`.
//!
//! The client accumulates dropped-sequence ranges here in real time.
//! At batch send it drains the set, coalesces adjacent or overlapping
//! ranges into a canonical sorted non-overlapping list, and hands
//! that list to the batch as its `gap_advances`. New loss recorded
//! after the drain begins a fresh interval in the (now empty) set
//! and is carried by the next batch.
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
/// - No two intervals overlap or touch (touching intervals coalesce
///   on insert so the canonical shape carries as few `GapAdvance`
///   records as possible).
#[derive(Debug, Default)]
pub struct LossIntervals {
    // Keyed by `first`; each value is `last`.
    intervals: BTreeMap<u64, u64>,
}

impl LossIntervals {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a loss range `[first, last]`. Panics if `first > last`.
    pub fn record(&mut self, first: u64, last: u64, _reason: &str) -> LossPushOutcome {
        assert!(first <= last, "loss range must be non-empty");
        let mut new_first = first;
        let mut new_last = last;
        let mut merged = false;

        // Merge with any interval that touches or overlaps the new range
        // to the left.
        if let Some((&lo, &hi)) = self.left_neighbour(new_first) {
            if hi + 1 >= new_first {
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
            .map(|(k, _)| *k)
            .collect();
        for key in overlapping {
            if let Some(hi) = self.intervals.remove(&key) {
                new_first = new_first.min(key);
                new_last = new_last.max(hi);
                merged = true;
            }
        }

        self.intervals.insert(new_first, new_last);
        if merged {
            LossPushOutcome::Merged
        } else {
            LossPushOutcome::Inserted
        }
    }

    /// Drain the current set into a canonical sorted non-overlapping
    /// list of `GapAdvance` records. Leaves the set empty.
    pub fn drain(&mut self, reason: &str) -> Vec<GapAdvance> {
        let intervals = std::mem::take(&mut self.intervals);
        intervals
            .into_iter()
            .map(|(first, last)| GapAdvance {
                first_dropped_seq: first,
                last_dropped_seq: last,
                reason: reason.to_string(),
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.intervals.is_empty()
    }

    pub fn len(&self) -> usize {
        self.intervals.len()
    }

    fn left_neighbour(&self, key: u64) -> Option<(&u64, &u64)> {
        self.intervals.range(..key).next_back()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges(set: &LossIntervals) -> Vec<(u64, u64)> {
        set.intervals.iter().map(|(a, b)| (*a, *b)).collect()
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
        let out = set.drain("buffer_overflow");
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
