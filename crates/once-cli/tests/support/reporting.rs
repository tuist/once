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
