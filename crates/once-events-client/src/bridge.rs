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
    log_scope::Scope as LogScopeVariant, run_event::Payload, ActionAttemptCompleted,
    ActionAttemptStarted, ActionCompleted, CacheDownload, CacheUpload, ContentRef, HashAlgorithm,
    LogChunk, LogScope, Phase as WirePhase, ResourceScope, RunCompleted, RunHeartbeat,
    RunResult as WireRunResult, RunStarted, Stream as WireStream, SystemSampled, TargetCompleted,
    TargetPhase, TargetQueued, TargetResult as WireResult, TargetStarted, TestCaseCompleted,
    TestCaseResult as WireCaseResult, TestCaseStarted, TestFailure, TestSuiteCompleted,
    TestSuiteStarted, TestTotals as WireTestTotals,
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
                producer_dropped_events: 0,
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
                test_case_execution_id: test_case_execution_id(&target_id, &case_id, attempt),
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
            name,
            suite_id,
            attempt,
            result,
            duration_ms,
            duration_known,
            failure_message,
        } => Translated::Ordinary {
            payload: Payload::TestCaseCompleted(TestCaseCompleted {
                test_case_execution_id: test_case_execution_id(&target_id, &case_id, attempt),
                case_id,
                name,
                suite_id,
                attempt,
                result: wire_case_result(result) as i32,
                was_flaky: false,
                duration_ms,
                observed_duration_ms: duration_known.then_some(duration_ms),
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
        CoreEvent::ActionAttemptStarted {
            at_epoch_ms,
            target_id,
            capability,
            action_index,
            attempt,
            worker_id,
        } => Translated::Ordinary {
            payload: Payload::ActionAttemptStarted(ActionAttemptStarted {
                target_execution_id: target_id,
                capability,
                action_index,
                attempt,
                worker_id,
            }),
            epoch_ms: at_epoch_ms,
            mono_ns,
        },
        CoreEvent::ActionAttemptCompleted {
            at_epoch_ms,
            target_id,
            capability,
            action_index,
            attempt,
            result,
            exit_code,
            duration_ms,
            was_cached,
        } => Translated::Ordinary {
            payload: Payload::ActionAttemptCompleted(ActionAttemptCompleted {
                target_execution_id: target_id,
                capability,
                action_index,
                attempt,
                result: wire_target_result(result) as i32,
                exit_code,
                duration_ms,
                was_cached,
            }),
            epoch_ms: at_epoch_ms,
            mono_ns,
        },
        CoreEvent::ActionCompleted {
            at_epoch_ms,
            target_id,
            capability,
            action_index,
            identifier,
            result,
            was_cached,
            duration_ms,
            exit_code,
            start_at_epoch_ms,
            worker_id,
            prepare_ms,
            execute_ms,
            cache_key,
            selected_attempt,
        } => Translated::Ordinary {
            payload: if capability == "_phase" {
                Payload::TargetPhaseCompleted(crate::proto::TargetPhaseCompleted {
                    target_execution_id: target_id,
                    phase: identifier.unwrap_or_default(),
                    worker_id,
                    start_at_epoch_ms,
                    duration_ms,
                })
            } else {
                Payload::ActionCompleted(ActionCompleted {
                    target_execution_id: target_id,
                    capability,
                    action_index,
                    identifier: identifier.unwrap_or_default(),
                    result: wire_target_result(result) as i32,
                    was_cached,
                    duration_ms,
                    exit_code,
                    start_at_epoch_ms,
                    worker_id,
                    prepare_ms,
                    execute_ms,
                    cache_key,
                    selected_attempt,
                })
            },
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
        CoreEvent::SystemSampled {
            at_epoch_ms,
            interval_ms,
            cpu_percent,
            memory_bytes,
            network_in_bytes_per_second,
            network_out_bytes_per_second,
        } => Translated::Ordinary {
            payload: Payload::SystemSampled(SystemSampled {
                at_epoch_ms,
                interval_ms,
                resource_id: "reporting-host".to_string(),
                scope: ResourceScope::Host as i32,
                cpu_percent,
                memory_bytes,
                network_in_bytes_per_second,
                network_out_bytes_per_second,
            }),
            epoch_ms: at_epoch_ms,
            mono_ns,
        },
        CoreEvent::CacheContentTransferred {
            at_epoch_ms,
            kind,
            target_id,
            content_hash,
            size_bytes,
            duration_ms,
        } => {
            let Some(digest) = decode_digest(&content_hash) else {
                tracing::warn!("omitting cache transfer with invalid digest");
                return Translated::Skip;
            };
            let content = ContentRef {
                hash_algorithm: HashAlgorithm::Blake3 as i32,
                digest,
                size_bytes: u64::try_from(size_bytes).unwrap_or_default(),
                namespace: String::new(),
                media_type: String::new(),
            };
            let bytes = u64::try_from(size_bytes).unwrap_or_default();
            let payload = if kind == "upload" {
                Payload::CacheUpload(CacheUpload {
                    cache_decision_id: String::new(),
                    target_execution_id: target_id,
                    content: Some(content),
                    tier: "remote".to_string(),
                    kind: "output".to_string(),
                    duration_ms,
                    bytes_transferred: bytes,
                })
            } else {
                Payload::CacheDownload(CacheDownload {
                    cache_decision_id: String::new(),
                    target_execution_id: target_id,
                    content: Some(content),
                    tier: "remote".to_string(),
                    kind: "output".to_string(),
                    duration_ms,
                    bytes_transferred: bytes,
                })
            };
            Translated::Ordinary {
                payload,
                epoch_ms: at_epoch_ms,
                mono_ns,
            }
        }
        _ => Translated::Skip,
    }
}

fn decode_digest(value: &str) -> Option<Vec<u8>> {
    if value.len() != 64 || !value.is_ascii() {
        return None;
    }
    (0..64)
        .step_by(2)
        .map(|offset| u8::from_str_radix(&value[offset..offset + 2], 16).ok())
        .collect()
}

/// A default heartbeat payload for use by the transport's periodic
/// keep-alive.
pub fn heartbeat_payload() -> Payload {
    Payload::RunHeartbeat(RunHeartbeat::default())
}

fn test_case_execution_id(target: &str, case: &str, attempt: u32) -> String {
    format!("{}:{target}:{}:{case}:{attempt}", target.len(), case.len())
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
        CoreCaseResult::Unknown => WireCaseResult::Unknown,
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
        unknown: totals.unknown,
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
    fn content_digest_uses_raw_bytes() {
        assert_eq!(decode_digest(&"ab".repeat(32)).unwrap(), vec![0xab; 32]);
        assert!(decode_digest("ab").is_none());
        assert!(decode_digest(&"zz".repeat(32)).is_none());
    }

    #[test]
    fn internal_phases_are_not_declared_actions_on_the_wire() {
        let translated = translate(
            CoreEvent::ActionCompleted {
                at_epoch_ms: 10,
                target_id: "tool".into(),
                capability: "_phase".into(),
                action_index: 0,
                identifier: Some("analysis".into()),
                result: CoreResult::Succeeded,
                was_cached: false,
                duration_ms: 5,
                exit_code: 0,
                start_at_epoch_ms: 5,
                worker_id: "worker-0".into(),
                prepare_ms: 0,
                execute_ms: 0,
                cache_key: String::new(),
                selected_attempt: 0,
            },
            0,
        );
        assert!(
            matches!(translated, Translated::Ordinary { payload: Payload::TargetPhaseCompleted(phase), .. } if phase.phase == "analysis")
        );
    }

    #[test]
    fn test_case_identity_is_unambiguous_when_names_contain_separators() {
        assert_ne!(
            test_case_execution_id("a#b", "c", 1),
            test_case_execution_id("a", "b#c", 1)
        );
    }

    #[test]
    fn host_sample_has_run_local_scope_and_measurement_interval() {
        let translated = translate(
            CoreEvent::SystemSampled {
                at_epoch_ms: 100,
                interval_ms: 750,
                cpu_percent: 25.0,
                memory_bytes: 1024,
                network_in_bytes_per_second: 4,
                network_out_bytes_per_second: 5,
            },
            0,
        );
        assert!(matches!(translated, Translated::Ordinary {
            payload: Payload::SystemSampled(sample), ..
        } if sample.scope == ResourceScope::Host as i32 && sample.resource_id == "reporting-host" && sample.interval_ms == 750));
    }

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
