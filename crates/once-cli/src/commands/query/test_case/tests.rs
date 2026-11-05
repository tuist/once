use super::*;

fn fixture() -> Value {
    json!({
        "schema": "once.test_results.v1", "target": "tests/unit", "status": "cancelled",
        "runner": {"type": "native", "metadata": {}},
        "summary": {"total": 2, "passed": 1, "failed": 0, "skipped": 0, "flaky": 1},
        "cases": [
            {"id": "other", "name": "other", "suite": "unit", "status": "passed",
             "runner_metadata": {}, "attempts": [{"status": "passed"}]},
            {"id": "retry", "name": "retry", "suite": "unit", "status": "flaky",
             "runner_metadata": {}, "attempts": [
                {"status": "failed", "timestamp": 900, "failure": {"message": "first failure"}, "extension": [3, {"a": true}]},
                {"status": "producer_future_status", "timestamp": 100, "extension": "last array entry"}
             ]}
        ],
        "artifacts": {"logs": ["../../never-read", "https://invalid.example/private"], "native_results": ["missing.json"]}
    })
}

fn write(workspace: &Path, bytes: &[u8]) -> std::path::PathBuf {
    let path = workspace.join(super::super::test_results_path("tests/unit").unwrap());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    path
}

fn query(workspace: &Path, attempt: Option<usize>) -> Result<Value> {
    test_case_value(workspace, "tests/unit", "retry", attempt)
}

fn failure(error: anyhow::Error, code: &str, attribute: &str) -> once_frontend::Diagnostic {
    let diagnostic = error
        .downcast::<once_frontend::analysis::AnalysisFailure>()
        .unwrap()
        .diagnostic;
    assert_eq!(diagnostic.code, code);
    assert_eq!(diagnostic.target.as_deref(), Some("tests/unit"));
    assert_eq!(diagnostic.attribute.as_deref(), Some(attribute));
    assert!(!diagnostic.repairs.is_empty());
    assert!(serde_json::to_vec(&diagnostic).unwrap().len() < 12000);
    diagnostic
}

