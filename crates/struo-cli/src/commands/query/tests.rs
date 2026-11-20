use std::collections::BTreeMap;

use struo_frontend::{Capability, GraphTarget, TargetLabel};

use super::*;

fn target(id: &str, kind: &str, capabilities: &[&str]) -> GraphTarget {
    let (package, name) = id.rsplit_once('/').unwrap_or(("", id));
    GraphTarget {
        label: TargetLabel {
            package: package.to_string(),
            name: name.to_string(),
            id: id.to_string(),
        },
        kind: kind.to_string(),
        deps: Vec::new(),
        dependency_edges: std::collections::BTreeMap::new(),
        srcs: Vec::new(),
        visibility: Vec::new(),
        attrs: BTreeMap::new(),
        capabilities: capabilities
            .iter()
            .map(|name| Capability {
                name: (*name).to_string(),
                output_groups: Vec::new(),
                requires_outputs: Vec::new(),
            })
            .collect(),
        providers: Vec::new(),
        tools: Vec::new(),
        diagnostics: Vec::new(),
    }
}

#[test]
fn test_results_summary_omits_case_records() {
    let value = serde_json::json!({
        "schema": "struo.test_results.v1",
        "target": "tests/unit",
        "runner": { "type": "rust_libtest", "metadata": {} },
        "status": "passed",
        "summary": {
            "total": 2,
            "passed": 1,
            "failed": 0,
            "skipped": 1,
            "flaky": 0
        },
        "cases": [
            { "name": "passes", "status": "passed" },
            { "name": "skips", "status": "skipped" }
        ],
        "artifacts": { "logs": ["test.log"], "native_results": [] }
    });

    let summary = test_results_summary_value(&value).unwrap();

    assert_eq!(summary["schema"], "struo.test_results_summary.v1");
    assert_eq!(summary["source_schema"], "struo.test_results.v1");
    assert_eq!(summary["summary"]["passed"], 1);
    assert_eq!(summary["summary"]["skipped"], 1);
    assert!(summary.get("cases").is_none());
}

#[test]
fn missing_modules_return_validation_diagnostics() {
    let workspace = tempfile::TempDir::new().unwrap();

    let validation = module_validation(workspace.path(), "modules/missing.star").unwrap();

    assert!(!validation.valid);
    assert_eq!(validation.diagnostics[0].code, "module_read_failed");
    assert!(!validation.diagnostics[0].repairs.is_empty());
}

#[test]
fn render_targets_human_reports_empty_and_populated() {
    assert_eq!(render_targets_human(&[]), "targets: none\n");
    let rendered = render_targets_human(&[TargetRecord {
        id: "apps/ios/App".to_string(),
        package: "apps/ios".to_string(),
        name: "App".to_string(),
        kind: "apple_application".to_string(),
        deps: Vec::new(),
        dependency_edges: std::collections::BTreeMap::new(),
        visibility: Vec::new(),
        capabilities: vec!["build".to_string(), "run".to_string()],
        tools: Vec::new(),
    }]);
    assert!(rendered.contains("apps/ios/App (apple_application) [build, run]"));
}

#[test]
fn render_capabilities_human_lists_outputs_and_requires() {
    let mut target = target("apps/ios/App", "apple_application", &["build", "run"]);
    target.capabilities[1].output_groups = vec!["default".to_string()];
    target.capabilities[1].requires_outputs = vec!["bundle".to_string()];

    let rendered = render_capabilities_human(&target);

    assert!(rendered.contains("apps/ios/App (apple_application)"));
    assert!(rendered.contains("  run\n    outputs: default\n    requires: bundle"));
}

#[test]
fn render_capabilities_human_reports_none() {
    let target = target("apps/ios/App", "mystery", &[]);
    assert!(render_capabilities_human(&target).contains("capabilities: none"));
}

#[test]
fn render_schema_human_includes_attrs_and_capabilities() {
    let schema = struo_frontend::built_in_target_kind_schema("apple_application").unwrap();
    let rendered = render_schema_human(&schema);
    assert!(rendered.starts_with("apple_application: "));
    assert!(rendered.contains("bundle_id: string (required"));
    assert!(rendered.contains("build: default, bundle, dsyms"));
    assert!(rendered.contains("run: default (requires: bundle)"));
}

#[test]
fn render_schema_human_includes_tools() {
    let schema = struo_frontend::built_in_target_kind_schema("rust_binary").unwrap();

    let rendered = render_schema_human(&schema);

    assert!(rendered.contains("tools:\n  rust: rustc, cargo"));
}

#[test]
fn target_records_filters_by_kind() {
    let records = target_records(
        vec![
            target("apps/ios/App", "apple_application", &["build", "run"]),
            target("apps/ios/AppTests", "apple_test_bundle", &["build"]),
        ],
        Some("apple_application"),
    );

    assert_eq!(
        records,
        vec![TargetRecord {
            id: "apps/ios/App".to_string(),
            package: "apps/ios".to_string(),
            name: "App".to_string(),
            kind: "apple_application".to_string(),
            deps: Vec::new(),
            dependency_edges: std::collections::BTreeMap::new(),
            visibility: Vec::new(),
            capabilities: vec!["build".to_string(), "run".to_string()],
            tools: Vec::new(),
        }]
    );
}

#[test]
fn target_id_path_accepts_only_path_safe_segments() {
    assert_eq!(
        target_id_path("apps/ios/AppTests").unwrap(),
        PathBuf::from("apps").join("ios").join("AppTests")
    );

    for target_id in [
        "",
        ".",
        "..",
        "apps/../Secret",
        "apps//Tests",
        "apps\\Tests",
        "apps/ios/App Tests",
        "apps/ios/App;rm",
    ] {
        assert!(
            target_id_path(target_id).is_err(),
            "{target_id} should fail"
        );
    }
}

#[test]
fn explicit_test_unit_validation_rejects_unsupported_filtering() {
    let manifest = TestManifest::new(
        "suite",
        Some("example".to_string()),
        "normalized_results",
        true,
        "unsupported",
        struo_core::TestSharding::default(),
        vec![TestUnit {
            id: "suite::case".to_string(),
            name: "case".to_string(),
            suite: "suite".to_string(),
            file: None,
        }],
    )
    .unwrap();

    let error = validate_test_unit(&manifest, "suite", "suite::case").unwrap_err();

    assert_eq!(
        error.to_string(),
        "target `suite` does not support explicit test-unit filtering"
    );
}

#[test]
fn explicit_test_unit_validation_requires_a_current_manifest_unit() {
    let manifest = TestManifest::new(
        "suite",
        Some("example".to_string()),
        "normalized_results",
        true,
        "runner_args",
        struo_core::TestSharding::default(),
        vec![],
    )
    .unwrap();

    let error = validate_test_unit(&manifest, "suite", "suite::missing").unwrap_err();

    assert!(error
        .to_string()
        .contains("run the whole target to refresh discovery"));
}
