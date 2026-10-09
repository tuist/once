use std::fmt::Write;
use std::path::Path;

use anyhow::{Context, Result};
use once_core::{TestPlan, TestSelectionPolicy};
use once_frontend::GraphTarget;

pub(crate) fn explicit_batch_plan(
    workspace: &Path,
    graph: &[GraphTarget],
    target: &str,
    batch_id: &str,
) -> Result<TestPlan> {
    let plan = super::explicit_plan(workspace, graph, &[target.to_string()])?;
    select_batch(plan, target, batch_id)
}

fn select_batch(mut plan: TestPlan, target: &str, batch_id: &str) -> Result<TestPlan> {
    let batch = plan
        .batches
        .iter()
        .find(|batch| batch.id == batch_id)
        .cloned();
    let batch = batch.with_context(|| {
        let mut message = format!("test batch `{batch_id}` is not in the current plan for `{target}`; inspect `once query test-plan --target {target} --format json` and use `once test {target} --test-batch <ID>`");
        for batch in plan.batches.iter().take(5) {
            let scope = if batch.test_filters.is_empty() {
                "whole target".to_string()
            } else {
                let preview = batch.test_filters.iter().take(2).map(String::as_str).collect::<Vec<_>>().join(", ");
                format!("{} units: {preview}", batch.test_filters.len())
            };
            let _ = write!(message, "\n  {} ({scope})", batch.id);
        }
        message
    })?;
    plan.selection.policy = TestSelectionPolicy {
        mode: "explicit".to_string(),
        safety: "exact".to_string(),
        evidence: "requested_test_batch".to_string(),
    };
    for test in &mut plan.selection.tests {
        test.reasons = vec![format!("explicitly requested test batch `{batch_id}`")];
    }
    Ok(TestPlan::new(plan.selection, vec![batch])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_core::{SelectedTest, TestBatch, TestSelectionReport, TEST_SELECTION_SCHEMA};

    fn plan() -> TestPlan {
        TestPlan::new(
            TestSelectionReport {
                schema: TEST_SELECTION_SCHEMA.to_string(),
                policy: TestSelectionPolicy {
                    mode: "explicit".to_string(),
                    safety: "exact".to_string(),
                    evidence: "requested_targets".to_string(),
                },
                changed_paths: vec![],
                unmatched_paths: vec![],
                tests: vec![SelectedTest {
                    id: "tests".to_string(),
                    kind: "test".to_string(),
                    reasons: vec![],
                }],
            },
            vec![
                TestBatch::new(
                    "tests",
                    vec!["tests::file/a".to_string(), "tests::file/b".to_string()],
                )
                .unwrap(),
                TestBatch::new("tests", vec!["tests::other".to_string()]).unwrap(),
            ],
        )
        .unwrap()
    }

    #[test]
    fn replay_preserves_the_entire_batch_scope_and_identity() {
        let original = plan();
        let batch = original
            .batches
            .iter()
            .find(|batch| batch.test_filters.len() == 2)
            .unwrap();
        let replay = select_batch(original.clone(), "tests", &batch.id).unwrap();
        assert_eq!(replay.batches, vec![batch.clone()]);
        assert_eq!(replay.selection.policy.evidence, "requested_test_batch");
        replay.validate().unwrap();
    }

    #[test]
    fn unknown_batch_suggests_current_ids_and_semantic_units() {
        let error = select_batch(plan(), "tests", "obsolete")
            .unwrap_err()
            .to_string();
        assert!(error.contains("once test tests --test-batch <ID>"));
        assert!(error.contains("tests::file/a"));
        assert!(error.contains(&plan().batches[0].id));
    }
}
