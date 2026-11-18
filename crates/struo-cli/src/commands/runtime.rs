//! Runtime session inspection and control.

mod query;
mod session;
mod supervisor;

#[cfg(unix)]
mod protocol;
#[cfg(unix)]
mod server;

#[cfg(unix)]
pub use server::rpc;
pub(crate) use supervisor::{logs_session, start_session, status_session, stop_session};

use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::cli::{Format, Output};
use crate::render;

#[cfg(not(unix))]
pub async fn rpc(
    _session_dir: &std::path::Path,
    _socket: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    anyhow::bail!("runtime JSON-RPC over Unix sockets is only supported on Unix platforms")
}

pub async fn start(workspace: &Path, output: Output, target: &str) -> Result<ExitCode> {
    let record = start_session(workspace, target)?;
    write_body(output, || render_start_human(&record), &record).await?;
    Ok(ExitCode::SUCCESS)
}

pub async fn status(workspace: &Path, output: Output, session: &str) -> Result<ExitCode> {
    let record = status_session(workspace, session)?;
    write_body(output, || render_status_human(&record), &record).await?;
    Ok(ExitCode::SUCCESS)
}

pub async fn logs(
    workspace: &Path,
    output: Output,
    session: &str,
    source: Option<&str>,
    cursor: Option<&str>,
    limit: Option<usize>,
) -> Result<ExitCode> {
    let record = logs_session(workspace, session, source, cursor, limit)?;
    write_body(output, || render_logs_human(&record), &record).await?;
    Ok(ExitCode::SUCCESS)
}

pub async fn stop(workspace: &Path, output: Output, session: &str) -> Result<ExitCode> {
    let record = stop_session(workspace, session)?;
    write_body(output, || render_status_human(&record), &record).await?;
    Ok(ExitCode::SUCCESS)
}

pub fn supervise(workspace: &Path, session_dir: &Path, target: &str) -> Result<ExitCode> {
    supervisor::supervise_session(workspace, session_dir, target)?;
    Ok(ExitCode::SUCCESS)
}

async fn write_body<T: Serialize>(
    output: Output,
    human: impl FnOnce() -> String,
    value: &T,
) -> Result<()> {
    let body = match output.format {
        Format::Human => human(),
        Format::Json | Format::Toon => render::structured(output.format, value)?,
    };
    let mut out = tokio::io::stdout();
    out.write_all(body.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}

fn render_start_human(record: &supervisor::RuntimeSessionRecord) -> String {
    format!(
        "session: {}\nstatus: {}\ntarget: {}\n",
        record.session_id, record.status, record.target
    )
}

fn render_status_human(record: &supervisor::RuntimeSessionRecord) -> String {
    let mut out = render_start_human(record);
    if let Some(pid) = record.pid {
        let _ = writeln!(out, "pid: {pid}");
    }
    if let Some(code) = record.exit_code {
        let _ = writeln!(out, "exit: {code}");
    }
    out
}

fn render_logs_human(record: &supervisor::RuntimeLogsRecord) -> String {
    let mut out = String::new();
    for entry in &record.records {
        out.push_str(&entry.message);
        out.push('\n');
    }
    out
}
