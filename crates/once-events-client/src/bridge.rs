//! Bridge from the internal [`once_core::RunEvent`] bus to the wire
//! proto payloads consumed by [`EventSession`].
//!
//! The internal bus carries a compact, evolving enum. The wire proto
//! carries the full RFC-defined vocabulary. The bridge maps between
//! them for the variants we already emit today; new variants land in
//! both places as fire-points multiply.

use once_core::{
    LogStream as CoreStream, Phase as CorePhase, RunEvent as CoreEvent, TargetResult as CoreResult,
    TestCaseResult as CoreCaseResult, TestTotals as CoreTestTotals,
};

use crate::proto::{
    log_scope::Scope as LogScopeVariant, run_event::Payload, LogChunk, LogScope,
    Phase as WirePhase, RunCompleted, RunHeartbeat, RunResult as WireRunResult, RunStarted,
    Stream as WireStream, TargetCompleted, TargetPhase, TargetQueued, TargetResult as WireResult,
    TargetStarted, TestCaseCompleted, TestCaseResult as WireCaseResult, TestCaseStarted,
    TestFailure, TestSuiteCompleted, TestSuiteStarted, TestTotals as WireTestTotals,
};

/// Result of translating one internal event.
pub enum Translated {
    /// Non-terminal event; caller pushes via
    /// [`crate::EventSession::push_ordinary`].
    Ordinary {
        payload: Payload,
        epoch_ms: i64,
        mono_ns: i64,
    },
    /// Terminal event; caller pushes via
    /// [`crate::EventSession::push_terminal`].
    Terminal {
        result: RunCompleted,
        epoch_ms: i64,
        mono_ns: i64,
    },
    /// Event that has no direct wire twin yet; skip.
    Skip,
}

/// Translate one internal event into a wire payload plus a
/// classification the caller uses to route it through the session.
#[allow(clippy::too_many_lines)]
pub fn translate(event: CoreEvent, mono_ns: i64) -> Translated {
    match event {
        CoreEvent::RunStarted { at_epoch_ms } => Translated::Ordinary {
            payload: Payload::RunStarted(RunStarted::default()),
            epoch_ms: at_epoch_ms,
            mono_ns,
        },
        CoreEvent::RunCompleted {
            at_epoch_ms,
            exit_status,
        } => Translated::Terminal {
            result: RunCompleted {
                result: wire_result_from_exit(exit_status) as i32,
                cancellation_reason: String::new(),
                wall_ms: 0,
                totals: None,
            },
            epoch_ms: at_epoch_ms,
            mono_ns,
        },
        CoreEvent::TargetQueued {
            at_epoch_ms,
            target_id,
        } => Translated::Ordinary {
            payload: Payload::TargetQueued(TargetQueued {
                target_execution_id: target_id,
                target_instance_id: String::new(),
                kind: String::new(),
                capability: String::new(),
                action_digest: None,
                input_digest: None,
                dep_target_executions: Vec::new(),
                attempt: 1,
            }),
            epoch_ms: at_epoch_ms,
            mono_ns,
        },
        CoreEvent::TargetStarted {
            at_epoch_ms,
            target_id,
        } => Translated::Ordinary {
            payload: Payload::TargetStarted(TargetStarted {
                target_execution_id: target_id,
                worker_class: String::new(),
            }),
            epoch_ms: at_epoch_ms,
            mono_ns,
        },
        CoreEvent::TargetPhase {
            at_epoch_ms,
            target_id,
            phase,
        } => Translated::Ordinary {
            payload: Payload::TargetPhase(TargetPhase {
                target_execution_id: target_id,
                phase: wire_phase(phase) as i32,
            }),
            epoch_ms: at_epoch_ms,
            mono_ns,
        },
        CoreEvent::TargetCompleted {
            at_epoch_ms,
            target_id,
            result,
            was_cached,
            duration_ms,
        } => Translated::Ordinary {
            payload: Payload::TargetCompleted(TargetCompleted {
                target_execution_id: target_id,
                result: wire_target_result(result) as i32,
                was_cached,
                exit_code: None,
                evidence_digest: None,
                duration_ms,
            }),
            epoch_ms: at_epoch_ms,
            mono_ns,
        },
        CoreEvent::TestSuiteStarted {
            at_epoch_ms,
            target_id,
            planned_case_count,
        } => Translated::Ordinary {
            payload: Payload::TestSuiteStarted(TestSuiteStarted {
                target_execution_id: target_id,
                suite_id: String::new(),
                planned_case_count,
            }),
            epoch_ms: at_epoch_ms,
            mono_ns,
        },
        CoreEvent::TestSuiteCompleted {
            at_epoch_ms,
            target_id,
            totals,
        } => Translated::Ordinary {
            payload: Payload::TestSuiteCompleted(TestSuiteCompleted {
                target_execution_id: target_id,
                suite_id: String::new(),
                totals: Some(wire_test_totals(totals)),
                junit_digest: None,
            }),
            epoch_ms: at_epoch_ms,
            mono_ns,
        },
        CoreEvent::TestCaseStarted {
            at_epoch_ms,
            target_id,
            case_id,
            name,
            attempt,
        } => Translated::Ordinary {
            payload: Payload::TestCaseStarted(TestCaseStarted {
                test_case_execution_id: format!("{target_id}#{case_id}#{attempt}"),
                target_execution_id: target_id,
                case_id,
                name,
                class_name: String::new(),
                file: String::new(),
                parameters: String::new(),
                tags: Vec::new(),
                attempt,
            }),
            epoch_ms: at_epoch_ms,
            mono_ns,
        },
        CoreEvent::TestCaseCompleted {
            at_epoch_ms,
            target_id,
            case_id,
            result,
            duration_ms,
            failure_message,
        } => Translated::Ordinary {
            payload: Payload::TestCaseCompleted(TestCaseCompleted {
                test_case_execution_id: format!("{target_id}#{case_id}#1"),
                result: wire_case_result(result) as i32,
                was_flaky: false,
                duration_ms,
                failure: failure_message.map(|message| TestFailure {
                    message,
                    expected: String::new(),
                    actual: String::new(),
                    stack_digest: None,
                }),
            }),
            epoch_ms: at_epoch_ms,
            mono_ns,
        },
        CoreEvent::LogChunk {
            at_epoch_ms,
            target_id,
            stream,
            bytes,
        } => Translated::Ordinary {
            payload: Payload::LogChunk(LogChunk {
                scope: Some(LogScope {
                    scope: Some(LogScopeVariant::TargetExecutionId(target_id)),
                }),
                stream: wire_stream(stream) as i32,
                offset: 0,
                bytes,
            }),
            epoch_ms: at_epoch_ms,
            mono_ns,
        },
        _ => Translated::Skip,
    }
}

