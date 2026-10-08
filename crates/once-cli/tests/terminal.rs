#![cfg(unix)]

#[path = "support/terminal.rs"]
mod support;

use support::{command, graph_workspace, PtyProcess};

fn assert_no_controls(output: &str) {
    for sequence in ["\x1b]7501;", "\x1b]8;", "\x1b[?2026"] {
        assert!(
            !output.contains(sequence),
            "unexpected {sequence:?}: {output:?}"
        );
    }
}

#[test]
fn successful_and_cached_builds_emit_status_and_balanced_frames() {
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    graph_workspace(workspace.path(), false);
    for _ in 0..2 {
        let mut cmd = command(workspace.path(), home.path());
        cmd.args(["--color", "never", "build", "Task"]);
        let (status, output) = PtyProcess::spawn(cmd).finish();
        assert!(status.success(), "{output}");
        assert!(output.contains("state=working:app=once"), "{output:?}");
        assert!(output.contains("state=done:app=once"), "{output:?}");
        assert_eq!(output.matches("7501;").count(), 2, "{output:?}");
        assert!(output.contains("\x1b[?2026h"), "{output:?}");
        assert_eq!(
            output.matches("\x1b[?2026h").count(),
            output.matches("\x1b[?2026l").count()
        );
    }
}

#[test]
fn early_failure_is_reported_and_quiet_keeps_only_invisible_status() {
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    for verb in ["build", "test", "lint", "run"] {
        let mut cmd = command(workspace.path(), home.path());
        cmd.args(["--quiet", verb, "missing"]);
        let (status, output) = PtyProcess::spawn(cmd).finish();
        assert!(!status.success());
        assert!(output.contains("state=working"), "{output:?}");
        assert!(output.contains("state=error"), "{output:?}");
        assert!(!output.contains("\x1b[?2026"), "{output:?}");
    }
}

#[test]
fn machine_dumb_ci_and_disabled_modes_never_emit_controls() {
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    for flags in [
        vec!["--format", "json"],
        vec!["--format", "toon"],
        vec!["--terminal-controls", "never"],
    ] {
        let mut cmd = command(workspace.path(), home.path());
        cmd.args(flags).args(["build", "missing"]);
        let (_, output) = PtyProcess::spawn(cmd).finish();
        assert_no_controls(&output);
    }
    for (key, value) in [("TERM", "dumb"), ("CI", "true")] {
        let mut cmd = command(workspace.path(), home.path());
        cmd.env(key, value).args(["build", "missing"]);
        let (_, output) = PtyProcess::spawn(cmd).finish();
        assert_no_controls(&output);
    }
    let output = command(workspace.path(), home.path())
        .args(["build", "missing"])
        .output()
        .unwrap();
    assert_no_controls(&String::from_utf8_lossy(&output.stderr));
}

#[test]
fn captured_child_output_cannot_spoof_status_or_leave_controls_open() {
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    graph_workspace(workspace.path(), false);
    let module = workspace.path().join("modules/task.star");
    let source = std::fs::read_to_string(&module).unwrap();
    std::fs::write(&module, source.replace(
        "    write_path(out, \"built\")",
        r#"    write_path(out, "built")
    run_action(argv = [host_which("printf"), "\\033]7501;state=done:app=child\\033\\\\\\033]8;;https://child.example\\033\\\\spoof\\033[?2026h"], outputs = [], identifier = "noisy")"#,
    )).unwrap();
    let mut cmd = command(workspace.path(), home.path());
    cmd.args(["-vv", "build", "Task"]);
    let (status, output) = PtyProcess::spawn(cmd).finish();
    assert!(status.success(), "{output}");
    assert_eq!(output.matches("7501;").count(), 2, "{output:?}");
    assert!(!output.contains("app=child"), "{output:?}");
    assert!(
        output.contains("spoof"),
        "noisy action did not run: {output:?}"
    );
    assert!(!output.contains("\x1b]8;;https://child.example"));
    assert_eq!(
        output.matches("\x1b[?2026h").count(),
        output.matches("\x1b[?2026l").count()
    );
}

#[test]
fn verbose_logging_does_not_interrupt_a_synchronized_frame() {
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    graph_workspace(workspace.path(), false);
    let mut cmd = command(workspace.path(), home.path());
    cmd.args(["-vv", "build", "Task"]);
    let (status, output) = PtyProcess::spawn(cmd).finish();
    assert!(status.success(), "{output}");
    let mut synchronized = false;
    for chunk in output.split('\x1b') {
        if chunk.starts_with("[?2026h") {
            assert!(!synchronized);
            synchronized = true;
        }
        if chunk.starts_with("[?2026l") {
            assert!(synchronized);
            synchronized = false;
        }
    }
    assert!(!synchronized);
    assert!(output.contains("state=done"));
}
