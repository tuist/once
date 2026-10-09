use std::collections::{BTreeMap, BTreeSet};

use anyhow::{ensure, Result};

use super::{TestBatch, TestPlan, TEST_PLAN_SCHEMA, TEST_SELECTION_SCHEMA};

pub(super) fn validate(plan: &TestPlan) -> Result<()> {
    ensure!(
        plan.schema == TEST_PLAN_SCHEMA,
        "unsupported test plan schema"
    );
    ensure!(
        plan.selection.schema == TEST_SELECTION_SCHEMA,
        "unsupported test selection schema"
    );
    let canonical = TestPlan::new(plan.selection.clone(), plan.batches.clone())?;
    ensure!(
        *plan == canonical,
        "test plan identity or ordering is not canonical"
    );
    let mut targets = BTreeSet::new();
    for test in &plan.selection.tests {
        ensure!(!test.id.is_empty(), "selected test target cannot be empty");
        ensure!(
            targets.insert(test.id.as_str()),
            "duplicate selected test target `{}`",
            test.id
        );
    }
    let mut ids = BTreeSet::new();
    let mut scopes = BTreeMap::<&str, (bool, BTreeSet<&str>)>::new();
    for batch in &plan.batches {
        ensure!(
            targets.contains(batch.target.as_str()),
            "test batch target `{}` was not selected",
            batch.target
        );
        ensure!(
            *batch == TestBatch::new(&batch.target, batch.test_filters.clone())?,
            "test batch `{}` identity or filters are not canonical",
            batch.id
        );
        ensure!(
            ids.insert(batch.id.as_str()),
            "duplicate test batch `{}`",
            batch.id
        );
        let (whole, units) = scopes.entry(&batch.target).or_default();
        if batch.test_filters.is_empty() {
            ensure!(
                !*whole && units.is_empty(),
                "whole-target and exact test batches overlap for `{}`",
                batch.target
            );
            *whole = true;
        } else {
            ensure!(
                !*whole,
                "whole-target and exact test batches overlap for `{}`",
                batch.target
            );
            for unit in &batch.test_filters {
                ensure!(!unit.is_empty(), "test unit cannot be empty");
                ensure!(
                    units.insert(unit),
                    "test batches overlap on unit `{unit}` for `{}`",
                    batch.target
                );
            }
        }
    }
    ensure!(
        targets.len() == scopes.len(),
        "selected test target has no batch"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SelectedTest, TestSelectionPolicy, TestSelectionReport};

    fn plan(filters: &[&[&str]]) -> TestPlan {
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
            filters
                .iter()
                .map(|units| {
                    TestBatch::new("tests", units.iter().map(|s| (*s).to_string()).collect())
                        .unwrap()
                })
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn accepts_disjoint_exact_scopes_and_whole_target_fallbacks() {
        plan(&[&["a"], &["b", "c"]]).validate().unwrap();
        plan(&[&[]]).validate().unwrap();
    }

    #[test]
    fn rejects_duplicate_batches_and_overlapping_units() {
        assert!(plan(&[&["a"], &["a"]])
            .validate()
            .unwrap_err()
            .to_string()
            .contains("duplicate test batch"));
        assert!(plan(&[&["a", "b"], &["b", "c"]])
            .validate()
            .unwrap_err()
            .to_string()
            .contains("overlap on unit"));
    }

    #[test]
    fn rejects_whole_target_overlap_regardless_of_sort_order() {
        assert!(plan(&[&[], &["a"]])
            .validate()
            .unwrap_err()
            .to_string()
            .contains("overlap"));
    }

    #[test]
    fn rejects_forged_identity_and_missing_selected_work() {
        let mut forged = plan(&[&["a"]]);
        forged.batches[0].id = "forged".to_string();
        forged = TestPlan::new(forged.selection, forged.batches).unwrap();
        assert!(forged
            .validate()
            .unwrap_err()
            .to_string()
            .contains("test batch"));
        assert!(plan(&[])
            .validate()
            .unwrap_err()
            .to_string()
            .contains("no batch"));
    }
}
