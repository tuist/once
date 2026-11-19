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

#[test]
fn portable_binding_ignores_runtime_outputs_but_tracks_recursive_sources() {
    for patterns in [
        vec!["**/*".into(), "nested/**/*".into()],
        vec![".".into(), "./nested/**/*".into()],
        vec!["nested".into()],
        vec!["nested/*".into()],
    ] {
        let (workspace, mut graph, portable) = fixture();
        graph[0].srcs = patterns;
        let source = workspace.path().join("nested/deeper/source");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "first").unwrap();
        let plan = TestPlan::new(
            portable.plan.selection,
            vec![TestBatch::new("tests", vec![]).unwrap()],
        )
        .unwrap();
        let portable = PortableTestPlan::export(workspace.path(), &graph, plan).unwrap();

        let output = workspace.path().join(".struo/out/tests/binary");
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        std::fs::write(&output, "generated output").unwrap();
        portable.validate(workspace.path(), &graph).unwrap();
        std::fs::write(&output, "changed generated output").unwrap();
        portable.validate(workspace.path(), &graph).unwrap();

        std::fs::write(&source, "other").unwrap();
        assert!(portable.validate(workspace.path(), &graph).is_err());
        std::fs::write(&source, "first").unwrap();
        portable.validate(workspace.path(), &graph).unwrap();

        let added = workspace.path().join("nested/deeper/added");
        std::fs::write(&added, "new source").unwrap();
        assert!(portable.validate(workspace.path(), &graph).is_err());
        std::fs::remove_file(added).unwrap();
        portable.validate(workspace.path(), &graph).unwrap();
        std::fs::remove_file(source).unwrap();
        assert!(portable.validate(workspace.path(), &graph).is_err());
    }
}

#[test]
fn portable_binding_preserves_directory_only_source_patterns() {
    for pattern in ["nested/*/", "nested/subdir/"] {
        let (workspace, mut graph, portable) = fixture();
        graph[0].srcs = vec![pattern.into()];
        let source = workspace.path().join("nested/subdir/input.rs");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "first").unwrap();
        let unrelated = workspace.path().join("nested/unrelated.txt");
        std::fs::write(&unrelated, "first").unwrap();
        let plan = TestPlan::new(
            portable.plan.selection,
            vec![TestBatch::new("tests", vec![]).unwrap()],
        )
        .unwrap();
        let portable = PortableTestPlan::export(workspace.path(), &graph, plan).unwrap();

        std::fs::write(&unrelated, "other").unwrap();
        portable.validate(workspace.path(), &graph).unwrap();
        std::fs::write(workspace.path().join("nested/added.txt"), "not a source").unwrap();
        portable.validate(workspace.path(), &graph).unwrap();
        std::fs::write(&source, "other").unwrap();
        assert!(portable.validate(workspace.path(), &graph).is_err());
        std::fs::write(&source, "first").unwrap();
        portable.validate(workspace.path(), &graph).unwrap();
        std::fs::write(workspace.path().join("nested/subdir/added.rs"), "source").unwrap();
        assert!(portable.validate(workspace.path(), &graph).is_err());

        let directory = workspace.path().join("nested/subdir");
        std::fs::remove_dir_all(&directory).unwrap();
        std::fs::write(&directory, "first").unwrap();
        let portable =
            PortableTestPlan::export(workspace.path(), &graph, portable.plan.clone()).unwrap();
        std::fs::write(&directory, "other").unwrap();
        portable.validate(workspace.path(), &graph).unwrap();
    }
}

