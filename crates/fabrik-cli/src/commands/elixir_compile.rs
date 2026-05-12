//! `fabrik elixir-compile` - the wrapper tool that every elixir build
//! action invokes.
//!
//! When a daemon socket is reachable (via `FABRIK_ELIXIR_DAEMON_SOCKET`
//! or the workspace default), the wrapper sends the compile job over
//! the socket so it lands inside the warm BEAM. When no daemon is
//! running, the wrapper exec()s `elixirc` directly, matching the
//! pre-daemon behavior so caching stays correct either way.
//!
//! The action argv stays the same in both modes; daemon presence is
//! invisible to the cache key. Outputs must therefore be byte-identical
//! across backends. `Code.compile_file/2` and `elixirc` agree on .beam
//! contents given the same sources and dep code path, so this contract
//! holds in practice.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use fabrik_elixir::daemon::{self, ClientError};
use fabrik_elixir::protocol::CompileRequest;

#[derive(Debug, clap::Args)]
pub struct ElixirCompileArgs {
    /// Workspace-relative output `.ebin` directory.
    #[arg(long, value_name = "DIR")]
    pub out: String,

    /// Workspace-relative dep `.ebin` directories to put on the BEAM
    /// code path. Repeatable.
    #[arg(long = "pa", value_name = "DIR")]
    pub pa: Vec<String>,

    /// Workspace-relative `.ex` source files to compile.
    #[arg(required = true)]
    pub srcs: Vec<String>,
}

pub fn run(workspace: &Path, args: &ElixirCompileArgs) -> Result<ExitCode> {
    let req = CompileRequest::new(
        next_request_id(),
        workspace.to_string_lossy().into_owned(),
        args.out.clone(),
        args.pa.clone(),
        args.srcs.clone(),
    );
    let socket = socket_path(workspace);

    match daemon::submit(&socket, &req) {
        Ok(resp) if resp.ok => Ok(ExitCode::SUCCESS),
        Ok(resp) => {
            eprintln!(
                "fabrik elixir-compile: daemon at {} reported failure",
                socket.display()
            );
            if let Some(err) = &resp.error {
                eprintln!("{err}");
            }
            Ok(ExitCode::from(1))
        }
        Err(ClientError::NotRunning { .. }) => fall_back_to_elixirc(workspace, args),
        Err(other) => {
            eprintln!("fabrik elixir-compile: {other}");
            eprintln!(
                "fabrik elixir-compile: falling back to direct elixirc spawn for this action"
            );
            fall_back_to_elixirc(workspace, args)
        }
    }
}

/// When no daemon answers, exec `elixirc` with the same arguments. The
/// outputs are identical, but cold-start cost lands on every action.
fn fall_back_to_elixirc(workspace: &Path, args: &ElixirCompileArgs) -> Result<ExitCode> {
    use std::process::Command;

    let elixirc = fabrik_core::workspace_tool(workspace, "elixirc")?;
    let mut cmd = Command::new(&elixirc);
    cmd.arg("-o").arg(&args.out);
    for dir in &args.pa {
        cmd.arg("-pa").arg(dir);
    }
    for src in &args.srcs {
        cmd.arg(src);
    }
    cmd.current_dir(workspace);
    let status = cmd
        .status()
        .map_err(|source| anyhow::anyhow!("failed to spawn `{elixirc}`: {source}"))?;
    Ok(crate::cli::exit_from(status.code().unwrap_or(1)))
}

fn socket_path(workspace: &Path) -> PathBuf {
    if let Ok(env) = std::env::var(daemon::SOCKET_ENV_VAR) {
        if !env.is_empty() {
            return PathBuf::from(env);
        }
    }
    daemon::default_socket_path(workspace)
}

/// Per-process counter for request ids. The ids only need to be unique
/// within a connection; sequential is plenty.
fn next_request_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
