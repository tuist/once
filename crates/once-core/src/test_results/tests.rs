use serde_json::json;

use super::*;

fn valid_results() -> Value {
    json!({
        "schema": TEST_RESULTS_SCHEMA,
        "target": "tests/example",
        "runner": { "type": "native", "metadata": {} },
        "status": "passed",
        "summary": {
            "total": 1,
            "passed": 1,
            "failed": 0,
            "skipped": 0,
            "flaky": 0
        },
        "cases": [{
            "id": "tests/example::case-name",
            "name": "case-name",
            "suite": "example-suite",
            "status": "passed",
            "attempts": [{ "status": "passed" }],
            "runner_metadata": {}
        }],
        "artifacts": { "logs": ["test.log"], "native_results": [] }
    })
}

#[test]
fn accepts_the_generic_normalized_shape() {
    validate_test_results(&valid_results(), "tests/example").unwrap();
}

#[test]
fn rejects_a_result_for_another_target() {
    let error = validate_test_results(&valid_results(), "tests/other").unwrap_err();
    assert!(error.to_string().contains("must be `tests/other`"));
}

#[test]
fn rejects_runner_shorthand() {
    let mut results = valid_results();
    results["runner"] = json!("native");

    let error = validate_test_results(&results, "tests/example").unwrap_err();
    assert!(error.to_string().contains("`runner` must be an object"));
}

#[test]
fn rejects_numeric_attempt_shorthand() {
    let mut results = valid_results();
    results["cases"][0]["attempts"] = json!(1);

    let error = validate_test_results(&results, "tests/example").unwrap_err();
    assert!(format!("{error:#}").contains("`attempts` must be an array"));
}

#[test]
fn rejects_incomplete_summary_and_artifacts() {
    let mut results = valid_results();
    results["summary"].as_object_mut().unwrap().remove("flaky");
    let error = validate_test_results(&results, "tests/example").unwrap_err();
    assert!(error.to_string().contains("missing `flaky`"));

    let mut results = valid_results();
    results["artifacts"] = json!([]);
    let error = validate_test_results(&results, "tests/example").unwrap_err();
    assert!(error.to_string().contains("`artifacts` must be an object"));
}
