//! Typed event bus for a run.
//!
//! Producers publish structured [`RunEvent`] values during execution;
//! subscribers render, forward, or ingest them. The event set defined
//! here is the subset knowable from today's producers. The design
//! target is in `rfcs/0008-live-run-event-protocol.md`; future work
//! grows this enum in step with new fire-points in the runner and
//! executor.

use std::sync::Arc;
use tokio::sync::broadcast;

/// A structured event emitted during a run.
///
/// Variants carry only fields the current producers can populate. The
/// enum is `#[non_exhaustive]` so new variants and fields can land
/// without breaking existing subscribers.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum RunEvent {
    /// The run has started. Emitted once at the top of a run.
    RunStarted { at_epoch_ms: i64 },
    /// The run has ended. Emitted once as the terminal event.
    RunCompleted { at_epoch_ms: i64, exit_status: i32 },
    /// A target has been accepted into the scheduler and is waiting
    /// for its dependencies or a worker.
    TargetQueued { at_epoch_ms: i64, target_id: String },
    /// A target has begun executing.
    TargetStarted { at_epoch_ms: i64, target_id: String },
    /// A target has transitioned into a new phase. See [`Phase`] for
    /// the exclusive ordered lifecycle.
    TargetPhase {
        at_epoch_ms: i64,
        target_id: String,
        phase: Phase,
    },
    /// A target execution finished. `was_cached` covers hit-or-restore
    /// as one boolean; the RFC's richer cache-decision events will
    /// arrive with the cache fire-points.
    TargetCompleted {
        at_epoch_ms: i64,
        target_id: String,
        result: TargetResult,
        was_cached: bool,
        duration_ms: i64,
    },
    /// A worker began preparing one attempt of a declared action.
    ActionAttemptStarted {
        at_epoch_ms: i64,
        target_id: String,
        capability: String,
        action_index: u32,
        attempt: u32,
        worker_id: String,
    },
    /// One attempt ended; the logical action outcome selects an attempt.
    ActionAttemptCompleted {
        at_epoch_ms: i64,
        target_id: String,
        capability: String,
        action_index: u32,
        attempt: u32,
        result: TargetResult,
        exit_code: i32,
        duration_ms: i64,
        was_cached: bool,
    },
    /// One declared action inside a target finished. Emitted per
    /// action so a subscriber can render the internal task list of a
    /// target (Bazel's actions view: "Compiling foo.cc", "Linking
    /// libfoo.a") instead of only the target rollup. `identifier` is
    /// the Starlark-declared identity of the action within its target;
    /// `capability` names the capability the action was declared under
    /// (`build`, `test`, ...). `action_index` is the position within
    /// that target's ordered action list.
    ActionCompleted {
        at_epoch_ms: i64,
        target_id: String,
        capability: String,
        action_index: u32,
        identifier: Option<String>,
        result: TargetResult,
        was_cached: bool,
        duration_ms: i64,
        exit_code: i32,
        // Wall-clock start of the action. When zero the projector
        // derives it from `at_epoch_ms - duration_ms`.
        start_at_epoch_ms: i64,
        // Stable id of the worker (tokio task / OS thread) that ran
        // this action, so the dashboard can render one flame-graph
        // row per worker in the same way Bazel groups by thread.
        worker_id: String,
        // Split of the action's wall time. `prepare_ms` covers arg
        // files, input fingerprinting, and action digest; `execute_ms`
        // covers cache probe + (on a miss) command execution and
        // output upload. Their sum can be less than `duration_ms` on
        // a cache hit because we intentionally leave the small
        // outer bookkeeping out. Zero for cached-replay actions.
        prepare_ms: i64,
        execute_ms: i64,
        // Content-address digest (hex) of this action's cached
        // result — Bazel's action_digest. Persisted on the server so
        // the Cache tab's Cacheable Actions rows can show the exact
        // key Once probed. Empty string when unknown (a failure that
        // never reached the cache probe).
        cache_key: String,
        /// Selected attempt number, or zero for a retained action outcome
        /// with no execution attempt in this run.
        selected_attempt: u32,
    },
    /// A single content blob crossed the cache boundary — either
    /// pulled down from the remote tier ("download") or pushed up
    /// after a miss ("upload"). Emitted once per unique blob per run
    /// so the dashboard's Content Objects view lists real digests +
    /// sizes, and the Cache Summary can total `content_downloaded` /
    /// `content_uploaded` bytes.
    CacheContentTransferred {
        at_epoch_ms: i64,
        // "download" (cache → workspace) or "upload" (workspace → cache).
        kind: String,
        // Target execution that requested the transfer, so the row can
        // link back to its action row on the Cache tab.
        target_id: String,
        // Content-address digest (hex string) of the blob.
        content_hash: String,
        // Size of the blob in bytes.
        size_bytes: i64,
        // Wall time observed for this transfer. Zero when Once merely
        // hardlinked a blob that was already local (a replay), so the
        // dashboard can distinguish a real network fetch from a
        // near-instant local restore.
        duration_ms: i64,
    },
    /// A slice of subprocess output. Scope is a target for now; the
    /// RFC's `LogScope` union arrives with the per-case fire-points.
    LogChunk {
        at_epoch_ms: i64,
        target_id: String,
        stream: LogStream,
        bytes: Vec<u8>,
    },
    /// A test suite (one test target's set of cases) has started.
    TestSuiteStarted {
        at_epoch_ms: i64,
        target_id: String,
        planned_case_count: Option<u32>,
    },
    /// A test suite has completed with aggregate totals.
    TestSuiteCompleted {
        at_epoch_ms: i64,
        target_id: String,
        totals: TestTotals,
    },
    /// A test case has started.
    TestCaseStarted {
        at_epoch_ms: i64,
        target_id: String,
        case_id: String,
        name: String,
        attempt: u32,
    },
    /// A test case has completed with a terminal result.
    TestCaseCompleted {
        at_epoch_ms: i64,
        target_id: String,
        case_id: String,
        name: String,
        suite_id: String,
        attempt: u32,
        result: TestCaseResult,
        duration_ms: i64,
        duration_known: bool,
        failure_message: Option<String>,
    },
    /// Periodic host-resource sample published while a run executes so
    /// the dashboard's timeline can plot CPU, memory, and network
    /// usage over wall-clock. Published at 1 Hz from a single
    /// background task while the run is live.
    SystemSampled {
        at_epoch_ms: i64,
        interval_ms: u32,
        cpu_percent: f32,
        memory_bytes: u64,
        network_in_bytes_per_second: u64,
        network_out_bytes_per_second: u64,
    },
}

