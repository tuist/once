use std::path::Path;
use std::process::{Command, Output};

fn invoke(root: &Path, arguments: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_once"));
    command.env_clear();
    for name in ["PATH", "SystemRoot"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .args(arguments)
        .current_dir(root)
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env("LOCALAPPDATA", root)
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .output()
        .unwrap()
}

fn logs(root: &Path) -> Vec<serde_json::Value> {
    [
        root.join("Library/Logs/Once"),
        root.join("state/once/logs"),
        root.join("Once/Logs"),
    ]
    .into_iter()
    .filter(|directory| directory.is_dir())
    .flat_map(|directory| std::fs::read_dir(directory).unwrap().map(Result::unwrap))
    .filter(|entry| {
        entry.file_type().unwrap().is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "log")
    })
    .flat_map(|entry| {
        std::fs::read_to_string(entry.path())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect::<Vec<_>>()
    })
    .collect()
}

fn assert_event_target(records: &[serde_json::Value], message: &str) {
    let event = records
        .iter()
        .find(|record| record["fields"]["message"] == message)
        .unwrap_or_else(|| panic!("missing {message} event in {records:?}"));
    assert_eq!(event["target"], "once");
    assert!(event["fields"]["session_id"].is_string());
}

#[test]
fn early_parse_paths_preserve_exit_codes_and_log_targets() {
    for (arguments, exit_code) in [
        (vec!["--help"], 0),
        (vec!["cache", "--help"], 0),
        (vec!["--version"], 0),
        (vec![], 2),
        (vec!["cache"], 2),
        (vec!["--not-a-once-option"], 2),
    ] {
        let root = tempfile::tempdir().unwrap();
        let output = invoke(root.path(), &arguments);
        assert_eq!(output.status.code(), Some(exit_code), "{arguments:?}");
        let records = logs(root.path());
        assert_event_target(&records, "argument parsing stopped");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("argument parsing stopped"));
    }
}

#[test]
fn successful_dispatch_preserves_session_log_targets() {
    let root = tempfile::tempdir().unwrap();
    let output = invoke(root.path(), &["--list"]);
    assert!(output.status.success());
    assert!(!output.stdout.is_empty());
    let records = logs(root.path());
    assert_event_target(&records, "session started");
    assert_event_target(&records, "session finished");
}

#[test]
fn failed_dispatch_preserves_structured_errors_and_session_log_targets() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing-executable");
    let output = invoke(
        root.path(),
        &["--format", "json", "exec", "--", missing.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("once.error.v1"));
    assert!(stderr.contains(missing.to_str().unwrap()));
    let records = logs(root.path());
    assert_event_target(&records, "session started");
    assert_event_target(&records, "session failed");
}