#[test]
fn portable_binding_tracks_provider_runtime_inputs_before_overlapping_sources() {
    for field in ["affected_inputs", "test_discovery_inputs"] {
        let (workspace, mut graph, portable) = fixture();
        graph[0].srcs = vec!["tree".into(), "tree/subdir/**/*".into()];
        std::fs::write(
            workspace.path().join("struo.toml"),
            "[modules]\npaths = [\"custom.star\"]\n",
        )
        .unwrap();
        std::fs::write(
            workspace.path().join("custom.star"),
            format!(
                "def _impl(ctx):\n    return {{\"{field}\": [\"tree/subdir\"]}}\ncustom_test = target_kind(impl = _impl)\n"
            ),
        )
        .unwrap();
        let source = workspace.path().join("tree/subdir/.struo/input");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "first").unwrap();
        let unrelated = workspace.path().join("tree/.struo/unrelated");
        std::fs::create_dir_all(unrelated.parent().unwrap()).unwrap();
        std::fs::write(&unrelated, "first").unwrap();
        let plan = TestPlan::new(
            portable.plan.selection,
            vec![TestBatch::new("tests", vec![]).unwrap()],
        )
        .unwrap();
        let portable = PortableTestPlan::export(workspace.path(), &graph, plan).unwrap();

        std::fs::write(&unrelated, "other").unwrap();
        portable.validate(workspace.path(), &graph).unwrap();
        std::fs::write(&source, "other").unwrap();
        assert!(portable.validate(workspace.path(), &graph).is_err());
        std::fs::write(&source, "first").unwrap();
        portable.validate(workspace.path(), &graph).unwrap();
        let added = workspace.path().join("tree/subdir/.struo/added");
        std::fs::write(&added, "source").unwrap();
        assert!(portable.validate(workspace.path(), &graph).is_err());
        std::fs::remove_file(added).unwrap();
        portable.validate(workspace.path(), &graph).unwrap();
        std::fs::remove_file(source).unwrap();
        assert!(portable.validate(workspace.path(), &graph).is_err());
    }
}

#[test]
fn portable_binding_tracks_explicit_runtime_sources() {
    for (package, declared) in [
        ("", ".struo/packages/input.rs"),
        ("", ".struo/packages"),
        ("package", "./.struo/packages/input.rs"),
    ] {
        let (workspace, mut graph, mut portable) = fixture();
        graph[0].label.package = package.into();
        if !package.is_empty() {
            graph[0].label.id = format!("{package}/tests");
        }
        portable.plan.selection.tests[0].id = graph[0].label.id.clone();
        graph[0].srcs = vec!["**/*".into(), declared.into()];
        let source = workspace.path().join(".struo/packages/input.rs");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "first").unwrap();
        let plan = TestPlan::new(
            portable.plan.selection,
            vec![TestBatch::new(&graph[0].label.id, vec![]).unwrap()],
        )
        .unwrap();
        let portable = PortableTestPlan::export(workspace.path(), &graph, plan).unwrap();

        std::fs::create_dir_all(workspace.path().join(".struo/out")).unwrap();
        std::fs::write(workspace.path().join(".struo/out/binary"), "output").unwrap();
        portable.validate(workspace.path(), &graph).unwrap();
        std::fs::write(&source, "other").unwrap();
        assert!(portable.validate(workspace.path(), &graph).is_err());
        std::fs::write(&source, "first").unwrap();
        portable.validate(workspace.path(), &graph).unwrap();
        std::fs::remove_file(source).unwrap();
        assert!(portable.validate(workspace.path(), &graph).is_err());
    }
}

#[cfg(unix)]
#[test]
fn portable_binding_tracks_symlink_sources_with_multiple_patterns() {
    let (workspace, mut graph, portable) = fixture();
    graph[0].srcs = vec!["alias/**/*.rs".into(), "tests/**/*.rs".into()];
    std::fs::create_dir_all(workspace.path().join("hidden")).unwrap();
    std::fs::create_dir_all(workspace.path().join("tests/nested")).unwrap();
    std::fs::write(workspace.path().join("tests/nested/test.rs"), "test").unwrap();
    let source = workspace.path().join("hidden/source.rs");
    std::fs::write(&source, "first").unwrap();
    std::os::unix::fs::symlink("hidden", workspace.path().join("alias")).unwrap();
    let plan = TestPlan::new(
        portable.plan.selection,
        vec![TestBatch::new("tests", vec![]).unwrap()],
    )
    .unwrap();
    let portable = PortableTestPlan::export(workspace.path(), &graph, plan).unwrap();

    std::fs::write(
        workspace.path().join("hidden/unrelated.txt"),
        "not a source",
    )
    .unwrap();
    portable.validate(workspace.path(), &graph).unwrap();
    std::fs::write(&source, "other").unwrap();
    assert!(portable.validate(workspace.path(), &graph).is_err());
    std::fs::write(&source, "first").unwrap();
    portable.validate(workspace.path(), &graph).unwrap();
    std::fs::write(workspace.path().join("hidden/added.rs"), "new source").unwrap();
    assert!(portable.validate(workspace.path(), &graph).is_err());
    std::fs::remove_file(workspace.path().join("hidden/added.rs")).unwrap();
    portable.validate(workspace.path(), &graph).unwrap();
    std::fs::remove_file(source).unwrap();
    assert!(portable.validate(workspace.path(), &graph).is_err());
}
