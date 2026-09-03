//! Parser for `cargo test -- -Z unstable-options --format=json`
//! output that emits [`once_core::RunEvent`] test-case events into a
//! callback. Serves as the proof-of-concept per-framework parser for
//! Once's per-test-case fire-points; other frameworks follow the
//! same shape.
//!
//! Cargo's JSON stream is one object per line, of two kinds:
//!
//! - `{ "type": "suite", "event": "started", "test_count": N }`
//! - `{ "type": "suite", "event": "ok" | "failed", "passed": ..., ... }`
//! - `{ "type": "test",  "event": "started", "name": "…" }`
//! - `{ "type": "test",  "event": "ok" | "failed" | "ignored", "name": "…", "exec_time": 0.001, "stdout": "…" }`

use std::time::{SystemTime, UNIX_EPOCH};

use once_core::{RunEvent, TestCaseResult, TestTotals};
use serde::Deserialize;

/// Parse one line of cargo test JSON output and emit any events it
/// generates into `emit`. Unknown or malformed lines are silently
/// skipped; the parser is tolerant on purpose because the format is
/// unstable and the CLI must not fail the run for a broken line.
pub fn parse_line(target_id: &str, line: &str, emit: &mut impl FnMut(RunEvent)) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let Ok(record) = serde_json::from_str::<RawRecord>(line) else {
        return;
    };
    let at = now_epoch_ms();
    match (record.record_type.as_deref(), record.event.as_deref()) {
        (Some("suite"), Some("started")) => {
            if let Some(count) = record.test_count {
                emit(RunEvent::TestSuiteStarted {
                    at_epoch_ms: at,
                    target_id: target_id.to_string(),
                    planned_case_count: Some(count),
                });
            }
        }
        (Some("suite"), Some("ok" | "failed")) => {
            emit(RunEvent::TestSuiteCompleted {
                at_epoch_ms: at,
                target_id: target_id.to_string(),
                totals: TestTotals {
                    passed: record.passed.unwrap_or(0),
                    failed: record.failed.unwrap_or(0),
                    skipped: record.ignored.unwrap_or(0),
                    errored: 0,
                    timed_out: 0,
                    cancelled: 0,
                },
            });
        }
        (Some("test"), Some("started")) => {
            if let Some(name) = record.name {
                emit(RunEvent::TestCaseStarted {
                    at_epoch_ms: at,
                    target_id: target_id.to_string(),
                    case_id: name.clone(),
                    name,
                    attempt: 1,
                });
            }
        }
        (Some("test"), Some("ok")) => {
            if let Some(name) = record.name {
                emit(RunEvent::TestCaseCompleted {
                    at_epoch_ms: at,
                    target_id: target_id.to_string(),
                    case_id: name,
                    result: TestCaseResult::Passed,
                    duration_ms: seconds_to_millis(record.exec_time),
                    failure_message: None,
                });
            }
        }
        (Some("test"), Some("failed")) => {
            if let Some(name) = record.name {
                emit(RunEvent::TestCaseCompleted {
                    at_epoch_ms: at,
                    target_id: target_id.to_string(),
                    case_id: name,
                    result: TestCaseResult::Failed,
                    duration_ms: seconds_to_millis(record.exec_time),
                    failure_message: record.stdout,
                });
            }
        }
        (Some("test"), Some("ignored")) => {
            if let Some(name) = record.name {
                emit(RunEvent::TestCaseCompleted {
                    at_epoch_ms: at,
                    target_id: target_id.to_string(),
                    case_id: name,
                    result: TestCaseResult::Skipped,
                    duration_ms: 0,
                    failure_message: None,
                });
            }
        }
        _ => {}
    }
}

/// Feed a stream of newline-delimited cargo test JSON output into
/// `emit`. Intended for use with tokio's line reader wrapper.
pub fn parse_stream(target_id: &str, text: &str, emit: &mut impl FnMut(RunEvent)) {
    for line in text.lines() {
        parse_line(target_id, line, emit);
    }
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn seconds_to_millis(secs: Option<f64>) -> i64 {
    match secs {
        Some(s) if s.is_finite() && s >= 0.0 => (s * 1000.0) as i64,
        _ => 0,
    }
}

#[derive(Deserialize)]
struct RawRecord {
    #[serde(rename = "type", default)]
    record_type: Option<String>,
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    test_count: Option<u32>,
    #[serde(default)]
    passed: Option<u32>,
    #[serde(default)]
    failed: Option<u32>,
    #[serde(default)]
    ignored: Option<u32>,
    #[serde(default)]
    exec_time: Option<f64>,
    #[serde(default)]
    stdout: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(input: &str) -> Vec<RunEvent> {
        let mut out = Vec::new();
        parse_stream("//foo:bar", input, &mut |evt| out.push(evt));
        out
    }

    #[test]
    fn parses_a_passing_run() {
        let input = concat!(
            r#"{"type":"suite","event":"started","test_count":1}"#,
            "\n",
            r#"{"type":"test","event":"started","name":"m::t"}"#,
            "\n",
            r#"{"type":"test","event":"ok","name":"m::t","exec_time":0.001}"#,
            "\n",
            r#"{"type":"suite","event":"ok","passed":1,"failed":0,"ignored":0}"#,
            "\n",
        );
        let events = collect(input);
        assert!(matches!(
            events[0],
            RunEvent::TestSuiteStarted {
                planned_case_count: Some(1),
                ..
            }
        ));
        assert!(matches!(events[1], RunEvent::TestCaseStarted { .. }));
        match &events[2] {
            RunEvent::TestCaseCompleted {
                result: TestCaseResult::Passed,
                duration_ms,
                ..
            } => assert_eq!(*duration_ms, 1),
            other => panic!("unexpected: {other:?}"),
        }
        assert!(matches!(
            events[3],
            RunEvent::TestSuiteCompleted {
                totals: TestTotals {
                    passed: 1,
                    failed: 0,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn captures_failure_message_on_failed_case() {
        let input = r#"{"type":"test","event":"failed","name":"m::t","stdout":"boom","exec_time":0.5}"#;
        let events = collect(input);
        match &events[0] {
            RunEvent::TestCaseCompleted {
                result: TestCaseResult::Failed,
                failure_message: Some(msg),
                duration_ms,
                ..
            } => {
                assert_eq!(msg, "boom");
                assert_eq!(*duration_ms, 500);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn ignored_case_becomes_skipped() {
        let input = r#"{"type":"test","event":"ignored","name":"m::t"}"#;
        let events = collect(input);
        assert!(matches!(
            events[0],
            RunEvent::TestCaseCompleted {
                result: TestCaseResult::Skipped,
                ..
            }
        ));
    }

    #[test]
    fn malformed_lines_are_silently_skipped() {
        let events = collect("not json\n{\"partial\":true}\n\n");
        assert!(events.is_empty());
    }

    #[test]
    fn failed_suite_summary_is_still_a_completed_event() {
        let input =
            r#"{"type":"suite","event":"failed","passed":3,"failed":2,"ignored":1}"#;
        let events = collect(input);
        match &events[0] {
            RunEvent::TestSuiteCompleted { totals, .. } => {
                assert_eq!(totals.passed, 3);
                assert_eq!(totals.failed, 2);
                assert_eq!(totals.skipped, 1);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
