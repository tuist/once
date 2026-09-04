//! Always-on producers for [`RunEventBus`] events.
//!
//! Historically only the `--ui` HTTP dashboard published lifecycle
//! events onto the bus, so nothing else (a terminal renderer, the RFC
//! 0008 ingest, an in-process sound module that wants to subscribe)
//! could observe them. This module provides small helpers that a
//! command's hot path can call unconditionally to broadcast the same
//! events the dashboard used to emit, plus a lightweight
//! [`ActionOutputObserver`] that only pushes `LogChunk` events onto
//! the bus (with no channel or per-run UI store).
//!
//! The UI dashboard still exists; when it is enabled, its
//! [`Publisher`](crate::commands::ui::Publisher) additionally updates a
//! rendering store from these events, but the bus emit itself moves
//! here so it fires whether or not the dashboard is on.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use once_core::{
    ActionOutputObserver, ActionOutputStream, LogStream, Phase, RunEvent, RunEventBus, TargetResult,
};

/// Publish `RunStarted` and `TargetQueued`. Idempotent from the caller's
/// perspective; the bus itself is a broadcast channel that silently
/// succeeds when nothing is subscribed.
pub fn run_started(bus: &RunEventBus, target_id: &str, at_epoch_ms: i64) {
    bus.publish(RunEvent::RunStarted { at_epoch_ms });
    bus.publish(RunEvent::TargetQueued {
        at_epoch_ms,
        target_id: target_id.to_string(),
    });
}

pub fn target_cache_checking(bus: &RunEventBus, target_id: &str) {
    bus.publish(RunEvent::TargetPhase {
        at_epoch_ms: now_epoch_ms(),
        target_id: target_id.to_string(),
        phase: Phase::CacheChecking,
    });
}

pub fn target_preparing(bus: &RunEventBus, target_id: &str) {
    bus.publish(RunEvent::TargetPhase {
        at_epoch_ms: now_epoch_ms(),
        target_id: target_id.to_string(),
        phase: Phase::Preparing,
    });
}

/// Fire `TargetStarted` followed by `TargetPhase(Executing)` for a
/// target that has begun running its action.
pub fn target_executing(bus: &RunEventBus, target_id: &str) {
    let at = now_epoch_ms();
    bus.publish(RunEvent::TargetStarted {
        at_epoch_ms: at,
        target_id: target_id.to_string(),
    });
    bus.publish(RunEvent::TargetPhase {
        at_epoch_ms: at,
        target_id: target_id.to_string(),
        phase: Phase::Executing,
    });
}

pub fn target_capturing(bus: &RunEventBus, target_id: &str) {
    bus.publish(RunEvent::TargetPhase {
        at_epoch_ms: now_epoch_ms(),
        target_id: target_id.to_string(),
        phase: Phase::Capturing,
    });
}

pub fn target_publishing(bus: &RunEventBus, target_id: &str) {
    bus.publish(RunEvent::TargetPhase {
        at_epoch_ms: now_epoch_ms(),
        target_id: target_id.to_string(),
        phase: Phase::Publishing,
    });
}

/// Publish `TargetCompleted` for a target that finished (any outcome)
/// followed by `RunCompleted` for the whole run. The cache label is
/// interpreted case-insensitively: `hit` maps to `was_cached=true`.
pub fn target_finished(
    bus: &RunEventBus,
    target_id: &str,
    duration_ms: u64,
    cache: &str,
    exit_code: i32,
) {
    let result = if exit_code == 0 {
        TargetResult::Succeeded
    } else {
        TargetResult::Failed
    };
    bus.publish(RunEvent::TargetCompleted {
        at_epoch_ms: now_epoch_ms(),
        target_id: target_id.to_string(),
        result,
        was_cached: cache.eq_ignore_ascii_case("hit"),
        duration_ms: i64::try_from(duration_ms).unwrap_or(i64::MAX),
    });
    bus.publish(RunEvent::RunCompleted {
        at_epoch_ms: now_epoch_ms(),
        exit_status: exit_code,
    });
}

/// Publish `TargetCompleted{Failed}` and `RunCompleted{1}` for a run
/// that couldn't even reach the action stage (setup failure).
pub fn target_failed(bus: &RunEventBus, target_id: &str, duration_ms: u64) {
    bus.publish(RunEvent::TargetCompleted {
        at_epoch_ms: now_epoch_ms(),
        target_id: target_id.to_string(),
        result: TargetResult::Failed,
        was_cached: false,
        duration_ms: i64::try_from(duration_ms).unwrap_or(i64::MAX),
    });
    bus.publish(RunEvent::RunCompleted {
        at_epoch_ms: now_epoch_ms(),
        exit_status: 1,
    });
}

/// A lightweight observer that only publishes `LogChunk` events onto
/// the bus. Unlike the UI dashboard's observer, it never queues text
/// into a decoder or channel; the terminal reporter and any other
/// subscriber decode as they render.
pub struct BusOutputObserver {
    bus: RunEventBus,
    target_id: String,
}

impl BusOutputObserver {
    #[must_use]
    pub fn new(bus: RunEventBus, target_id: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            bus,
            target_id: target_id.into(),
        })
    }
}

impl ActionOutputObserver for BusOutputObserver {
    fn observe(&self, stream: ActionOutputStream, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.bus.publish(RunEvent::LogChunk {
            at_epoch_ms: now_epoch_ms(),
            target_id: self.target_id.clone(),
            stream: match stream {
                ActionOutputStream::Stdout => LogStream::Stdout,
                ActionOutputStream::Stderr => LogStream::Stderr,
            },
            bytes: bytes.to_vec(),
        });
    }
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

pub fn now_ms() -> i64 {
    now_epoch_ms()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn helpers_fire_expected_events() {
        let bus = RunEventBus::new(16);
        let mut rx = bus.subscribe();
        run_started(&bus, "//foo:bar", 42);
        target_executing(&bus, "//foo:bar");
        target_finished(&bus, "//foo:bar", 10, "miss", 0);

        assert!(matches!(
            rx.recv().await.unwrap(),
            RunEvent::RunStarted { at_epoch_ms: 42 }
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            RunEvent::TargetQueued { target_id, .. } if target_id == "//foo:bar"
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            RunEvent::TargetStarted { target_id, .. } if target_id == "//foo:bar"
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            RunEvent::TargetPhase {
                phase: Phase::Executing,
                ..
            }
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            RunEvent::TargetCompleted {
                result: TargetResult::Succeeded,
                was_cached: false,
                ..
            }
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            RunEvent::RunCompleted { exit_status: 0, .. }
        ));
    }

    #[tokio::test]
    async fn bus_output_observer_publishes_log_chunks() {
        let bus = RunEventBus::new(8);
        let mut rx = bus.subscribe();
        let observer = BusOutputObserver::new(bus.clone(), "//x:y");
        observer.observe(ActionOutputStream::Stdout, b"hello");

        match rx.recv().await.unwrap() {
            RunEvent::LogChunk {
                target_id,
                stream,
                bytes,
                ..
            } => {
                assert_eq!(target_id, "//x:y");
                assert_eq!(stream, LogStream::Stdout);
                assert_eq!(bytes, b"hello");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn empty_chunk_is_ignored() {
        let bus = RunEventBus::new(4);
        let mut rx = bus.subscribe();
        let observer = BusOutputObserver::new(bus.clone(), "//x:y");
        observer.observe(ActionOutputStream::Stdout, b"");
        // Nothing published; try_recv would immediately error.
        assert!(rx.try_recv().is_err());
    }
}