/// A default heartbeat payload for use by the transport's periodic
/// keep-alive.
pub fn heartbeat_payload() -> Payload {
    Payload::RunHeartbeat(RunHeartbeat::default())
}

fn wire_target_result(result: CoreResult) -> WireResult {
    match result {
        CoreResult::Succeeded => WireResult::Succeeded,
        CoreResult::Failed => WireResult::Failed,
        CoreResult::Skipped => WireResult::Skipped,
        CoreResult::Cancelled => WireResult::Cancelled,
    }
}

fn wire_stream(stream: CoreStream) -> WireStream {
    match stream {
        CoreStream::Stdout => WireStream::Stdout,
        CoreStream::Stderr => WireStream::Stderr,
    }
}

fn wire_phase(phase: CorePhase) -> WirePhase {
    match phase {
        CorePhase::Queued => WirePhase::Queued,
        CorePhase::CacheChecking => WirePhase::CacheChecking,
        CorePhase::Preparing => WirePhase::Preparing,
        CorePhase::Executing => WirePhase::Executing,
        CorePhase::Capturing => WirePhase::Capturing,
        CorePhase::Publishing => WirePhase::Publishing,
    }
}

fn wire_case_result(result: CoreCaseResult) -> WireCaseResult {
    match result {
        CoreCaseResult::Passed => WireCaseResult::Passed,
        CoreCaseResult::Failed => WireCaseResult::Failed,
        CoreCaseResult::Skipped => WireCaseResult::Skipped,
        CoreCaseResult::TimedOut => WireCaseResult::TimedOut,
        CoreCaseResult::Errored => WireCaseResult::Errored,
        CoreCaseResult::Cancelled => WireCaseResult::Cancelled,
    }
}

fn wire_test_totals(totals: CoreTestTotals) -> WireTestTotals {
    WireTestTotals {
        passed: totals.passed,
        failed: totals.failed,
        skipped: totals.skipped,
        errored: totals.errored,
        timed_out: totals.timed_out,
        cancelled: totals.cancelled,
        flaky_final_pass: 0,
        flaky_final_fail: 0,
    }
}

fn wire_result_from_exit(exit_status: i32) -> WireRunResult {
    if exit_status == 0 {
        WireRunResult::Succeeded
    } else {
        WireRunResult::Failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_started_translates_to_ordinary() {
        let out = translate(CoreEvent::RunStarted { at_epoch_ms: 42 }, 1);
        matches!(
            out,
            Translated::Ordinary {
                epoch_ms: 42,
                mono_ns: 1,
                ..
            }
        );
    }

    #[test]
    fn run_completed_translates_to_terminal() {
        let out = translate(
            CoreEvent::RunCompleted {
                at_epoch_ms: 100,
                exit_status: 0,
            },
            2,
        );
        match out {
            Translated::Terminal {
                result, epoch_ms, ..
            } => {
                assert_eq!(epoch_ms, 100);
                assert_eq!(result.result, WireRunResult::Succeeded as i32);
            }
            _ => panic!("expected Terminal"),
        }
    }

    #[test]
    fn target_completed_carries_wire_status_and_cache_flag() {
        let out = translate(
            CoreEvent::TargetCompleted {
                at_epoch_ms: 50,
                target_id: "//foo:bar".into(),
                result: CoreResult::Succeeded,
                was_cached: true,
                duration_ms: 12,
            },
            3,
        );
        match out {
            Translated::Ordinary { payload, .. } => match payload {
                Payload::TargetCompleted(t) => {
                    assert_eq!(t.target_execution_id, "//foo:bar");
                    assert_eq!(t.result, WireResult::Succeeded as i32);
                    assert!(t.was_cached);
                    assert_eq!(t.duration_ms, 12);
                }
                other => panic!("wrong payload: {other:?}"),
            },
            _ => panic!("expected Ordinary"),
        }
    }

    #[test]
    fn log_chunk_carries_target_scope_and_stream() {
        let out = translate(
            CoreEvent::LogChunk {
                at_epoch_ms: 10,
                target_id: "//baz".into(),
                stream: CoreStream::Stderr,
                bytes: b"boom\n".to_vec(),
            },
            0,
        );
        match out {
            Translated::Ordinary { payload, .. } => match payload {
                Payload::LogChunk(c) => {
                    assert_eq!(c.stream, WireStream::Stderr as i32);
                    let scope = c.scope.expect("scope").scope.expect("variant");
                    match scope {
                        LogScopeVariant::TargetExecutionId(id) => assert_eq!(id, "//baz"),
                        other => panic!("wrong scope: {other:?}"),
                    }
                    assert_eq!(c.bytes, b"boom\n");
                }
                other => panic!("wrong payload: {other:?}"),
            },
            _ => panic!("expected Ordinary"),
        }
    }
}
