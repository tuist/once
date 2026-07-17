use anyhow::{bail, ensure, Context, Result};
use serde_json::{Map, Value};

pub const TEST_RESULTS_SCHEMA: &str = "once.test_results.v1";

pub fn validate_test_results(value: &Value, expected_target: &str) -> Result<()> {
    let root = object(value, "normalized test results")?;
    exact_string(
        root,
        "schema",
        "normalized test results",
        TEST_RESULTS_SCHEMA,
    )?;
    exact_string(root, "target", "normalized test results", expected_target)?;
    string(root, "status", "normalized test results")?;

    let runner = nested_object(root, "runner", "normalized test results")?;
    string(runner, "type", "normalized test results runner")?;
    nested_object(runner, "metadata", "normalized test results runner")?;

    let summary = nested_object(root, "summary", "normalized test results")?;
    for field in ["total", "passed", "failed", "skipped", "flaky"] {
        unsigned_integer(summary, field, "normalized test results summary")?;
    }

    let cases = array(root, "cases", "normalized test results")?;
    for (index, case) in cases.iter().enumerate() {
        validate_case(case).with_context(|| format!("validating normalized test case {index}"))?;
    }

    let artifacts = nested_object(root, "artifacts", "normalized test results")?;
    string_array(artifacts, "logs", "normalized test result artifacts")?;
    string_array(
        artifacts,
        "native_results",
        "normalized test result artifacts",
    )?;
    if artifacts.contains_key("coverage") {
        string_array(artifacts, "coverage", "normalized test result artifacts")?;
    }
    Ok(())
}

fn validate_case(value: &Value) -> Result<()> {
    let case = object(value, "normalized test case")?;
    for field in ["id", "name", "suite", "status"] {
        string(case, field, "normalized test case")?;
    }
    if let Some(file) = case.get("file") {
        ensure!(
            file.is_string(),
            "normalized test case `file` must be a string"
        );
    }
    nested_object(case, "runner_metadata", "normalized test case")?;
    let attempts = array(case, "attempts", "normalized test case")?;
    ensure!(
        !attempts.is_empty(),
        "normalized test case `attempts` must contain at least one attempt"
    );
    for (index, attempt) in attempts.iter().enumerate() {
        let attempt = object(attempt, "normalized test attempt")?;
        string(attempt, "status", "normalized test attempt")
            .with_context(|| format!("validating normalized test attempt {index}"))?;
    }
    Ok(())
}

fn object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .with_context(|| format!("{context} must be an object"))
}

fn nested_object<'a>(
    object: &'a Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<&'a Map<String, Value>> {
    object
        .get(field)
        .with_context(|| format!("{context} is missing `{field}`"))?
        .as_object()
        .with_context(|| format!("{context} `{field}` must be an object"))
}

fn string<'a>(object: &'a Map<String, Value>, field: &str, context: &str) -> Result<&'a str> {
    let value = object
        .get(field)
        .with_context(|| format!("{context} is missing `{field}`"))?
        .as_str()
        .with_context(|| format!("{context} `{field}` must be a string"))?;
    ensure!(!value.is_empty(), "{context} `{field}` cannot be empty");
    Ok(value)
}

fn exact_string(
    object: &Map<String, Value>,
    field: &str,
    context: &str,
    expected: &str,
) -> Result<()> {
    let actual = string(object, field, context)?;
    ensure!(
        actual == expected,
        "{context} `{field}` must be `{expected}`, got `{actual}`"
    );
    Ok(())
}

fn unsigned_integer(object: &Map<String, Value>, field: &str, context: &str) -> Result<u64> {
    object
        .get(field)
        .with_context(|| format!("{context} is missing `{field}`"))?
        .as_u64()
        .with_context(|| format!("{context} `{field}` must be a non-negative integer"))
}

fn array<'a>(object: &'a Map<String, Value>, field: &str, context: &str) -> Result<&'a [Value]> {
    object
        .get(field)
        .with_context(|| format!("{context} is missing `{field}`"))?
        .as_array()
        .map(Vec::as_slice)
        .with_context(|| format!("{context} `{field}` must be an array"))
}

fn string_array(object: &Map<String, Value>, field: &str, context: &str) -> Result<()> {
    for (index, value) in array(object, field, context)?.iter().enumerate() {
        if !value.is_string() {
            bail!("{context} `{field}` item {index} must be a string");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
