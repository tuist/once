use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::cli::CACHE_DIR;

const STOP_REQUEST_FILE: &str = "stop.requested";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RuntimeSessionRecord {
    pub(crate) session_id: String,
    pub(crate) target: String,
    pub(crate) status: String,
    pub(crate) workspace: String,
    pub(crate) session_dir: String,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) supervisor_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pid: Option<u32>,
    pub(crate) started_at_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) finished_at_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RuntimeLogsRecord {
    pub(crate) session_id: String,
    pub(crate) records: Vec<RuntimeLogRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RuntimeLogRecord {
    pub(crate) cursor: String,
    pub(crate) source: String,
    pub(crate) level: String,
    pub(crate) message: String,
}

pub(crate) fn start_session(workspace: &Path, target: &str) -> Result<RuntimeSessionRecord> {
    let session_id = session_id(target);
    let session_dir = workspace.join(CACHE_DIR).join("runtime").join(&session_id);
    fs::create_dir_all(&session_dir)
        .with_context(|| format!("creating runtime session {}", session_dir.display()))?;
    File::create(session_dir.join("stdout.log"))
        .with_context(|| format!("creating {}", session_dir.join("stdout.log").display()))?;
    File::create(session_dir.join("stderr.log"))
        .with_context(|| format!("creating {}", session_dir.join("stderr.log").display()))?;

    let mut record = RuntimeSessionRecord {
        session_id,
        target: target.to_string(),
        status: "starting".to_string(),
        workspace: ".".to_string(),
        session_dir: display_path(workspace, &session_dir),
        stdout: display_path(workspace, &session_dir.join("stdout.log")),
        stderr: display_path(workspace, &session_dir.join("stderr.log")),
        supervisor_pid: None,
        pid: None,
        started_at_ms: now_ms(),
        finished_at_ms: None,
        exit_code: None,
    };
    write_session_record(&session_dir, &record)?;

    let mut supervisor = Command::new(std::env::current_exe()?);
    supervisor
        .arg("-C")
        .arg(workspace)
        .arg("runtime")
        .arg("supervise")
        .arg("--session-dir")
        .arg(&session_dir)
        .arg("--target")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = supervisor.spawn().context("spawning runtime supervisor")?;
    record.supervisor_pid = Some(child.id());
    thread::spawn(move || {
        let _ = child.wait();
    });
    write_session_record(&session_dir, &record)?;
    Ok(record)
}

pub(crate) fn status_session(workspace: &Path, session: &str) -> Result<RuntimeSessionRecord> {
    let dir = resolve_session_dir(workspace, session)?;
    read_session_record(&dir)
}

pub(crate) fn logs_session(
    workspace: &Path,
    session: &str,
    source: Option<&str>,
    cursor: Option<&str>,
    limit: Option<usize>,
) -> Result<RuntimeLogsRecord> {
    if let Some(source) = source {
        anyhow::ensure!(
            matches!(source, "stdout" | "stderr"),
            "runtime log source must be `stdout` or `stderr`"
        );
    }
    let dir = resolve_session_dir(workspace, session)?;
    let session_id = read_session_record(&dir)?.session_id;
    let mut records = Vec::new();
    for stream in ["stdout", "stderr"] {
        if source.is_some_and(|wanted| wanted != stream) {
            continue;
        }
        let path = dir.join(format!("{stream}.log"));
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        for (index, line) in content.lines().enumerate() {
            let cursor_value = format!("{stream}:{index:012}");
            if cursor.is_some_and(|cursor| cursor_value.as_str() <= cursor) {
                continue;
            }
            records.push(RuntimeLogRecord {
                cursor: cursor_value,
                source: stream.to_string(),
                level: if stream == "stderr" { "error" } else { "info" }.to_string(),
                message: line.to_string(),
            });
            if records.len() >= limit.unwrap_or(200) {
                return Ok(RuntimeLogsRecord {
                    session_id,
                    records,
                });
            }
        }
    }
    Ok(RuntimeLogsRecord {
        session_id,
        records,
    })
}

pub(crate) fn stop_session(workspace: &Path, session: &str) -> Result<RuntimeSessionRecord> {
    let dir = resolve_session_dir(workspace, session)?;
    let mut record = read_session_record(&dir)?;
    if matches!(record.status.as_str(), "starting" | "running") {
        fs::write(dir.join(STOP_REQUEST_FILE), b"stop\n")
            .with_context(|| format!("writing {}", dir.join(STOP_REQUEST_FILE).display()))?;
        record.status = "stopping".to_string();
        write_session_record(&dir, &record)?;
    }
    Ok(record)
}

pub(crate) fn supervise_session(workspace: &Path, session_dir: &Path, target: &str) -> Result<()> {
    let mut record = read_session_record(session_dir)?;
    let stdout = log_file(session_dir, "stdout")?;
    let stderr = log_file(session_dir, "stderr")?;
    let mut child = Command::new(std::env::current_exe()?)
        .arg("-C")
        .arg(workspace)
        .arg("-q")
        .arg("run")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| format!("starting runtime target `{target}`"))?;

    record.status = "running".to_string();
    record.pid = Some(child.id());
    write_session_record(session_dir, &record)?;

    loop {
        if session_dir.join(STOP_REQUEST_FILE).exists() {
            let _ = child.kill();
            let status = child.wait().context("waiting for stopped runtime target")?;
            record.status = "stopped".to_string();
            record.exit_code = status.code();
            record.finished_at_ms = Some(now_ms());
            write_session_record(session_dir, &record)?;
            return Ok(());
        }
        if let Some(status) = child.try_wait().context("polling runtime target")? {
            record.status = if status.success() {
                "exited".to_string()
            } else {
                "failed".to_string()
            };
            record.exit_code = status.code();
            record.finished_at_ms = Some(now_ms());
            write_session_record(session_dir, &record)?;
            return Ok(());
        }
        thread::sleep(Duration::from_millis(200));
    }
}

