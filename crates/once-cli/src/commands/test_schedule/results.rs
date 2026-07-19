use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result};
use once_core::{TestBatch, TestPlan, TEST_RESULTS_SCHEMA};
use serde_json::{json, Value};

pub(super) fn persist(workspace: &Path, plan: &TestPlan, runs: &[Value]) -> Result<()> {
    let runs = runs
        .iter()
        .filter_map(|run| {
            run.get("batch_id")
                .and_then(Value::as_str)
                .map(|batch_id| (batch_id, run))
        })
        .collect::<BTreeMap<_, _>>();
    let mut by_target = BTreeMap::<&str, Vec<&TestBatch>>::new();
    for batch in &plan.batches {
        by_target.entry(&batch.target).or_default().push(batch);
    }
    for (target, batches) in by_target {
        let value = aggregate(target, &batches, &runs)?;
        let path = workspace
            .join(".once/out")
            .join(crate::commands::query::target_id_path(target)?)
            .join("test/test_results.json");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating `{}`", parent.display()))?;
        }
        std::fs::write(&path, serde_json::to_vec(&value)?)
            .with_context(|| format!("writing `{}`", path.display()))?;
    }
    Ok(())
}

fn aggregate(target: &str, batches: &[&TestBatch], runs: &BTreeMap<&str, &Value>) -> Result<Value> {
    let mut runner = None;
    let mut cases = BTreeMap::<String, Value>::new();
    let mut logs = BTreeSet::new();
    let mut native_results = BTreeSet::new();
    let mut coverage = BTreeSet::new();
    let mut all_passed = true;

    for batch in batches {
        let run = runs.get(batch.id.as_str()).copied();
        let success = run
            .and_then(|run| run.get("success"))
            .and_then(Value::as_bool)
            == Some(true);
        all_passed &= success;
        let result = run
            .and_then(|run| run.get("results"))
            .filter(|value| !value.is_null());
        let Some(result) = result else {
            // A whole-target batch may legitimately succeed without emitting
            // normalized results (the process layer keeps the subprocess status
            // authoritative). Record its outcome to match that verdict instead of
            // always synthesizing a failure.
            add_synthetic_case(target, batch, success, &mut cases);
            continue;
        };
        once_core::validate_test_results_for_units(result, target, &batch.test_filters)
            .with_context(|| format!("validating result for test batch `{}`", batch.id))?;
        all_passed &= result.get("status").and_then(Value::as_str) == Some("passed");
        if runner.is_none() {
            runner = result.get("runner").cloned();
        }
        if let Some(result_cases) = result.get("cases").and_then(Value::as_array) {
            for case in result_cases {
                if let Some(id) = case.get("id").and_then(Value::as_str) {
                    cases.insert(id.to_string(), case.clone());
                }
            }
        }
        collect_artifacts(result, "logs", &mut logs);
        collect_artifacts(result, "native_results", &mut native_results);
        collect_artifacts(result, "coverage", &mut coverage);
    }

    let mut passed = 0_u64;
    let mut failed = 0_u64;
    let mut skipped = 0_u64;
    let mut flaky = 0_u64;
    for case in cases.values() {
        match case.get("status").and_then(Value::as_str) {
            Some("passed") => passed += 1,
            Some("skipped" | "pending" | "todo" | "disabled") => skipped += 1,
            Some("flaky") => {
                passed += 1;
                flaky += 1;
            }
            _ => failed += 1,
        }
    }
    let cases = cases.into_values().collect::<Vec<_>>();
    let status = if all_passed && failed == 0 {
        "passed"
    } else {
        "failed"
    };
    let mut artifacts = json!({
        "logs": logs.into_iter().collect::<Vec<_>>(),
        "native_results": native_results.into_iter().collect::<Vec<_>>(),
    });
    if !coverage.is_empty() {
        artifacts["coverage"] = json!(coverage.into_iter().collect::<Vec<_>>());
    }
    let value = json!({
        "schema": TEST_RESULTS_SCHEMA,
        "target": target,
        "runner": runner.unwrap_or_else(|| json!({"type": "unknown", "metadata": {}})),
        "status": status,
        "summary": {
            "total": cases.len(),
            "passed": passed,
            "failed": failed,
            "skipped": skipped,
            "flaky": flaky,
        },
        "cases": cases,
        "artifacts": artifacts,
    });
    once_core::validate_test_results(&value, target)?;
    Ok(value)
}

