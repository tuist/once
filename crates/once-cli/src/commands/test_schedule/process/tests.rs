use super::*;

fn results() -> Value {
    json!({
        "schema": once_core::TEST_RESULTS_SCHEMA, "target": "tests",
        "runner": {"type": "demo", "metadata": {}}, "status": "passed",
        "summary": {"total": 1, "passed": 1, "failed": 0, "skipped": 0, "flaky": 0},
        "cases": [{"id": "tests::requested", "name": "requested", "suite": "tests", "status": "passed",
            "attempts": [{"status": "passed"}], "runner_metadata": {}}],
        "artifacts": {"logs": [], "native_results": []},
    })
}

#[test]
fn exact_batch_requires_valid_passing_evidence() {
    let exact = TestBatch::new("tests", vec!["tests::requested".to_string()]).unwrap();
    let whole = TestBatch::new("tests", Vec::new()).unwrap();
    assert!(!batch_succeeded(
        true,
        &exact,
        None,
        Some("missing requested unit")
    ));
    assert!(!batch_succeeded(true, &exact, None, None));
    assert!(batch_succeeded(true, &exact, Some(&results()), None));
    assert!(!batch_succeeded(false, &exact, Some(&results()), None));
    assert!(batch_succeeded(
        true,
        &whole,
        None,
        Some("optional results unavailable")
    ));

    let mut failed = results();
    failed["status"] = json!("failed");
    assert!(!batch_succeeded(true, &exact, Some(&failed), None));
    assert!(!batch_succeeded(true, &whole, Some(&failed), None));
    failed["status"] = json!("passed");
    failed["cases"][0]["status"] = json!("failed");
    assert!(!batch_succeeded(true, &exact, Some(&failed), None));
}

#[test]
fn exact_results_cannot_reuse_a_canonical_or_another_batch_path() {
    let workspace = tempfile::TempDir::new().unwrap();
    let batch = TestBatch::new("tests", vec!["tests::requested".to_string()]).unwrap();
    for path in [
        ".once/out/tests/test/test_results.json",
        ".once/out/tests/test/batches/other/test_results.json",
    ] {
        let error = load_batch_results(workspace.path(), &batch, Some(path)).unwrap_err();
        assert!(error.to_string().contains("must isolate batch outputs"));
    }
}

#[cfg(unix)]
#[test]
fn startup_failure_preserves_its_exit_status_and_stderr_despite_old_evidence() {
    use std::os::unix::fs::PermissionsExt;
    let _guard = super::super::cancellation::TEST_LOCK.lock().unwrap();
    let workspace = tempfile::TempDir::new().unwrap();
    let batch = TestBatch::new("tests", vec!["tests::requested".to_string()]).unwrap();
    let directory = workspace
        .path()
        .join(".once/out/tests/test/batches")
        .join(&batch.id);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("test_results.json"),
        serde_json::to_vec(&results()).unwrap(),
    )
    .unwrap();
    let executable = workspace.path().join("failing-runner");
    std::fs::write(
        &executable,
        "#!/bin/sh\necho 'runner startup failed' >&2\nexit 17\n",
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let run = run_test_target(&executable, workspace.path(), &batch, SandboxMode::Off, 0).unwrap();
    assert_eq!(run["success"], false);
    assert_eq!(run["exit_code"], 17);
    assert!(run["stderr"]
        .as_str()
        .unwrap()
        .contains("runner startup failed"));
}