fn log_file(session_dir: &Path, source: &str) -> Result<File> {
    let path = session_dir.join(format!("{source}.log"));
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))
}

fn resolve_session_dir(workspace: &Path, session: &str) -> Result<PathBuf> {
    anyhow::ensure!(
        !session.is_empty()
            && session.chars().all(
                |character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            ),
        "runtime session id must contain only letters, numbers, `-`, or `_`"
    );
    Ok(workspace.join(CACHE_DIR).join("runtime").join(session))
}

fn read_session_record(session_dir: &Path) -> Result<RuntimeSessionRecord> {
    let path = session_dir.join("session.json");
    let raw = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

fn write_session_record(session_dir: &Path, record: &RuntimeSessionRecord) -> Result<()> {
    let path = session_dir.join("session.json");
    let raw = serde_json::to_vec_pretty(record)?;
    fs::write(&path, raw).with_context(|| format!("writing {}", path.display()))
}

fn session_id(target: &str) -> String {
    let target = target
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("{}-{}-{}", target, std::process::id(), now_ms())
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn display_path(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn generated_session_id_is_path_safe() {
        let id = session_id("apps/ios/App Tests");
        assert!(id.starts_with("apps-ios-App-Tests-"));
        assert!(!id.contains('/'));
        assert!(!id.contains(' '));
    }

    #[test]
    fn session_paths_reject_workspace_escapes() {
        let workspace = Path::new("/workspace");

        for session in [
            "",
            "../outside",
            "nested/session",
            r"nested\session",
            "/absolute",
            "with.dot",
        ] {
            assert!(resolve_session_dir(workspace, session).is_err());
        }
        assert_eq!(
            resolve_session_dir(workspace, "safe-session_1").unwrap(),
            Path::new("/workspace/.once/runtime/safe-session_1")
        );
    }

    #[test]
    fn logs_session_filters_by_source_cursor_and_limit() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        let session_dir = workspace.join(CACHE_DIR).join("runtime").join("s1");
        fs::create_dir_all(&session_dir).unwrap();
        let record = RuntimeSessionRecord {
            session_id: "s1".to_string(),
            target: "tool".to_string(),
            status: "exited".to_string(),
            workspace: ".".to_string(),
            session_dir: ".once/runtime/s1".to_string(),
            stdout: ".once/runtime/s1/stdout.log".to_string(),
            stderr: ".once/runtime/s1/stderr.log".to_string(),
            supervisor_pid: None,
            pid: None,
            started_at_ms: 1,
            finished_at_ms: Some(2),
            exit_code: Some(0),
        };
        write_session_record(&session_dir, &record).unwrap();
        fs::write(session_dir.join("stdout.log"), "one\ntwo\nthree\n").unwrap();
        fs::write(session_dir.join("stderr.log"), "err\n").unwrap();

        let logs = logs_session(
            workspace,
            "s1",
            Some("stdout"),
            Some("stdout:000000000000"),
            Some(1),
        )
        .unwrap();

        assert_eq!(logs.records.len(), 1);
        assert_eq!(logs.records[0].cursor, "stdout:000000000001");
        assert_eq!(logs.records[0].message, "two");
    }

    #[test]
    fn logs_session_rejects_unknown_source() {
        let tmp = TempDir::new().unwrap();
        let error = logs_session(tmp.path(), "s1", Some("combined"), None, None).unwrap_err();
        assert!(error
            .to_string()
            .contains("runtime log source must be `stdout` or `stderr`"));
    }
}
