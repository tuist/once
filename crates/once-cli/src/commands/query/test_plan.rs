mod inputs;
mod selection;

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use once_core::{TestBatch, TestSelectionPolicy, TestSelectionReport, TEST_SELECTION_SCHEMA};
use once_frontend::GraphTarget;

pub(crate) use once_core::{SelectedTest as AffectedTestRecord, TestPlan};

pub(crate) fn affected_tests(
    workspace: &Path,
    graph: &[GraphTarget],
    changed_paths: &[String],
) -> Result<Vec<AffectedTestRecord>> {
    Ok(selection::selection_report(workspace, graph, changed_paths)?.tests)
}

pub(crate) fn plan(
    workspace: &Path,
    graph: &[GraphTarget],
    changed_paths: &[String],
) -> Result<TestPlan> {
    let selection = selection::selection_report(workspace, graph, changed_paths)?;
    plan_from_selection(selection)
}

pub(crate) fn explicit_plan(graph: &[GraphTarget], targets: &[String]) -> Result<TestPlan> {
    let by_id = graph
        .iter()
        .map(|target| (target.label.id.as_str(), target))
        .collect::<BTreeMap<_, _>>();
    let tests = targets
        .iter()
        .map(|target_id| {
            let target = by_id
                .get(target_id.as_str())
                .with_context(|| format!("no target matches `{target_id}`"))?;
            Ok(AffectedTestRecord {
                id: target.label.id.clone(),
                kind: target.kind.clone(),
                reasons: vec!["explicitly requested test target".to_string()],
            })
        })
        .collect::<Result<Vec<_>>>()?;
    plan_from_selection(TestSelectionReport {
        schema: TEST_SELECTION_SCHEMA.to_string(),
        policy: TestSelectionPolicy {
            mode: "explicit".to_string(),
            safety: "exact".to_string(),
            evidence: "requested_targets".to_string(),
        },
        changed_paths: Vec::new(),
        unmatched_paths: Vec::new(),
        tests,
    })
}

pub(crate) fn explicit_unit_plan(
    graph: &[GraphTarget],
    target_id: &str,
    test_unit: &str,
) -> Result<TestPlan> {
    let target = graph
        .iter()
        .find(|target| target.label.id == target_id)
        .with_context(|| format!("no target matches `{target_id}`"))?;
    let selection = TestSelectionReport {
        schema: TEST_SELECTION_SCHEMA.to_string(),
        policy: TestSelectionPolicy {
            mode: "explicit".to_string(),
            safety: "exact".to_string(),
            evidence: "requested_test_unit".to_string(),
        },
        changed_paths: Vec::new(),
        unmatched_paths: Vec::new(),
        tests: vec![AffectedTestRecord {
            id: target.label.id.clone(),
            kind: target.kind.clone(),
            reasons: vec![format!("explicitly requested test unit `{test_unit}`")],
        }],
    };
    Ok(TestPlan::new(
        selection,
        vec![TestBatch::new(
            &target.label.id,
            vec![test_unit.to_string()],
        )?],
    )?)
}

fn plan_from_selection(selection: TestSelectionReport) -> Result<TestPlan> {
    Ok(TestPlan::for_selected_targets(selection)?)
}
