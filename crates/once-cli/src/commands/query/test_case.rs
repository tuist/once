use std::io::{Read, Write};
use std::path::Path;

use anyhow::Result;
use serde::Serialize;
use serde_json::{json, Value};

const MAX_RESULTS_BYTES: usize = 16 * 1024 * 1024;
const MAX_PROJECTION_BYTES: usize = 1024 * 1024;

pub(crate) fn diagnostic(target: &str, code: &str, attribute: &str, repair: &str) -> anyhow::Error {
    once_frontend::analysis::AnalysisFailure {
        diagnostic: once_frontend::Diagnostic::new(code, repair)
            .with_target(bounded_text(target))
            .with_attribute(attribute)
            .with_repair(repair),
    }
    .into()
}

fn bounded_text(value: &str) -> String {
    let mut text: String = value.chars().take(128).collect();
    if value.chars().nth(128).is_some() {
        text.push('…');
    }
    text
}

pub(crate) fn validate_selection(
    target: &str,
    summary_only: bool,
    case: Option<&str>,
    attempt: Option<usize>,
) -> Result<()> {
    let invalid = if summary_only && case.is_some() {
        Some((
            "case",
            "Use either case selection or summary_only, not both.",
        ))
    } else if attempt.is_some() && case.is_none() {
        Some((
            "attempt",
            "Select a case before selecting its producer attempt.",
        ))
    } else if attempt == Some(0) {
        Some((
            "attempt",
            "Use a one-based producer attempt position, starting at 1.",
        ))
    } else if case == Some("") {
        Some(("case", "Use a nonempty exact retained case id."))
    } else {
        None
    };
    if let Some((attribute, repair)) = invalid {
        return Err(diagnostic(
            target,
            "invalid_test_result_selection",
            attribute,
            repair,
        ));
    }
    Ok(())
}

#[derive(Serialize)]
struct CaseResult<'a> {
    schema: &'static str,
    target: &'a str,
    results_digest: String,
    status: &'a Value,
    case: std::collections::BTreeMap<&'static str, &'a Value>,
    attempt: usize,
    attempt_count: usize,
    attempt_result: &'a Value,
    target_artifacts: &'a Value,
    artifact_availability: &'static str,
    rerun: Rerun<'a>,
}

#[derive(Serialize)]
struct Rerun<'a> {
    argv: [&'a str; 6],
    scope: &'static str,
    configuration: &'static str,
    reason: &'static str,
}

pub(crate) fn test_case_value(
    workspace: &Path,
    target: &str,
    case_id: &str,
    attempt: Option<usize>,
) -> Result<Value> {
    validate_selection(target, false, Some(case_id), attempt)?;
    let invalid =
        |code: &str, attribute: &str, repair: &str| diagnostic(target, code, attribute, repair);
    let path = workspace.join(super::test_results_path(target).map_err(|_| {
        invalid(
            "invalid_test_result_target",
            "target",
            "Use a workspace-relative canonical target id without traversal components.",
        )
    })?);
    let mut raw = Vec::new();
    std::fs::File::open(&path)
        .and_then(|file| {
            file.take((MAX_RESULTS_BYTES + 1) as u64)
                .read_to_end(&mut raw)
        })
        .map_err(|_| {
            invalid(
                "test_results_unavailable",
                "results",
                "Retain a readable normalized result file for this target before querying it.",
            )
        })?;
    if raw.len() > MAX_RESULTS_BYTES {
        return Err(invalid("test_results_too_large", "results", "Focused reads accept at most 16777216 bytes of retained JSON. Inspect the file locally instead."));
    }
    let results: Value = serde_json::from_slice(&raw).map_err(|_| {
        invalid(
            "invalid_test_results",
            "results",
            "Retain valid normalized JSON with nesting within the JSON parser recursion limit.",
        )
    })?;
    once_core::validate_test_results(&results, target).map_err(|_| {
        invalid("invalid_test_results", "results", "Retain a valid once.test_results.v1 record matching this target, with unique case ids and nonempty producer attempts.")
    })?;
    let cases = results["cases"].as_array().expect("validated cases");
    let case = cases.iter().find(|case| case["id"].as_str() == Some(case_id)).ok_or_else(|| {
        let nearby: Vec<_> = cases.iter().take(5).map(|case| bounded_text(case["id"].as_str().expect("validated id"))).collect();
        invalid("test_case_not_found", "case", &format!("Use an exact retained case id. First retained ids (at most 5, each truncated to 128 characters): {}", json!(nearby)))
    })?;
    let attempts = case["attempts"].as_array().expect("validated attempts");
    let number = attempt.unwrap_or(attempts.len());
    let selected = number
        .checked_sub(1)
        .and_then(|index| attempts.get(index))
        .ok_or_else(|| {
            invalid(
                "test_attempt_not_found",
                "attempt",
                &format!(
                    "Use a one-based producer attempt position in 1..={} for this retained case.",
                    attempts.len()
                ),
            )
        })?;
    // Borrow producer-owned payloads until their serialized size has been checked.
    let projection = CaseResult {
        schema: "once.test_case_result.v1",
        target,
        results_digest: once_cas::Digest::of_bytes(&raw).to_hex(),
        status: &results["status"],
        case: ["id", "name", "suite", "status"]
            .into_iter()
            .map(|key| (key, &case[key]))
            .collect(),
        attempt: number,
        attempt_count: attempts.len(),
        attempt_result: selected,
        target_artifacts: &results["artifacts"],
        artifact_availability: "not_checked",
        rerun: Rerun {
            argv: ["once", "test", "--format", "json", "--", target],
            scope: "target",
            configuration: "current",
            reason: "retained_evidence_only",
        },
    };
    check_projection_size(&projection).map_err(|_| invalid("test_case_projection_too_large", "case", "Focused projections accept at most 1048576 compact JSON bytes. Inspect the retained file locally instead."))?;
    Ok(serde_json::to_value(projection)?)
}

fn check_projection_size(value: &impl Serialize) -> serde_json::Result<()> {
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.0 {
                return Err(std::io::Error::other("projection limit exceeded"));
            }
            self.0 -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter(MAX_PROJECTION_BYTES), value)
}

#[cfg(test)]
mod tests;
