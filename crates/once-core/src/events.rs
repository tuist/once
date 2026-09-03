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
    RunCompleted {
        at_epoch_ms: i64,
        exit_status: i32,
    },
    /// A target has been accepted into the scheduler and is waiting
    /// for its dependencies or a worker.
    TargetQueued {
        at_epoch_ms: i64,
        target_id: String,
    },
    /// A target has begun executing.
    TargetStarted {
        at_epoch_ms: i64,
        target_id: String,
    },
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
        result: TestCaseResult,
        duration_ms: i64,
        failure_message: Option<String>,
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
