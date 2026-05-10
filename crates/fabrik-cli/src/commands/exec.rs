//! `fabrik exec` - cache and execute a literal command.
//!
//! Substrate-level escape hatch: bypass the target graph and put any
//! argv through the action cache. Useful for ad-hoc shell-outs and for
//! exercising the cache directly. Most users want `fabrik run` against
//! a declared target instead.
//!
//! Stdout always carries the wrapped program's stdout verbatim
//! (transparency), regardless of `--format`. Stderr carries the
//! wrapped program's stderr plus a Fabrik trailer; the trailer's
//! shape is human-readable by default and structured under `json` or
//! `toon`.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use fabrik_cas::Cas;
use fabrik_core::{Action, ResourceRequest, RunOpts, WorkspacePath};
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::cli::{exit_from, Format};
use crate::commands::util::cache_tag;
use crate::render;

#[derive(Serialize)]
struct ExecTrailer<'a> {
    action_digest: String,
    cache: &'a str,
    exit_code: i32,
}

/// Inputs to [`exec`], grouped so the function signature stays
/// readable as the verb gains options. Owned types: the call site
/// builds these from clap and hands them over.
pub struct ExecArgs {
    pub env: Vec<(String, String)>,
    pub cwd: Option<WorkspacePath>,
    pub timeout_ms: Option<u64>,
    pub cache_failures: bool,
    pub argv: Vec<String>,
}

pub async fn exec(workspace: &Path, cas: &Cas, args: ExecArgs, format: Format) -> Result<ExitCode> {
    let ExecArgs {
        env,
        cwd,
        timeout_ms,
        cache_failures,
        argv,
    } = args;
    let action = Action::RunCommand {
        argv,
        env: env.into_iter().collect::<BTreeMap<_, _>>(),
        cwd,
        input_digest: None,
        outputs: vec![],
        resources: ResourceRequest::default(),
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

    let tag = cache_tag(outcome.cache);
    let trailer = ExecTrailer {
        action_digest: outcome.action.to_string(),
        cache: tag,
        exit_code: outcome.result.exit_code,
    };
    let trailer = match format {
        Format::Human => format!(
            "fabrik: cache {tag} action={} exit={}\n",
            outcome.action, outcome.result.exit_code
        ),
        Format::Json | Format::Toon => render::structured(format, &trailer)?,
    };
    err.write_all(trailer.as_bytes()).await?;
    err.flush().await?;

    Ok(exit_from(outcome.result.exit_code))
}
