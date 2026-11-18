use std::collections::BTreeMap;

use super::*;
use struo_core::{
    SelectedTest, TestSelectionPolicy, TestSelectionReport, TestSharding, TestUnit,
    TEST_SELECTION_SCHEMA,
};
use struo_frontend::{Capability, TargetLabel};

fn fixture() -> (tempfile::TempDir, Vec<GraphTarget>, PortableTestPlan) {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("struo.toml"), "").unwrap();
    std::fs::write(workspace.path().join("input"), "discovery input").unwrap();
    let graph = vec![GraphTarget {
        label: TargetLabel {
            package: String::new(),
            name: "tests".into(),
            id: "tests".into(),
        },
        kind: "custom_test".into(),
        deps: vec![],
        dependency_edges: BTreeMap::default(),
        srcs: vec!["input".into()],
        visibility: vec![],
        attrs: BTreeMap::default(),
        capabilities: vec![Capability {
            name: "test".into(),
            output_groups: vec![],
            requires_outputs: vec![],
        }],
        providers: vec![],
        tools: vec![],
        diagnostics: vec![],
    }];
    let manifest = TestManifest::new(
        "tests",
        None,
        "normalized_results",
        true,
        "runner_args",
        TestSharding {
            supported: true,
            granularity: "file".into(),
        },
        [("a", "first"), ("b", "first"), ("c", "second")]
            .into_iter()
            .map(|(id, file)| TestUnit {
                id: id.into(),
                name: id.into(),
                suite: "suite".into(),
                file: Some(file.into()),
            })
            .collect(),
    )
    .unwrap()
    .with_discovery_fingerprint(Some(
        discovery_fingerprint(workspace.path(), &graph, "tests").unwrap(),
    ))
    .unwrap();
    super::super::super::write_test_manifest(workspace.path(), &manifest).unwrap();
    let selection = TestSelectionReport {
        schema: TEST_SELECTION_SCHEMA.into(),
        policy: TestSelectionPolicy {
            mode: "explicit".into(),
            safety: "exact".into(),
            evidence: "requested_targets".into(),
        },
        changed_paths: vec![],
        unmatched_paths: vec![],
        tests: vec![SelectedTest {
            id: "tests".into(),
            kind: "custom_test".into(),
            reasons: vec![],
        }],
    };
    let plan = PortableTestPlan::export(
        workspace.path(),
        &graph,
        super::super::plan_from_selection_with_graph(workspace.path(), &graph, selection).unwrap(),
    )
    .unwrap();
    (workspace, graph, plan)
}

#[test]
fn portable_document_rejects_invalid_identity_scopes_and_fixture_splitting() {
    let (workspace, graph, portable) = fixture();
    assert_eq!(portable.plan.batches.len(), 2);
    for filters in [
        vec![vec!["a"], vec!["a", "b", "c"]],
        vec![vec!["a"], vec!["b"], vec!["c"]],
        vec![vec!["a", "b"]],
        vec![vec!["unknown"]],
    ] {
        let mut invalid = portable.clone();
        invalid.plan = TestPlan::new(
            invalid.plan.selection,
            filters
                .into_iter()
                .map(|units| {
                    TestBatch::new("tests", units.into_iter().map(str::to_string).collect())
                        .unwrap()
                })
                .collect(),
        )
        .unwrap();
        invalid.id = invalid.identity().unwrap();
        assert!(invalid.validate(workspace.path(), &graph).is_err());
    }
    let mut invalid = portable.clone();
    invalid.schema = "struo.portable_test_plan.v999".into();
    assert!(invalid.validate(workspace.path(), &graph).is_err());
    let mut invalid = portable.clone();
    invalid.id = "forged".into();
    assert!(invalid.validate(workspace.path(), &graph).is_err());
    assert!(portable.select(&["unknown".into()]).is_err());
    let id = portable.plan.batches[0].id.clone();
    assert!(portable.select(&[id.clone(), id.clone()]).is_err());
    assert_eq!(
        portable.select(&[id]).unwrap().batches,
        portable.plan.batches[..1]
    );
}

#[test]
fn portable_binding_rejects_changed_inputs_configuration_and_module_implementation() {
    let (workspace, graph, portable) = fixture();
    std::fs::write(workspace.path().join("input"), "changed").unwrap();
    assert!(portable
        .validate(workspace.path(), &graph)
        .unwrap_err()
        .to_string()
        .contains("stale"));
    std::fs::write(workspace.path().join("input"), "discovery input").unwrap();
    let mut changed_graph = graph.clone();
    changed_graph[0].srcs.push("new_input".into());
    assert!(portable.validate(workspace.path(), &changed_graph).is_err());
    let mut changed_graph = graph.clone();
    changed_graph[0].attrs.insert(
        "mode".into(),
        struo_frontend::AttrValue::String("changed".into()),
    );
    assert!(portable.validate(workspace.path(), &changed_graph).is_err());
    std::fs::write(
        workspace.path().join("struo.toml"),
        "[modules]\npaths = [\"custom.star\"]\n",
    )
    .unwrap();
    std::fs::write(workspace.path().join("custom.star"), "VALUE = 2\n").unwrap();
    assert!(portable
        .validate(workspace.path(), &graph)
        .unwrap_err()
        .to_string()
        .contains("stale"));
}