fn add_synthetic_case(
    target: &str,
    batch: &TestBatch,
    succeeded: bool,
    cases: &mut BTreeMap<String, Value>,
) {
    let (status, name) = if succeeded {
        ("passed", "test batch passed without normalized results")
    } else {
        ("failed", "test batch did not produce normalized results")
    };
    let filters = if batch.test_filters.is_empty() {
        vec![format!("{target}::batch:{}", batch.id)]
    } else {
        batch.test_filters.clone()
    };
    for filter in filters {
        cases.entry(filter.clone()).or_insert_with(|| {
            json!({
                "id": filter,
                "name": name,
                "suite": target,
                "status": status,
                "attempts": [{"status": status}],
                "runner_metadata": {},
            })
        });
    }
}

fn collect_artifacts(result: &Value, field: &str, out: &mut BTreeSet<String>) {
    if let Some(values) = result
        .pointer(&format!("/artifacts/{field}"))
        .and_then(Value::as_array)
    {
        out.extend(values.iter().filter_map(Value::as_str).map(str::to_string));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregation_merges_isolated_batch_results() {
        let first = TestBatch::new("tests/unit", vec!["tests/unit::a".to_string()]).unwrap();
        let second = TestBatch::new("tests/unit", vec!["tests/unit::b".to_string()]).unwrap();
        let values = BTreeMap::from([
            (first.id.as_str(), run(&first, "a")),
            (second.id.as_str(), run(&second, "b")),
        ]);
        let runs = values
            .iter()
            .map(|(id, value)| (*id, value))
            .collect::<BTreeMap<_, _>>();

        let value = aggregate("tests/unit", &[&first, &second], &runs).unwrap();

        assert_eq!(value["status"], "passed");
        assert_eq!(value["summary"]["total"], 2);
        assert_eq!(value["cases"][0]["id"], "tests/unit::a");
        assert_eq!(value["cases"][1]["id"], "tests/unit::b");
    }

    #[test]
    fn whole_target_batch_without_results_records_the_subprocess_verdict() {
        let batch = TestBatch::new("tests/unit", Vec::new()).unwrap();
        let values = BTreeMap::from([(
            batch.id.as_str(),
            json!({"batch_id": batch.id, "success": true, "results": Value::Null}),
        )]);
        let runs = values
            .iter()
            .map(|(id, value)| (*id, value))
            .collect::<BTreeMap<_, _>>();

        let value = aggregate("tests/unit", &[&batch], &runs).unwrap();

        assert_eq!(value["status"], "passed");
        assert_eq!(value["summary"]["passed"], 1);
        assert_eq!(value["summary"]["failed"], 0);
    }

    fn run(batch: &TestBatch, name: &str) -> Value {
        json!({
            "batch_id": batch.id,
            "success": true,
            "results": {
                "schema": TEST_RESULTS_SCHEMA,
                "target": "tests/unit",
                "runner": {"type": "demo", "metadata": {}},
                "status": "passed",
                "summary": {"total": 1, "passed": 1, "failed": 0, "skipped": 0, "flaky": 0},
                "cases": [{
                    "id": format!("tests/unit::{name}"),
                    "name": name,
                    "suite": "tests/unit",
                    "status": "passed",
                    "attempts": [{"status": "passed"}],
                    "runner_metadata": {},
                }],
                "artifacts": {"logs": [], "native_results": []},
            }
        })
    }
}