/// Terminal outcome of a target execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetResult {
    Succeeded,
    Failed,
    Skipped,
    Cancelled,
}

/// Which subprocess stream produced a log chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogStream {
    Stdout,
    Stderr,
}

/// Exclusive, ordered target execution phases. The projector renders
/// the phase timeline as a single bar per target execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Queued,
    CacheChecking,
    Preparing,
    Executing,
    Capturing,
    Publishing,
}

/// Terminal status of a test case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestCaseResult {
    Unknown,
    Passed,
    Failed,
    Skipped,
    TimedOut,
    Errored,
    Cancelled,
}

/// Aggregate totals for a test suite.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TestTotals {
    pub unknown: u32,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub errored: u32,
    pub timed_out: u32,
    pub cancelled: u32,
}

/// A cloneable broadcast bus for one run.
///
/// Every subscriber receives every event enqueued after it
/// subscribed. When a subscriber falls behind by more than the ring's
/// capacity, the broadcast channel drops its oldest unread events and
/// surfaces a lag error on the next receive; that maps directly onto
/// the loss semantics described in the RFC.
#[derive(Clone)]
pub struct RunEventBus {
    inner: Arc<broadcast::Sender<RunEvent>>,
}

impl RunEventBus {
    /// Create a bus with the given per-subscriber ring capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity.max(1));
        Self {
            inner: Arc::new(tx),
        }
    }

    /// Publish an event. Silently succeeds when no subscribers are
    /// attached; publication never blocks a producer.
    pub fn publish(&self, event: RunEvent) {
        let _ = self.inner.send(event);
    }

    /// Subscribe to events published after this call returns.
    pub fn subscribe(&self) -> broadcast::Receiver<RunEvent> {
        self.inner.subscribe()
    }

    /// Number of currently attached subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.inner.receiver_count()
    }
}

impl std::fmt::Debug for RunEventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunEventBus")
            .field("subscribers", &self.subscriber_count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscriber_receives_published_event() {
        let bus = RunEventBus::new(4);
        let mut rx = bus.subscribe();
        bus.publish(RunEvent::RunStarted { at_epoch_ms: 42 });
        match rx.recv().await.unwrap() {
            RunEvent::RunStarted { at_epoch_ms } => assert_eq!(at_epoch_ms, 42),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn events_published_before_subscribe_are_missed() {
        let bus = RunEventBus::new(4);
        bus.publish(RunEvent::RunStarted { at_epoch_ms: 1 });
        let mut rx = bus.subscribe();
        bus.publish(RunEvent::RunCompleted {
            at_epoch_ms: 2,
            exit_status: 0,
        });
        match rx.recv().await.unwrap() {
            RunEvent::RunCompleted { at_epoch_ms, .. } => assert_eq!(at_epoch_ms, 2),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn publish_without_subscribers_does_not_error() {
        let bus = RunEventBus::new(4);
        bus.publish(RunEvent::RunStarted { at_epoch_ms: 0 });
    }

    #[tokio::test]
    async fn slow_subscriber_lags_when_capacity_exceeded() {
        let bus = RunEventBus::new(2);
        let mut rx = bus.subscribe();
        for i in 0..5 {
            bus.publish(RunEvent::RunStarted { at_epoch_ms: i });
        }
        match rx.recv().await {
            Err(broadcast::error::RecvError::Lagged(_)) => (),
            other => panic!("expected Lagged, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn multiple_subscribers_receive_each_event() {
        let bus = RunEventBus::new(4);
        let mut a = bus.subscribe();
        let mut b = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 2);
        bus.publish(RunEvent::TargetCompleted {
            at_epoch_ms: 10,
            target_id: "//foo:bar".into(),
            result: TargetResult::Succeeded,
            was_cached: true,
            duration_ms: 12,
        });
        for rx in [&mut a, &mut b] {
            match rx.recv().await.unwrap() {
                RunEvent::TargetCompleted {
                    target_id, result, ..
                } => {
                    assert_eq!(target_id, "//foo:bar");
                    assert_eq!(result, TargetResult::Succeeded);
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
    }
}