#[test]
fn retained_attempt_positions_preserve_raw_evidence_without_discovery() {
    let workspace = tempfile::tempdir().unwrap();
    let results = fixture();
    let bytes = serde_json::to_vec(&results).unwrap();
    let path = write(workspace.path(), &bytes);
    for manifest in [None, Some("this is not valid TOML")] {
        if let Some(manifest) = manifest {
            std::fs::write(workspace.path().join("once.toml"), manifest).unwrap();
        }
        let first = query(workspace.path(), Some(1)).unwrap();
        let latest = query(workspace.path(), None).unwrap();
        assert_eq!(first["schema"], "once.test_case_result.v1");
        assert_eq!(first["status"], "cancelled");
        assert_eq!(first["case"]["status"], "flaky");
        assert_eq!(first["attempt_result"], results["cases"][1]["attempts"][0]);
        assert_eq!(latest["attempt_result"], results["cases"][1]["attempts"][1]);
        assert_eq!(latest["attempt"], 2);
        assert_eq!(latest["attempt_count"], 2);
        assert_eq!(latest, query(workspace.path(), Some(2)).unwrap());
        assert_eq!(latest["target_artifacts"], results["artifacts"]);
        assert_eq!(latest["artifact_availability"], "not_checked");
        assert_eq!(
            latest["rerun"],
            json!({"scope": "target", "configuration": "current", "reason": "retained_evidence_only", "argv": ["once", "test", "--format", "json", "--", "tests/unit"]})
        );
        assert_eq!(
            latest["results_digest"],
            blake3::hash(&bytes).to_hex().to_string()
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(!workspace.path().join(".once/test").exists());
    }
    let before = query(workspace.path(), None).unwrap();
    let mut changed = bytes.clone();
    changed.push(b'\n');
    write(workspace.path(), &changed);
    let after = query(workspace.path(), None).unwrap();
    assert_ne!(before["results_digest"], after["results_digest"]);
    assert_eq!(
        after["results_digest"],
        blake3::hash(&changed).to_hex().to_string()
    );
    let passed = test_case_value(workspace.path(), "tests/unit", "other", None).unwrap();
    assert_eq!(passed["status"], "cancelled");
    assert_eq!(passed["case"]["status"], "passed");
}

#[test]
fn focused_limits_are_inclusive_and_leave_legacy_reads_unlimited() {
    let workspace = tempfile::tempdir().unwrap();
    let mut bytes = serde_json::to_vec(&fixture()).unwrap();
    bytes.resize(16_777_216, b' ');
    write(workspace.path(), &bytes);
    assert_eq!(
        query(workspace.path(), None).unwrap()["results_digest"],
        blake3::hash(&bytes).to_hex().to_string()
    );
    bytes.push(b' ');
    write(workspace.path(), &bytes);
    failure(
        query(workspace.path(), None).unwrap_err(),
        "test_results_too_large",
        "results",
    );
    assert_eq!(
        super::super::test_results_value(workspace.path(), "tests/unit").unwrap(),
        fixture()
    );

    let mut results = fixture();
    results["cases"][1]["attempts"][1]["padding"] = json!("");
    write(workspace.path(), &serde_json::to_vec(&results).unwrap());
    let overhead = serde_json::to_vec(&query(workspace.path(), None).unwrap())
        .unwrap()
        .len();
    let padding = 1_048_576 - overhead;
    results["cases"][1]["attempts"][1]["padding"] = json!("x".repeat(padding));
    write(workspace.path(), &serde_json::to_vec(&results).unwrap());
    assert_eq!(
        serde_json::to_vec(&query(workspace.path(), None).unwrap())
            .unwrap()
            .len(),
        1_048_576
    );
    results["cases"][1]["attempts"][1]["padding"] = json!("x".repeat(padding + 1));
    write(workspace.path(), &serde_json::to_vec(&results).unwrap());
    failure(
        query(workspace.path(), None).unwrap_err(),
        "test_case_projection_too_large",
        "case",
    );
    assert!(query(workspace.path(), Some(1)).is_ok());
    // Escaped strings count by JSON bytes, not their unescaped length.
    results["cases"][1]["attempts"][1]["padding"] = json!("\0".repeat(1_048_576 / 6));
    write(workspace.path(), &serde_json::to_vec(&results).unwrap());
    failure(
        query(workspace.path(), None).unwrap_err(),
        "test_case_projection_too_large",
        "case",
    );
}

#[test]
fn selection_and_malformed_evidence_fail_with_bounded_diagnostics() {
    let workspace = tempfile::tempdir().unwrap();
    failure(
        query(workspace.path(), Some(0)).unwrap_err(),
        "invalid_test_result_selection",
        "attempt",
    );
    failure(
        query(workspace.path(), None).unwrap_err(),
        "test_results_unavailable",
        "results",
    );
    write(workspace.path(), &serde_json::to_vec(&fixture()).unwrap());
    let range = failure(
        query(workspace.path(), Some(3)).unwrap_err(),
        "test_attempt_not_found",
        "attempt",
    );
    assert!(range.repairs[0].contains("1..=2"));
    failure(
        test_case_value(workspace.path(), "tests/unit", "missing", None).unwrap_err(),
        "test_case_not_found",
        "case",
    );
    for (summary, case, attempt, attribute) in [
        (true, Some("retry"), None, "case"),
        (false, None, Some(1), "attempt"),
        (false, Some(""), None, "case"),
    ] {
        failure(
            validate_selection("tests/unit", summary, case, attempt).unwrap_err(),
            "invalid_test_result_selection",
            attribute,
        );
    }
    let mut results = fixture();
    results["schema"] = json!("bad".repeat(500_000));
    write(workspace.path(), &serde_json::to_vec(&results).unwrap());
    failure(
        query(workspace.path(), None).unwrap_err(),
        "invalid_test_results",
        "results",
    );
    write(
        workspace.path(),
        format!("{}0{}", "[".repeat(200), "]".repeat(200)).as_bytes(),
    );
    failure(
        query(workspace.path(), None).unwrap_err(),
        "invalid_test_results",
        "results",
    );
    let mut results = fixture();
    results["cases"][0]["id"] = json!("🦀".repeat(500_000));
    write(workspace.path(), &serde_json::to_vec(&results).unwrap());
    failure(
        test_case_value(
            workspace.path(),
            "tests/unit",
            &"missing".repeat(500_000),
            None,
        )
        .unwrap_err(),
        "test_case_not_found",
        "case",
    );
}
