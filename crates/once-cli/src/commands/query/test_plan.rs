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
    plan_from_selection(workspace, selection)
}

pub(crate) fn explicit_plan(
    workspace: &Path,
    graph: &[GraphTarget],
    targets: &[String],
) -> Result<TestPlan> {
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
    plan_from_selection(
        workspace,
        TestSelectionReport {
            schema: TEST_SELECTION_SCHEMA.to_string(),
            policy: TestSelectionPolicy {
                mode: "explicit".to_string(),
                safety: "exact".to_string(),
                evidence: "requested_targets".to_string(),
            },
            changed_paths: Vec::new(),
            unmatched_paths: Vec::new(),
            tests,
        },
    )
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

fn plan_from_selection(workspace: &Path, selection: TestSelectionReport) -> Result<TestPlan> {
    let mut batches = Vec::new();
    for test in &selection.tests {
        let Some(manifest) = super::stored_test_manifest_record(workspace, &test.id)? else {
            batches.push(TestBatch::new(&test.id, Vec::new())?);
            continue;
        };
        let sharded = manifest.source == "normalized_results"
            && manifest.listing_supported
            && manifest.case_filtering == "runner_args"
            && manifest.sharding.supported
            && !manifest.units.is_empty()
            && super::test_manifest_is_current(workspace, &test.id, &manifest);
        if !sharded {
            batches.push(TestBatch::new(&test.id, Vec::new())?);
            continue;
        }
        batches.extend(batches_for_manifest(test, manifest)?);
    }
    Ok(TestPlan::new(selection, batches)?)
}

fn batches_for_manifest(
    test: &AffectedTestRecord,
    manifest: once_core::TestManifest,
) -> Result<Vec<TestBatch>> {
    let mut batches = Vec::new();
    match manifest.sharding.granularity.as_str() {
        "case" => {
            for unit in manifest.units {
                batches.push(TestBatch::new(&test.id, vec![unit.id])?);
            }
        }
        "file" => {
            let mut by_file = BTreeMap::<String, Vec<String>>::new();
            let mut complete = true;
            for unit in manifest.units {
                let Some(file) = unit.file.filter(|file| !file.is_empty()) else {
                    complete = false;
                    break;
                };
                by_file.entry(file).or_default().push(unit.id);
            }
            if complete && !by_file.is_empty() {
                for units in by_file.into_values() {
                    batches.push(TestBatch::new(&test.id, units)?);
                }
            } else {
                batches.push(TestBatch::new(&test.id, Vec::new())?);
            }
        }
        _ => batches.push(TestBatch::new(&test.id, Vec::new())?),
    }
    Ok(batches)
}

#[cfg(test)]
mod tests {
    use once_core::{TestManifest, TestSharding, TestUnit};
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn file_sharding_groups_manifest_units_without_worker_identity() {
        let workspace = TempDir::new().unwrap();
        write_manifest(
            workspace.path(),
            TestSharding {
                supported: true,
                granularity: "file".to_string(),
            },
            vec![
                unit("a", "one.py"),
                unit("b", "one.py"),
                unit("c", "two.py"),
            ],
        );

        let manifest = super::super::test_manifest_record(workspace.path(), "tests/unit").unwrap();
        let plan = batches_for_manifest(&selection().tests[0], manifest).unwrap();

        assert_eq!(plan.len(), 2);
        assert!(plan
            .iter()
            .any(|batch| batch.test_filters == ["tests/unit::a", "tests/unit::b"]));
        assert!(plan
            .iter()
            .any(|batch| batch.test_filters == ["tests/unit::c"]));
    }

    #[test]
    fn case_sharding_creates_one_stable_batch_per_unit() {
        let workspace = TempDir::new().unwrap();
        write_manifest(
            workspace.path(),
            TestSharding {
                supported: true,
                granularity: "case".to_string(),
            },
            vec![unit("a", "one.py"), unit("b", "one.py")],
        );

        let manifest = super::super::test_manifest_record(workspace.path(), "tests/unit").unwrap();
        let plan = batches_for_manifest(&selection().tests[0], manifest).unwrap();

        assert_eq!(plan.len(), 2);
        assert!(plan.iter().all(|batch| batch.test_filters.len() == 1));
    }

    fn write_manifest(workspace: &Path, sharding: TestSharding, units: Vec<TestUnit>) {
        let manifest = TestManifest::new(
            "tests/unit",
            Some("demo".to_string()),
            "normalized_results",
            true,
            "runner_args",
            sharding,
            units,
        )
        .unwrap();
        let path = workspace.join(".once/test-manifests/tests/unit/manifest.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    }

    fn selection() -> TestSelectionReport {
        TestSelectionReport {
            schema: TEST_SELECTION_SCHEMA.to_string(),
            policy: TestSelectionPolicy {
                mode: "affected".to_string(),
                safety: "conservative".to_string(),
                evidence: "declared_graph".to_string(),
            },
            changed_paths: vec!["src/lib.py".to_string()],
            unmatched_paths: Vec::new(),
            tests: vec![AffectedTestRecord {
                id: "tests/unit".to_string(),
                kind: "pytest_test".to_string(),
                reasons: vec!["selected".to_string()],
            }],
        }
    }

    fn unit(name: &str, file: &str) -> TestUnit {
        TestUnit {
            id: format!("tests/unit::{name}"),
            name: name.to_string(),
            suite: file.to_string(),
            file: Some(file.to_string()),
        }
    }
}
