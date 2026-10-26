#![cfg(unix)]

use base64::Engine;
use serde_json::{json, Value};

#[path = "support/cache_miss.rs"]
mod cache_miss;
#[path = "support/hosted.rs"]
mod hosted;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn hosted_batches_round_trip_exact_bytes_and_publish_native_retries() {
    let fixture = hosted::Fixture::new().await;
    assert_eq!(
        fixture.plan().await,
        json!({"shard_plan_id": hosted::ID, "shards": [0, 1]})
    );
    let local = fixture.invoke(&["query", "test-plan"]).await;
    {
        let state = fixture.state.lock().unwrap();
        let artifact: Value = serde_json::from_slice(&state.artifact).unwrap();
        assert_eq!(artifact["shardPlanID"], hosted::ID);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(artifact["portablePlan"].as_str().unwrap())
                .unwrap(),
            local.stdout
        );
        assert_eq!(state.modules.len(), 2);
        let create = &state
            .requests
            .iter()
            .find(|(path, _)| path.ends_with("/shards"))
            .unwrap()
            .1;
        assert_eq!(create["granularity"], "module");
        assert_eq!(create["shard_total"], 2);
        for (_, body) in state
            .requests
            .iter()
            .filter(|(path, _)| path.contains("/upload/"))
        {
            assert_eq!(body["shard_plan_id"], hosted::ID);
        }
        let complete = &state
            .requests
            .iter()
            .find(|(path, _)| path.ends_with("/complete"))
            .unwrap()
            .1;
        assert_eq!(
            complete["parts"],
            json!([{"part_number":1,"etag":"\"etag-1\""}])
        );
    }
    std::fs::remove_dir_all(fixture.workspace.path().join(".once")).unwrap();
    let home = tempfile::tempdir().unwrap();
    let output = fixture
        .command(
            home.path(),
            &["test", "--shard-report-revision", "2", "--jobs", "2"],
        )
        .env("TUIST_SHARD_PLAN_ID", hosted::ID)
        .env("TUIST_SHARD_INDEX", "0")
        .env("TUIST_SHARD_ATTEMPT", "3")
        .env("GITHUB_RUN_ATTEMPT", "7")
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let state = fixture.state.lock().unwrap();
    assert!(
        state.protocol_errors.is_empty(),
        "{:?}",
        state.protocol_errors
    );
    assert!(state.requests.iter().any(|(path, _)| path
        == &format!(
            "/api/projects/acme/app/tests/shards/{}/0?shard_plan_id={}",
            hosted::ID,
            hosted::ID
        )));
    assert_eq!(state.reports.len(), 1);
    let report = &state.reports[0];
    assert_eq!(report["status"], "success");
    assert_eq!(report["shard_attempt"], 3);
    assert_eq!(report["shard_report_revision"], 2);
    let module = &report["test_modules"][0];
    assert_eq!(module["name"], "native");
    assert_eq!(module["test_cases"].as_array().unwrap().len(), 3);
    let case = module["test_cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "recovers")
        .unwrap();
    assert_eq!(case["test_suite_name"], "NativeSuite");
    assert_eq!(case["status"], "success");
    assert_eq!(case["duration"], 10);
    assert_eq!(
        case["repetitions"],
        json!([
            {"repetition_number":1,"name":"Attempt 1","status":"failure","duration":3},
            {"repetition_number":2,"name":"Attempt 2","status":"success","duration":7},
        ])
    );
    assert_eq!(case["failures"][0]["message"], "first failure");
    let ran = std::fs::read_to_string(fixture.workspace.path().join("ran")).unwrap();
    assert!(
        ran.lines().any(|line| line == "native::a,native::b"),
        "{ran}"
    );
    assert!(ran.lines().any(|line| line == "native::c"), "{ran}");
}

#[tokio::test]
async fn empty_failed_incomplete_and_rejected_reports_never_become_successful_partitions() {
    let fixture = hosted::Fixture::new().await;
    fixture.plan().await;
    let arguments = [
        "test",
        "--shard-plan",
        hosted::ID,
        "--shard-index",
        "1",
        "--jobs",
        "1",
    ];
    fixture.state.lock().unwrap().assigned = Some(vec![]);
    let before = std::fs::read(fixture.workspace.path().join("ran")).unwrap();
    assert!(fixture.invoke(&arguments).await.status.success());
    assert_eq!(
        std::fs::read(fixture.workspace.path().join("ran")).unwrap(),
        before
    );
    assert_eq!(
        fixture.state.lock().unwrap().reports[0]["test_modules"],
        json!([])
    );
    fixture.state.lock().unwrap().assigned = None;
    for mode in ["case-failed", "failed", "incomplete"] {
        std::fs::write(fixture.workspace.path().join("mode"), mode).unwrap();
        let output = fixture.invoke(&arguments).await;
        assert!(!output.status.success(), "mode {mode}");
        let state = fixture.state.lock().unwrap();
        let report = state.reports.last().unwrap();
        assert_eq!(report["status"], "failure");
        if mode == "case-failed" {
            let module = &report["test_modules"][0];
            assert_eq!(module["status"], "failure");
            let case = module["test_cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|case| case["name"] == "recovers")
                .unwrap();
            assert_eq!(case["status"], "failure");
            assert_eq!(case["duration"], 13);
            assert_eq!(case["repetitions"][0]["status"], "failure");
            assert_eq!(case["failures"][0]["message"], "persistent failure");
        }
    }
    std::fs::remove_file(fixture.workspace.path().join("mode")).unwrap();
    fixture.state.lock().unwrap().reject_report = true;
    assert!(!fixture.invoke(&arguments).await.status.success());
    assert_eq!(fixture.state.lock().unwrap().reports.len(), 5);
    let ran = std::fs::read_to_string(fixture.workspace.path().join("ran")).unwrap();
    assert_eq!(
        ran.lines().count(),
        9,
        "all batches must run after process failure: {ran}"
    );
}

#[tokio::test]
async fn upload_failure_and_foreign_artifacts_are_rejected() {
    let fixture = hosted::Fixture::new().await;
    fixture.state.lock().unwrap().reject_upload = true;
    assert!(!fixture
        .invoke(&["query", "test-plan", "--shards", "2", "--reference", "ref"])
        .await
        .status
        .success());
    fixture.state.lock().unwrap().reject_upload = false;
    fixture.plan().await;
    let before = std::fs::read(fixture.workspace.path().join("ran")).unwrap();
    let batch = fixture.state.lock().unwrap().modules[0].clone();
    for assigned in [
        vec!["unknown-batch".to_string()],
        vec![batch.clone(), batch],
    ] {
        fixture.state.lock().unwrap().assigned = Some(assigned);
        assert!(!fixture
            .invoke(&["test", "--shard-plan", hosted::ID, "--shard-index", "0"])
            .await
            .status
            .success());
        assert_eq!(
            std::fs::read(fixture.workspace.path().join("ran")).unwrap(),
            before
        );
    }
    fixture.state.lock().unwrap().assigned = None;
    {
        let mut state = fixture.state.lock().unwrap();
        let mut artifact: Value = serde_json::from_slice(&state.artifact).unwrap();
        artifact["shardPlanID"] = "01900000-0000-7000-8000-000000000002".into();
        state.artifact = serde_json::to_vec(&artifact).unwrap();
    }
    let output = fixture
        .invoke(&["test", "--shard-plan", hosted::ID, "--shard-index", "0"])
        .await;
    assert!(!output.status.success());
    assert_eq!(
        fixture.state.lock().unwrap().reports.last().unwrap()["status"],
        "failure"
    );
}
