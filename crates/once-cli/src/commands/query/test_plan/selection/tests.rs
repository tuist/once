use std::collections::BTreeMap;

use once_frontend::{Capability, TargetLabel};
use tempfile::TempDir;

use super::*;
use crate::commands::query::test_plan::plan;

#[test]
fn deleted_source_still_matches_its_declared_pattern() {
    let workspace = TempDir::new().unwrap();
    let graph = vec![
        target("lib", "library", &["src/**/*.rs"], &[], false),
        target("tests", "test", &["tests/**/*.rs"], &["lib"], true),
    ];

    let report = selection_report(
        workspace.path(),
        &graph,
        &["src/removed/module.rs".to_string()],
    )
    .unwrap();

    assert_eq!(report.tests.len(), 1);
    assert_eq!(report.tests[0].id, "tests");
    assert_eq!(
        report.tests[0].reasons,
        vec!["changed dependency `lib` input `src/removed/module.rs`"]
    );
    assert!(report.unmatched_paths.is_empty());
}

#[test]
fn graph_definition_changes_select_every_test() {
    let workspace = TempDir::new().unwrap();
    let graph = vec![
        target("unit", "test", &["unit.rs"], &[], true),
        target("integration", "test", &["integration.rs"], &[], true),
    ];

    let report = selection_report(
        workspace.path(),
        &graph,
        &["packages/core/once.toml".to_string()],
    )
    .unwrap();

    assert_eq!(
        report
            .tests
            .iter()
            .map(|test| test.id.as_str())
            .collect::<Vec<_>>(),
        vec!["unit", "integration"]
    );
}

#[test]
fn test_plan_identity_is_stable_and_independent_of_changed_path_order() {
    let workspace = TempDir::new().unwrap();
    let graph = vec![target(
        "unit",
        "test",
        &["src/**/*.rs", "tests/**/*.rs"],
        &[],
        true,
    )];

    let first = plan(
        workspace.path(),
        &graph,
        &["tests/a.rs".to_string(), "src/a.rs".to_string()],
    )
    .unwrap();
    let second = plan(
        workspace.path(),
        &graph,
        &["src/a.rs".to_string(), "tests/a.rs".to_string()],
    )
    .unwrap();

    assert_eq!(first.id, second.id);
    assert_eq!(first.batches, second.batches);
}

#[test]
fn unmatched_paths_conservatively_select_every_test() {
    let workspace = TempDir::new().unwrap();
    let graph = vec![target("unit", "test", &["tests/**/*.rs"], &[], true)];

    let report = selection_report(workspace.path(), &graph, &["README.md".to_string()]).unwrap();

    assert_eq!(report.tests.len(), 1);
    assert!(report.tests[0].reasons[0].contains("has no declared owner"));
    assert_eq!(report.unmatched_paths, vec!["README.md"]);
}

#[test]
fn changed_paths_that_are_not_workspace_relative_do_not_fail_selection() {
    let workspace = TempDir::new().unwrap();
    let graph = vec![target("unit", "test", &["tests/**/*.rs"], &[], true)];

    // Absolute paths and `..` escapes cannot be normalized to workspace-relative
    // form. They must degrade to conservative selection, not abort the command.
    let report = selection_report(
        workspace.path(),
        &graph,
        &["/etc/hosts".to_string(), "../outside/file.rs".to_string()],
    )
    .unwrap();

    assert_eq!(report.tests.len(), 1);
    assert!(report.tests[0].reasons[0].contains("has no declared owner"));
}

#[test]
fn configured_module_changes_select_every_test() {
    let workspace = TempDir::new().unwrap();
    std::fs::write(
        workspace.path().join("once.toml"),
        "[modules]\npaths = [\"modules/**/*.star\"]\n",
    )
    .unwrap();
    let graph = vec![target("unit", "test", &["tests/**/*.rs"], &[], true)];

    let report = selection_report(
        workspace.path(),
        &graph,
        &["modules/testing.star".to_string()],
    )
    .unwrap();

    assert_eq!(report.tests.len(), 1);
    assert!(report.tests[0].reasons[0].contains("changed graph definition"));
}

fn target(id: &str, kind: &str, srcs: &[&str], deps: &[&str], test: bool) -> GraphTarget {
    GraphTarget {
        label: TargetLabel {
            package: String::new(),
            name: id.to_string(),
            id: id.to_string(),
        },
        kind: kind.to_string(),
        deps: deps.iter().map(ToString::to_string).collect(),
        dependency_edges: BTreeMap::new(),
        srcs: srcs.iter().map(ToString::to_string).collect(),
        attrs: BTreeMap::new(),
        capabilities: test
            .then(|| Capability {
                name: "test".to_string(),
                output_groups: Vec::new(),
                requires_outputs: Vec::new(),
            })
            .into_iter()
            .collect(),
        providers: Vec::new(),
        tools: Vec::new(),
        diagnostics: Vec::new(),
    }
}
