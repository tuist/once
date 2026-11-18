use super::*;

const DASHBOARD: &str = "https://dashboard.example/runs/canonical";

#[tokio::test]
async fn active_build_has_a_clickable_dashboard_link_then_cancellation_status() {
    let collector = Collector::default();
    collector.recorded.lock().unwrap().dashboard_url = DASHBOARD.into();
    let url = serve(collector.clone()).await;
    let workspace = workspace(Some(&url));
    let home = tempfile::tempdir().unwrap();
    terminal::graph_workspace(workspace.path(), true);
    let mut command = terminal::command(workspace.path(), home.path());
    command
        .env("TUIST_TOKEN", "test-token")
        .args(["build", "Task"]);
    let process = terminal::PtyProcess::spawn(command);
    wait_until(Duration::from_secs(15), || {
        process
            .output()
            .contains("\x1b]8;;https://dashboard.example/runs/canonical\x1b\\")
    })
    .await;
    assert!(collector.started());
    assert!(collector.completions().is_empty());
    assert!(process.output().contains("state=working"));
    let status = Command::new("kill")
        .args(["-s", "INT", &process.child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let (status, output) = tokio::task::spawn_blocking(move || process.finish())
        .await
        .unwrap();
    assert_eq!(status.signal(), Some(libc::SIGINT));
    assert!(output.contains("state=idle"), "{output:?}");
    assert!(!output.contains("state=done"));
    assert_eq!(
        output
            .matches("\x1b]8;;https://dashboard.example/runs/canonical")
            .count(),
        1
    );
    assert_eq!(
        output.matches("\x1b[?2026h").count(),
        output.matches("\x1b[?2026l").count()
    );
    assert_eq!(collector.completions().len(), 1);
    assert_eq!(
        collector.completions()[0].result,
        RunResult::Cancelled as i32
    );
}

#[tokio::test]
async fn fast_runs_keep_plain_dashboard_links_in_logs_and_respect_quiet() {
    let collector = Collector::default();
    collector.recorded.lock().unwrap().dashboard_url = DASHBOARD.into();
    let url = serve(collector).await;
    let workspace = workspace(Some(&url));
    let home = tempfile::tempdir().unwrap();
    for (quiet, format) in [
        (false, "human"),
        (false, "json"),
        (true, "human"),
        (true, "json"),
    ] {
        let mut command = terminal::command(workspace.path(), home.path());
        command.env("TUIST_TOKEN", "test-token");
        if quiet {
            command.arg("--quiet");
        }
        command.args(["--format", format, "exec", "--", "true"]);
        let output = tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(stderr.contains(DASHBOARD), !quiet, "{stderr}");
        for sequence in ["\x1b]8;", "\x1b]7501;", "\x1b[?2026"] {
            assert!(!stderr.contains(sequence), "{stderr:?}");
        }
    }
}

#[tokio::test]
async fn portable_schedule_attributes_each_batch_and_forwards_cancellation() {
    let collector = Collector::default();
    let url = serve(collector.clone()).await;
    let workspace = workspace(Some(&url));
    let home = tempfile::tempdir().unwrap();
    terminal::graph_workspace(workspace.path(), true);
    let mut query = terminal::command(workspace.path(), home.path());
    query
        .args(["query", "test-plan", "--target", "Task", "--format", "json"])
        .stdout(Stdio::piped());
    let output = tokio::task::spawn_blocking(move || query.output().unwrap())
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let plan_path = workspace.path().join("plan.json");
    std::fs::write(&plan_path, output.stdout).unwrap();
    let group = "01900000-0000-7000-8000-000000000007";
    let mut command = terminal::command(workspace.path(), home.path());
    command
        .env("TUIST_TOKEN", "test-token")
        .args(["test", "--plan"])
        .arg(plan_path)
        .args([
            "--external-group",
            group,
            "--partition-index",
            "7",
            "--jobs",
            "1",
        ]);
    let process = terminal::PtyProcess::spawn(command);
    let probe = collector.clone();
    wait_until(Duration::from_secs(30), || probe.started()).await;
    let contexts = collector
        .recorded
        .lock()
        .unwrap()
        .payloads
        .iter()
        .filter_map(|payload| {
            if let Payload::RunStarted(started) = payload {
                started.test_context.clone()
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0].external_group_id, group);
    assert_eq!(contexts[0].partition_index, 7);
    assert_eq!(contexts[0].plan_id, plan["id"]);
    assert_eq!(contexts[0].batch_id, plan["plan"]["batches"][0]["id"]);
    assert!(Command::new("kill")
        .args(["-s", "TERM", &process.child.id().to_string()])
        .status()
        .unwrap()
        .success());
    let (status, output) = tokio::task::spawn_blocking(move || process.finish())
        .await
        .unwrap();
    assert_eq!(status.signal(), Some(libc::SIGTERM), "{output}");
    assert_eq!(collector.completions().len(), 1);
    assert_eq!(
        collector.completions()[0].result,
        RunResult::Cancelled as i32
    );
    assert_eq!(collector.completions()[0].cancellation_reason, "SIGTERM");
}
