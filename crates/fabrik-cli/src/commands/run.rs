//! `fabrik run` — execute a single command through the action cache.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use fabrik_cas::Cas;
use fabrik_core::{Action, CacheState, RunOpts, WorkspacePath};
use tokio::io::AsyncWriteExt;

use crate::cli::exit_from;

#[allow(clippy::too_many_arguments)]
pub async fn run_command(
    workspace: &Path,
    cas: &Cas,
    env: Vec<(String, String)>,
    cwd: Option<WorkspacePath>,
    timeout_ms: Option<u64>,
    cache_failures: bool,
    argv: Vec<String>,
) -> Result<ExitCode> {
    let action = Action::RunCommand {
        argv,
        env: env.into_iter().collect::<BTreeMap<_, _>>(),
        cwd,
        timeout_ms,
    };
    let opts = RunOpts { cache_failures };
    let outcome = fabrik_core::run(&action, workspace, cas, opts)
        .await
        .context("executing action")?;

    let stdout = cas.get_blob(&outcome.result.stdout).await?;
    let stderr = cas.get_blob(&outcome.result.stderr).await?;
    // tokio::io::stdout/stderr are line-buffered. Flush explicitly so
    // the bytes reach the pipe before the process exits; without this,
    // captured output is empty under timing pressure (we observed this
    // as flaky shellspec failures on macOS CI).
    let mut out = tokio::io::stdout();
    out.write_all(&stdout).await?;
    out.flush().await?;
    let mut err = tokio::io::stderr();
    err.write_all(&stderr).await?;

    let tag = match outcome.cache {
        CacheState::Hit => "hit",
        CacheState::Miss => "miss",
    };
    let trailer = format!(
        "fabrik: cache {tag} action={} exit={}\n",
        outcome.action, outcome.result.exit_code
    );
    err.write_all(trailer.as_bytes()).await?;
    err.flush().await?;

    Ok(exit_from(outcome.result.exit_code))
}
