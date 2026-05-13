//! `fabrik elixir-daemon` - lifecycle commands for the compile daemon.
//!
//! `start` materializes the bundled Elixir script under
//! `.fabrik/daemon/` and execs `elixir` on it; the resulting BEAM
//! listens on a unix socket and serves [`fabrik_elixir::protocol`]
//! requests until killed. `status` reports whether the socket exists
//! and is reachable.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use fabrik_elixir::daemon;
use fabrik_elixir::protocol::CompileRequest;

#[derive(Debug, clap::Subcommand)]
pub enum ElixirDaemonCmd {
    /// Start the compile daemon in the foreground.
    ///
    /// The daemon listens on the given socket path (default:
    /// `.fabrik/elixir-daemon.sock`) until terminated. Run it in a
    /// separate terminal or under a process supervisor.
    Start {
        /// Socket path the daemon should listen on. Defaults to
        /// `<workspace>/.fabrik/elixir-daemon.sock`.
        #[arg(long, value_name = "PATH")]
        socket: Option<PathBuf>,
    },
    /// Report whether a daemon is reachable at the given socket.
    Status {
        /// Socket path to probe. Defaults to the workspace default.
        #[arg(long, value_name = "PATH")]
        socket: Option<PathBuf>,
    },
}

pub fn run(workspace: &Path, cmd: ElixirDaemonCmd) -> Result<ExitCode> {
    match cmd {
        ElixirDaemonCmd::Start { socket } => start(workspace, socket),
        ElixirDaemonCmd::Status { socket } => Ok(status(workspace, socket)),
    }
}

fn start(workspace: &Path, socket: Option<PathBuf>) -> Result<ExitCode> {
    use std::process::Command;

    let socket_path = socket.unwrap_or_else(|| daemon::default_socket_path(workspace));
    let script_path = daemon::default_script_path(workspace);
    daemon::materialize_script(&script_path)
        .with_context(|| format!("materializing daemon script at {}", script_path.display()))?;

    let elixir = fabrik_core::workspace_tool(workspace, "elixir")
        .context("resolving `elixir` from the workspace toolchain")?;
    let mut cmd = Command::new(&elixir);
    cmd.arg(&script_path)
        .arg(&socket_path)
        .current_dir(workspace);
    let status = cmd
        .status()
        .with_context(|| format!("failed to spawn `{elixir}`"))?;
    Ok(crate::cli::exit_from(status.code().unwrap_or(1)))
}

fn status(workspace: &Path, socket: Option<PathBuf>) -> ExitCode {
    let socket_path = socket.unwrap_or_else(|| daemon::default_socket_path(workspace));
    if !socket_path.exists() {
        println!(
            "fabrik elixir-daemon: no socket at {} (daemon not running)",
            socket_path.display()
        );
        return ExitCode::from(1);
    }
    // A reachable socket file is necessary but not sufficient; do a
    // zero-source compile to probe round-trip health without doing real
    // work. The daemon accepts an empty srcs list as a no-op success.
    let probe = CompileRequest::new(
        0,
        workspace.to_string_lossy().into_owned(),
        ".fabrik/daemon/probe.ebin".into(),
        Vec::new(),
        Vec::new(),
    );
    match daemon::submit(&socket_path, &probe) {
        Ok(resp) if resp.ok => {
            println!(
                "fabrik elixir-daemon: alive at {} (protocol v{})",
                socket_path.display(),
                resp.v
            );
            ExitCode::SUCCESS
        }
        Ok(resp) => {
            println!(
                "fabrik elixir-daemon: daemon at {} answered but reported a failure: {}",
                socket_path.display(),
                resp.error.unwrap_or_default()
            );
            ExitCode::from(1)
        }
        Err(err) => {
            println!(
                "fabrik elixir-daemon: socket at {} is unreachable: {err}",
                socket_path.display()
            );
            ExitCode::from(1)
        }
    }
}
