mod editing;
mod validation;

use crate::{
    load_triggers_toml_str, TriggerConfiguration, TriggerInvocation, TriggerOn, TriggerSpec,
};

const SOURCE: &str = r#"
[[trigger]]
id = "core-checks"
on = { event = "vcs.ref_changed", refs = ["refs/heads/main"] }
invoke = [{ target = "checks/core", capability = "test", configuration = { os = "linux", arch = "x86_64", tokens = [] } }]
"#;

fn trigger() -> TriggerSpec {
    TriggerSpec {
        id: "core-checks".to_string(),
        on: TriggerOn {
            event: "vcs.ref_changed".to_string(),
            refs: vec!["refs/heads/main".to_string()],
        },
        invoke: vec![TriggerInvocation {
            target: "checks/core".to_string(),
            capability: "test".to_string(),
            configuration: TriggerConfiguration {
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
                tokens: Vec::new(),
            },
        }],
    }
}

#[test]
fn public_contract_round_trips_and_resolves_explicit_configuration() {
    let triggers = load_triggers_toml_str("struo.toml", SOURCE).unwrap();
    assert_eq!(triggers, [trigger()]);
    let json = serde_json::to_string(&triggers).unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<TriggerSpec>>(&json).unwrap(),
        triggers
    );
    let configuration = triggers[0].invoke[0].configuration.resolve().unwrap();
    assert_eq!(configuration.os, "linux");
    assert_eq!(configuration.arch, "x86_64");
    assert_eq!(
        configuration.tokens,
        ["linux-x86_64", "linux", "x86_64", "default"]
    );
    assert!(load_triggers_toml_str("struo.toml", "[workspace]\n")
        .unwrap()
        .is_empty());
}

#[test]
fn reads_validates_and_edits_without_evaluating_missing_or_malicious_modules() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("malicious.bzl"),
        "fail(\"must never evaluate\")",
    )
    .unwrap();
    let src = format!("[modules]\npaths = [\"malicious.bzl\", \"missing.bzl\"]\n{SOURCE}");
    let path = root.path().join("struo.toml");
    std::fs::write(&path, &src).unwrap();
    let triggers = load_triggers_toml_str(&path.display().to_string(), &src).unwrap();
    assert_eq!(triggers, [trigger()]);
    assert!(crate::validate_triggers(&triggers).is_empty());
    let edited = crate::apply_trigger_operations(
        &src,
        &[crate::TriggerEditOperation::Update {
            id: trigger().id,
            trigger: trigger(),
        }],
    )
    .unwrap();
    assert_eq!(
        load_triggers_toml_str("struo.toml", &edited).unwrap(),
        triggers
    );
    assert!(edited.contains("paths = [\"malicious.bzl\", \"missing.bzl\"]"));
}

#[test]
fn trigger_reader_does_not_validate_unrelated_target_or_module_shapes() {
    let src = format!("modules = 42\ntarget = [{{ future_field = true }}]\n{SOURCE}");
    assert_eq!(
        load_triggers_toml_str("struo.toml", &src).unwrap(),
        [trigger()]
    );
}

#[test]
fn ordinary_manifest_entry_points_accept_root_triggers_without_loading_modules() {
    let src = format!("[modules]\npaths = [\"missing.bzl\"]\n{SOURCE}");
    assert!(crate::load_toml_str("struo.toml", &src).unwrap().is_empty());
    assert!(crate::load_cache_provider_toml_str("struo.toml", &src)
        .unwrap()
        .is_none());
    crate::load_infrastructure_toml_str("struo.toml", &src).unwrap();
    assert_eq!(
        crate::manifest::load_module_paths_toml_str("struo.toml", &src).unwrap(),
        ["missing.bzl"]
    );
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("struo.toml"), src).unwrap();
    crate::load_workspace_configuration(root.path()).unwrap();
}

#[test]
fn package_manifests_reject_trigger_declarations_with_root_repair() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("struo.toml"), SOURCE).unwrap();
    std::fs::create_dir(root.path().join("checks")).unwrap();
    std::fs::write(root.path().join("checks/struo.toml"), SOURCE).unwrap();
    let crate::Error::Triggers { diagnostics, .. } =
        crate::load_workspace(root.path()).unwrap_err()
    else {
        panic!("expected structured trigger diagnostics");
    };
    assert_eq!(diagnostics[0].code, "trigger_root_only");
    assert_eq!(diagnostics[0].attribute.as_deref(), Some("trigger"));
    assert!(!diagnostics[0].repairs.is_empty());
    assert!(crate::manifest::load_toml_with(
        "checks/struo.toml",
        "trigger = []",
        root.path(),
        "checks"
    )
    .is_err());
}

#[test]
fn ordinary_loads_retain_structured_trigger_validation_failures() {
    let src = SOURCE.replace("vcs.ref_changed", "other");
    let crate::Error::Triggers { diagnostics, .. } =
        crate::load_toml_str("struo.toml", &src).unwrap_err()
    else {
        panic!("expected structured trigger diagnostics");
    };
    assert_eq!(diagnostics[0].code, "unsupported_trigger_event");
}

#[test]
fn nested_array_of_tables_invocations_are_supported() {
    let source = r#"
[[trigger]]
id = "core-checks"
on = { event = "vcs.ref_changed", refs = ["refs/heads/main"] }
[[trigger.invoke]]
target = "checks/core"
capability = "test"
configuration = { os = "linux", arch = "x86_64", tokens = [] }
"#;
    assert_eq!(
        load_triggers_toml_str("struo.toml", source).unwrap(),
        [trigger()]
    );
}
