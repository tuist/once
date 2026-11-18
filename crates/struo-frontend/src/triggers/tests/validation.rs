use super::{trigger, SOURCE};
use crate::{load_triggers_toml_str, validate_triggers, TriggerSpec};

#[test]
fn rejects_unknown_fields_at_every_trigger_level_and_missing_explicit_fields() {
    for (before, after) in [
        ("id =", "extra = true\nid ="),
        ("event =", "extra = true, event ="),
        ("target =", "extra = true, target ="),
        ("os =", "extra = true, os ="),
        ("os = \"linux\", ", ""),
        ("arch = \"x86_64\", ", ""),
        (", tokens = []", ""),
        (
            "configuration = { os = \"linux\", arch = \"x86_64\", tokens = [] }",
            "other = {}",
        ),
    ] {
        let source = SOURCE.replace(before, after);
        let diagnostics = load_triggers_toml_str("struo.toml", &source).unwrap_err();
        assert_eq!(diagnostics[0].code, "trigger_parse_error", "{source}");
        assert!(!diagnostics[0].repairs.is_empty());
    }
    let mut value = serde_json::to_value(trigger()).unwrap();
    value["invoke"][0]["configuration"]
        .as_object_mut()
        .unwrap()
        .remove("tokens");
    assert!(serde_json::from_value::<TriggerSpec>(value).is_err());
}

#[test]
fn rejects_bad_ids_empty_members_and_unsupported_events_or_capabilities() {
    for id in [
        "",
        " ",
        "../escape",
        "foo/bar",
        "foo.bar",
        "-leading",
        "with space",
        "ümlaut",
    ] {
        let mut candidate = trigger();
        candidate.id = id.to_string();
        assert_eq!(
            validate_triggers(&[candidate])[0].code,
            "invalid_trigger_id",
            "{id}"
        );
    }
    let mut candidate = trigger();
    candidate.on.event = "vcs.push".to_string();
    candidate.on.refs.clear();
    candidate.invoke.clear();
    let diagnostics = validate_triggers(&[candidate]);
    for code in [
        "unsupported_trigger_event",
        "trigger_refs_empty",
        "trigger_invocations_empty",
    ] {
        assert!(diagnostics.iter().any(|diagnostic| diagnostic.code == code));
    }
    for capability in ["", "lint", "Test", "test "] {
        let mut candidate = trigger();
        candidate.invoke[0].capability = capability.to_string();
        assert_eq!(
            validate_triggers(&[candidate])[0].code,
            "unsupported_trigger_capability"
        );
    }
    for capability in ["build", "test", "run"] {
        let mut candidate = trigger();
        candidate.invoke[0].capability = capability.to_string();
        assert!(validate_triggers(&[candidate]).is_empty());
    }
}

#[test]
fn refs_are_exact_fully_qualified_valid_names() {
    for reference in [
        "",
        "main",
        "HEAD",
        "refs/",
        "refs//main",
        "refs/heads/*",
        "refs/heads/m?in",
        "refs/heads/[main]",
        "refs/heads/has space",
        "refs/heads/main\n",
        "refs/heads/main\u{7f}",
        "refs/heads/main\u{85}",
        "refs/heads/../main",
        "refs/heads/m..n",
        "refs/heads/.hidden",
        "refs/heads/main.",
        "refs/heads/main.lock",
        "refs/heads/main/",
        "refs/heads/m@{1}",
        "refs/heads/m~1",
        "refs/heads/m^1",
        "refs/heads/m:1",
        "refs/heads/m\\n",
    ] {
        let mut candidate = trigger();
        candidate.on.refs = vec![reference.to_string()];
        assert_eq!(
            validate_triggers(&[candidate])[0].code,
            "invalid_trigger_ref",
            "{reference:?}"
        );
    }
    let mut candidate = trigger();
    candidate.on.refs = vec![
        "refs/heads/main".to_string(),
        "refs/tags/v1.0".to_string(),
        "refs/custom/feature/topic".to_string(),
        "refs/heads/UPPER.LOCK".to_string(),
    ];
    assert!(validate_triggers(&[candidate]).is_empty());
}

#[test]
fn target_references_must_already_be_canonical_and_never_queries() {
    for target in [
        "",
        "../escape",
        "/checks/core",
        "checks/../core",
        "./checks/core",
        "checks/./core",
        "checks//core",
        "checks/core/",
        "//checks:core",
        "cargo:core",
        "checks\\core",
        "checks/*",
        "checks/?",
        "checks/[core]",
        "checks/core\n",
    ] {
        let mut candidate = trigger();
        candidate.invoke[0].target = target.to_string();
        assert_eq!(
            validate_triggers(&[candidate])[0].code,
            "invalid_trigger_target",
            "{target:?}"
        );
    }
    let mut candidate = trigger();
    candidate.invoke[0].target = "core".to_string();
    assert!(validate_triggers(&[candidate]).is_empty());
}

#[test]
fn detects_duplicate_ids_refs_tokens_and_normalized_invocations() {
    let mut candidate = trigger();
    assert_eq!(
        validate_triggers(&[candidate.clone(), candidate.clone()])[0].code,
        "duplicate_trigger_id"
    );
    candidate.on.refs.push(candidate.on.refs[0].clone());
    candidate.invoke[0].configuration.os = "darwin".to_string();
    candidate.invoke[0].configuration.arch = "arm64".to_string();
    let mut normalized = candidate.invoke[0].clone();
    normalized.configuration.os = "macos".to_string();
    normalized.configuration.arch = "aarch64".to_string();
    normalized.configuration.tokens = vec!["macos".to_string(), "default".to_string()];
    candidate.invoke.push(normalized);
    let diagnostics = validate_triggers(&[candidate]);
    assert!(diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "duplicate_trigger_ref"));
    assert!(diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "duplicate_trigger_invocation"));
    let mut candidate = trigger();
    candidate.invoke[0].configuration.tokens = vec!["extra".to_string(), "extra".to_string()];
    assert_eq!(
        validate_triggers(&[candidate])[0].code,
        "duplicate_trigger_token"
    );
}

#[test]
fn configuration_values_cannot_be_empty_and_resolve_never_falls_back_to_host() {
    for field in ["os", "arch", "tokens"] {
        let mut candidate = trigger();
        let configuration = &mut candidate.invoke[0].configuration;
        match field {
            "os" => configuration.os = " \t".to_string(),
            "arch" => configuration.arch = String::new(),
            _ => configuration.tokens.push(" ".to_string()),
        }
        assert!(configuration.resolve().is_err());
        let diagnostics = validate_triggers(&[candidate]);
        assert_eq!(diagnostics[0].code, "invalid_trigger_configuration");
        assert!(diagnostics[0].attribute.as_ref().unwrap().contains(field));
        assert_eq!(diagnostics[0].target.as_deref(), Some("core-checks"));
        assert!(!diagnostics[0].repairs.is_empty());
    }
    let mut configuration = trigger().invoke.remove(0).configuration;
    configuration.os = "darwin".to_string();
    configuration.arch = "arm64".to_string();
    let resolved = configuration.resolve().unwrap();
    assert_eq!(resolved.os, "macos");
    assert_eq!(resolved.arch, "aarch64");
}
