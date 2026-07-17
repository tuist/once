use super::*;

#[test]
fn manifest_identity_is_independent_of_unit_order() {
    let first = manifest(vec![unit("case-b"), unit("case-a")]);
    let second = manifest(vec![unit("case-a"), unit("case-b")]);

    assert_eq!(first, second);
}

#[test]
fn manifest_rejects_empty_unit_identifiers() {
    let error = TestManifest::new(
        "tests/unit",
        None,
        "normalized_results",
        true,
        "runner_args",
        TestSharding::default(),
        vec![TestUnit {
            id: String::new(),
            name: "case-a".to_string(),
            suite: "tests/other".to_string(),
            file: None,
        }],
    )
    .unwrap_err();

    assert!(error.to_string().contains("cannot be empty"));
}

fn manifest(units: Vec<TestUnit>) -> TestManifest {
    TestManifest::new(
        "tests/unit",
        Some("runner".to_string()),
        "normalized_results",
        true,
        "runner_args",
        TestSharding::default(),
        units,
    )
    .and_then(|manifest| manifest.with_discovery_fingerprint(Some("inputs-v1".to_string())))
    .unwrap()
}

fn unit(name: &str) -> TestUnit {
    TestUnit {
        id: format!("tests/unit::{name}"),
        name: name.to_string(),
        suite: "tests/unit".to_string(),
        file: None,
    }
}
