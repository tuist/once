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
fn requires_every_requested_test_unit() {
    let results = valid_results();
    validate_test_results_for_units(
        &results,
        "tests/example",
        &["tests/example::case-name".to_string()],
    )
    .unwrap();

    let error = validate_test_results_for_units(
        &results,
        "tests/example",
        &["tests/example::missing".to_string()],
    )
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("missing requested test unit `tests/example::missing`"));
}

#[test]
fn rejects_duplicate_cases_even_when_the_last_observation_passes() {
    let mut results = valid_results();
    let passing = results["cases"][0].clone();
    results["cases"][0]["status"] = json!("failed");
    results["cases"].as_array_mut().unwrap().push(passing);

    let error = validate_test_results(&results, "tests/example").unwrap_err();
    assert!(error.to_string().contains("duplicate normalized test case"));
}

#[test]
fn exact_selection_rejects_a_runner_that_ignores_its_filters() {
    let mut results = valid_results();
    let mut extra = results["cases"][0].clone();
    extra["id"] = json!("tests/example::unrequested");
    results["cases"].as_array_mut().unwrap().push(extra);
    results["summary"]["total"] = json!(2);
    results["summary"]["passed"] = json!(2);
    validate_test_results_for_units(&results, "tests/example", &[]).unwrap();

    let error = validate_test_results_for_units(
        &results,
        "tests/example",
        &["tests/example::case-name".to_string()],
    )
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("unrequested test unit `tests/example::unrequested`"));
}

#[test]
fn exact_selection_rejects_empty_results_and_inconsistent_totals() {
    let mut results = valid_results();
    results["cases"] = json!([]);
    assert!(validate_test_results_for_units(
        &results,
        "tests/example",
        &["tests/example::case-name".to_string()],
    )
    .unwrap_err()
    .to_string()
    .contains("missing requested test unit"));

    let mut results = valid_results();
    results["summary"]["total"] = json!(2);
    assert!(validate_test_results_for_units(
        &results,
        "tests/example",
        &["tests/example::case-name".to_string()],
    )
    .unwrap_err()
    .to_string()
    .contains("summary total"));
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
