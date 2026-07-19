use std::collections::BTreeMap;
use std::sync::Mutex;

use once_core::{
    SelectedTest, TestBatchStatus, TestPlan, TestSelectionPolicy, TestSelectionReport,
    TEST_SELECTION_SCHEMA,
};
use serde_json::json;

use super::*;

#[test]
fn historical_duration_orders_longest_batch_first() {
    let plan = plan(&["fast", "slow"]);
    let slow = plan
        .batches
        .iter()
        .find(|batch| batch.target == "slow")
        .unwrap();
    let seen = Mutex::new(Vec::new());
    let completed = execute_with(
        &plan,
        &BTreeMap::from([(slow.id.clone(), 200)]),
        Some(1),
        |batch| {
            seen.lock().unwrap().push(batch.target.clone());
            Ok(json!({
                "target": batch.target,
                "exit_code": 0,
                "success": true,
                "record": { "cache": "miss" }
            }))
        },
    )
    .unwrap();

    assert_eq!(seen.into_inner().unwrap(), vec!["slow", "fast"]);
    assert_eq!(completed.schedule.workers, 1);
    assert_eq!(completed.runs.len(), 2);
    assert!(completed
        .schedule
        .attempts
        .iter()
        .all(|attempt| attempt.status == TestBatchStatus::Passed));
}

#[test]
fn workers_are_capped_by_batch_count() {
    let plan = plan(&["only"]);
    let completed = execute_with(&plan, &BTreeMap::new(), Some(20), |_| {
        Ok(json!({ "exit_code": 0, "success": true }))
    })
    .unwrap();

    assert_eq!(completed.schedule.workers, 1);
}

fn plan(targets: &[&str]) -> TestPlan {
    TestPlan::for_selected_targets(TestSelectionReport {
        schema: TEST_SELECTION_SCHEMA.to_string(),
        policy: TestSelectionPolicy {
            mode: "explicit".to_string(),
            safety: "exact".to_string(),
            evidence: "requested_targets".to_string(),
        },
        changed_paths: Vec::new(),
        unmatched_paths: Vec::new(),
        tests: targets
            .iter()
            .map(|target| SelectedTest {
                id: (*target).to_string(),
                kind: "test".to_string(),
                reasons: vec!["selected".to_string()],
            })
            .collect(),
    })
    .unwrap()
}
