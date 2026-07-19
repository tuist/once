use once_cas::Digest;
use serde::{Deserialize, Serialize};

pub const TEST_SELECTION_SCHEMA: &str = "once.test_selection.v1";
pub const TEST_PLAN_SCHEMA: &str = "once.test_plan.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedTest {
    pub id: String,
    pub kind: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestSelectionPolicy {
    pub mode: String,
    pub safety: String,
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestSelectionReport {
    pub schema: String,
    pub policy: TestSelectionPolicy,
    pub changed_paths: Vec<String>,
    pub unmatched_paths: Vec<String>,
    pub tests: Vec<SelectedTest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestBatch {
    pub id: String,
    pub target: String,
    pub test_filters: Vec<String>,
}

impl TestBatch {
    pub fn new(
        target: impl Into<String>,
        mut test_filters: Vec<String>,
    ) -> Result<Self, serde_json::Error> {
        let target = target.into();
        test_filters.sort();
        test_filters.dedup();
        let id = stable_id("test-batch", &(&target, &test_filters))?;
        Ok(Self {
            id,
            target,
            test_filters,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestPlan {
    pub schema: String,
    pub id: String,
    pub selection: TestSelectionReport,
    pub batches: Vec<TestBatch>,
}

impl TestPlan {
    pub fn for_selected_targets(selection: TestSelectionReport) -> Result<Self, serde_json::Error> {
        let batches = selection
            .tests
            .iter()
            .map(|test| TestBatch::new(&test.id, Vec::new()))
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(selection, batches)
    }

    pub fn new(
        mut selection: TestSelectionReport,
        mut batches: Vec<TestBatch>,
    ) -> Result<Self, serde_json::Error> {
        selection.changed_paths.sort();
        selection.changed_paths.dedup();
        selection.unmatched_paths.sort();
        selection.unmatched_paths.dedup();
        for test in &mut selection.tests {
            test.reasons.sort();
            test.reasons.dedup();
        }
        selection
            .tests
            .sort_by(|left, right| left.id.cmp(&right.id).then(left.kind.cmp(&right.kind)));
        batches.sort_by(|left, right| left.id.cmp(&right.id));
        let id = stable_id("test-plan", &(&selection, &batches))?;
        Ok(Self {
            schema: TEST_PLAN_SCHEMA.to_string(),
            id,
            selection,
            batches,
        })
    }
}

fn stable_id<T: Serialize>(domain: &str, value: &T) -> Result<String, serde_json::Error> {
    let mut material = domain.as_bytes().to_vec();
    material.push(0);
    material.extend(serde_json::to_vec(value)?);
    Ok(Digest::of_bytes(&material).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_batch_identity_does_not_depend_on_plan_context() {
        let first = TestBatch::new("tests/unit", Vec::new()).unwrap();
        let second = TestBatch::new("tests/unit", Vec::new()).unwrap();

        assert_eq!(first.id, second.id);
    }

    #[test]
    fn filters_are_part_of_batch_identity() {
        let first = TestBatch::new("tests/unit", vec!["case-a".to_string()]).unwrap();
        let second = TestBatch::new("tests/unit", vec!["case-b".to_string()]).unwrap();

        assert_ne!(first.id, second.id);
    }

    #[test]
    fn filter_order_does_not_change_batch_identity() {
        let first = TestBatch::new(
            "tests/unit",
            vec!["case-b".to_string(), "case-a".to_string()],
        )
        .unwrap();
        let second = TestBatch::new(
            "tests/unit",
            vec!["case-a".to_string(), "case-b".to_string()],
        )
        .unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn selected_test_order_does_not_change_plan_identity() {
        let first = TestPlan::for_selected_targets(selection(vec![test("b"), test("a")])).unwrap();
        let second = TestPlan::for_selected_targets(selection(vec![test("a"), test("b")])).unwrap();

        assert_eq!(first, second);
    }

    fn selection(tests: Vec<SelectedTest>) -> TestSelectionReport {
        TestSelectionReport {
            schema: TEST_SELECTION_SCHEMA.to_string(),
            policy: TestSelectionPolicy {
                mode: "affected".to_string(),
                safety: "conservative".to_string(),
                evidence: "declared_graph".to_string(),
            },
            changed_paths: vec!["src/lib.rs".to_string()],
            unmatched_paths: Vec::new(),
            tests,
        }
    }

    fn test(id: &str) -> SelectedTest {
        SelectedTest {
            id: id.to_string(),
            kind: "test".to_string(),
            reasons: vec!["selected".to_string()],
        }
    }
}
