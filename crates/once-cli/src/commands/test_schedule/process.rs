use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use once_core::{SandboxMode, TestBatch, TestBatchStatus};
use serde_json::{json, Value};

pub(super) fn run_test_target(
    executable: &Path,
    workspace: &Path,
    batch: &TestBatch,
    sandbox: SandboxMode,
) -> Result<Value> {
    let sandbox = match sandbox {
        SandboxMode::Off => "off",
        SandboxMode::Inputs | SandboxMode::Validate => "inputs",
    };
    let mut command = Command::new(executable);
    command
        .arg("-C")
        .arg(workspace)
        .arg("--format")
        .arg("json")
        .arg("test")
        .arg("--sandbox")
        .arg(sandbox)
        .arg(&batch.target);
    for test_filter in &batch.test_filters {
        command.arg("--batch-test-unit").arg(test_filter);
    }
    if !batch.test_filters.is_empty() {
        command.arg("--test-batch-id").arg(&batch.id);
    }
    let output = command
        .output()
        .with_context(|| format!("running `{}` test `{}`", executable.display(), batch.target))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let (record, record_parse_error) = parse_json_record(&stdout);
    let fallback_results_path = if batch.test_filters.is_empty() {
        None
    } else {
        Some(
            std::path::PathBuf::from(".once/out")
                .join(crate::commands::query::target_id_path(&batch.target)?)
                .join("test/batches")
                .join(&batch.id)
                .join("test_results.json")
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/"),
        )
    };
    let results_path = record
        .get("test_results")
        .and_then(Value::as_str)
        .or(fallback_results_path.as_deref());
    let (results, results_error) = match crate::commands::query::test_results_value_at(
        workspace,
        &batch.target,
        results_path,
        &batch.test_filters,
    ) {
        Ok(results) => (Some(results), None),
        Err(error) => (None, Some(format!("{error:#}"))),
    };
    // Whole-target execution keeps the subprocess status authoritative because
    // normalized results are optional. Exact batches also require normalized
    // evidence that every requested unit ran.
    let success = batch_succeeded(output.status.success(), batch, results_error.as_deref());
    Ok(json!({
        "batch_id": batch.id,
        "target": batch.target,
        "exit_code": output.status.code().unwrap_or(-1),
        "success": success,
        "record": record,
        "record_parse_error": record_parse_error,
        "results": results,
        "results_error": results_error,
        "stderr": stderr,
    }))
}

fn batch_succeeded(
    process_succeeded: bool,
    batch: &TestBatch,
    results_error: Option<&str>,
) -> bool {
    process_succeeded && (batch.test_filters.is_empty() || results_error.is_none())
}

pub(super) fn classify_run(
    batch: &TestBatch,
    result: Result<Value>,
) -> (Value, TestBatchStatus, Option<i32>, Option<String>) {
    match result {
        Ok(run) => {
            let success = run.get("success").and_then(Value::as_bool) == Some(true);
            let status = if success {
                TestBatchStatus::Passed
            } else {
                TestBatchStatus::Failed
            };
            let exit_code = run
                .get("exit_code")
                .and_then(Value::as_i64)
                .and_then(|code| i32::try_from(code).ok());
            let cache = run
                .pointer("/record/cache")
                .and_then(Value::as_str)
                .map(str::to_string);
            (run, status, exit_code, cache)
        }
        Err(error) => (
            json!({
                "batch_id": batch.id,
                "target": batch.target,
                "exit_code": -1,
                "success": false,
                "error": error.to_string(),
            }),
            TestBatchStatus::Error,
            None,
            None,
        ),
    }
}

fn parse_json_record(stdout: &str) -> (Value, Option<String>) {
    if stdout.is_empty() {
        return (Value::Null, None);
    }
    match serde_json::from_str(stdout) {
        Ok(value) => (value, None),
        Err(error) => (Value::String(stdout.to_string()), Some(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_batch_requires_valid_results() {
        let exact = TestBatch::new("tests", vec!["tests::requested".to_string()]).unwrap();
        let whole_target = TestBatch::new("tests", Vec::new()).unwrap();

        assert!(!batch_succeeded(
            true,
            &exact,
            Some("missing requested unit")
        ));
        assert!(batch_succeeded(true, &exact, None));
        assert!(batch_succeeded(
            true,
            &whole_target,
            Some("optional normalized results are unavailable")
        ));
    }
}
