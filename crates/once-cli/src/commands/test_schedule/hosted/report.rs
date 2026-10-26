use std::collections::{BTreeMap, BTreeSet};

use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};

use crate::commands::query::test_plan::portable::PortableTestPlan;

#[allow(clippy::too_many_lines)]
pub(super) fn project(
    value: &Value,
    plan: &PortableTestPlan,
    assigned: &[String],
) -> Result<(Vec<Value>, bool)> {
    let selected = assigned.iter().collect::<BTreeSet<_>>();
    let reported: Vec<String> = serde_json::from_value(value["selected_batch_ids"].clone())?;
    let runs = value["execution"]["runs"]
        .as_array()
        .context("missing batch executions")?;
    ensure!(
        value["schema"] == "once.portable_test_execution.v1"
            && value["plan_id"] == plan.id
            && value["complete"] == true
            && selected.len() == assigned.len()
            && reported.len() == assigned.len()
            && reported.iter().collect::<BTreeSet<_>>() == selected
            && runs.len() == assigned.len(),
        "Once did not complete exactly the assigned batches"
    );
    let mut observed_batches = BTreeSet::new();
    let mut observed_cases = BTreeSet::new();
    let mut by_target: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut success = value["success"] == true;
    for run in runs {
        let id = run["batch_id"].as_str().context("missing batch ID")?;
        ensure!(
            observed_batches.insert(id) && assigned.iter().any(|entry| entry == id),
            "unexpected or duplicate test batch"
        );
        let batch = plan
            .plan
            .batches
            .iter()
            .find(|batch| batch.id == id)
            .context("unknown assigned batch")?;
        ensure!(
            run["target"] == batch.target,
            "batch returned a different target"
        );
        success &= run["success"] == true && run["exit_code"] == 0;
        let results = &run["results"];
        if results.is_null() {
            ensure!(
                batch.test_filters.is_empty(),
                "exact batch has no normalized evidence"
            );
            continue;
        }
        once_core::validate_test_results_for_units(results, &batch.target, &batch.test_filters)?;
        for case in results["cases"].as_array().context("missing cases")? {
            let id = case["id"].as_str().context("missing native case ID")?;
            ensure!(
                observed_cases.insert((batch.target.as_str(), id)),
                "duplicate native case across batches"
            );
            let attempts = case["attempts"]
                .as_array()
                .context("missing case attempts")?;
            let repetitions: Vec<Value> = attempts
                .iter()
                .enumerate()
                .map(|(index, attempt)| {
                    json!({
                        "repetition_number": index + 1, "name": format!("Attempt {}", index + 1),
                        "status": status(attempt["status"].as_str().unwrap_or_default()),
                        "duration": milliseconds(&attempt["duration_ms"]),
                    })
                })
                .collect();
            let failures: Vec<Value> = attempts
                .iter()
                .filter_map(|attempt| {
                    attempt["failure"]["message"]
                        .as_str()
                        .map(|message| json!({"message": message, "line_number": 0}))
                })
                .collect();
            let native_status = case["status"].as_str().context("missing case status")?;
            let final_status = if native_status == "flaky" {
                status(
                    attempts.last().context("empty attempts")?["status"]
                        .as_str()
                        .unwrap_or_default(),
                )
            } else {
                status(native_status)
            };
            success &= final_status != "failure";
            by_target.entry(batch.target.clone()).or_default().push(json!({
                "name": case["name"], "test_suite_name": case["suite"], "status": final_status,
                "duration": duration(&repetitions), "repetitions": repetitions, "failures": failures,
            }));
        }
    }
    let modules = by_target.into_iter().map(|(target, mut cases)| {
        cases.sort_by(|a, b| (a["test_suite_name"].as_str(), a["name"].as_str()).cmp(&(b["test_suite_name"].as_str(), b["name"].as_str())));
        let mut by_suite: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
        for case in &cases {
            by_suite.entry(case["test_suite_name"].as_str().unwrap_or_default()).or_default().push(case.clone());
        }
        let suites: Vec<Value> = by_suite.into_iter().map(|(name, members)| json!({
            "name": name, "status": aggregate(&members), "duration": duration(&members),
        })).collect();
        json!({"name": target, "status": if aggregate(&cases) == "failure" { "failure" } else { "success" },
            "duration": duration(&cases), "test_cases": cases, "test_suites": suites})
    }).collect();
    Ok((modules, success))
}

fn status(value: &str) -> &'static str {
    match value {
        "passed" => "success",
        "skipped" | "pending" | "todo" | "disabled" => "skipped",
        _ => "failure",
    }
}

fn aggregate(cases: &[Value]) -> &'static str {
    if cases.iter().any(|case| case["status"] == "failure") {
        "failure"
    } else if cases.iter().all(|case| case["status"] == "skipped") {
        "skipped"
    } else {
        "success"
    }
}

fn milliseconds(value: &Value) -> u64 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    value
        .as_f64()
        .filter(|value| value.is_finite() && *value > 0.0)
        .map_or(0, |value| value.round().min(f64::from(i32::MAX)) as u64)
}

pub(super) fn duration(values: &[Value]) -> u64 {
    values
        .iter()
        .fold(0_u64, |sum, value| {
            sum.saturating_add(value["duration"].as_u64().unwrap_or(0))
        })
        .min(i32::MAX as u64)
}
