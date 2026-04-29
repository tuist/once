//! `fabrik` CLI entry point.

use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use fabrik_cas::Cas;
use fabrik_core::{Action, CacheState, RunOpts, WorkspacePath};
use tokio::io::AsyncWriteExt;
use tracing_subscriber::{fmt, EnvFilter};

/// Release pipeline sets `FABRIK_VERSION` at build time so the binary
/// reports the actual release tag rather than the pre-1.0
/// workspace.package version. Falls back to the Cargo version for
/// local dev builds.
const CLI_VERSION: &str = match option_env!("FABRIK_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Parser)]
#[command(
    name = "fabrik",
    version = CLI_VERSION,
    about = "Polyglot, agent-native build system"
)]
struct Cli {
    /// Workspace root. Defaults to the current directory; the cache
    /// lives under `<workspace>/.fabrik/`. Mirrors `make -C`.
    #[arg(short = 'C', long = "directory", global = true, value_name = "DIR")]
    directory: Option<PathBuf>,

    /// Increase log verbosity. Repeat for more (-v: info, -vv: debug,
    /// -vvv: trace). Overridden by `RUST_LOG`.
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a command through the action cache.
    ///
    /// The cache key is the full argv, declared environment variables,
    /// optional working directory, and optional timeout. A second
    /// invocation with the same key reuses the captured stdout, stderr,
    /// and exit code.
    Run {
        /// Pass an environment variable to the command. Repeatable.
        #[arg(short = 'e', value_parser = parse_env)]
        env: Vec<(String, String)>,

        /// Working directory, relative to the workspace root. Must not
        /// be absolute or escape the workspace.
        #[arg(long, value_parser = parse_workspace_path)]
        cwd: Option<WorkspacePath>,

        /// Per-action timeout in milliseconds. The child is killed if
        /// it exceeds the deadline.
        #[arg(long, value_name = "MS")]
        timeout_ms: Option<u64>,

        /// Cache non-zero exits the same way zero exits are cached.
        /// Off by default — transient failures shouldn't poison the
        /// cache.
        #[arg(long)]
        cache_failures: bool,

        /// Command and arguments. Use `--` to separate from fabrik flags.
        #[arg(trailing_var_arg = true, required = true)]
        argv: Vec<String>,
    },

    /// Run `cargo` against the workspace through the action cache.
    ///
    /// First invocation runs cargo end-to-end and captures the result.
    /// Subsequent invocations with the same workspace source digest,
    /// rust+cargo versions, and arguments replay the cached
    /// stdout/stderr/exit instead of re-running.
    ///
    /// This is opaque-mode integration: cargo runs as one unit. cargo's
    /// own incremental compilation still applies on a cache miss.
    /// Future phases will replace this with per-crate `rustc`
    /// invocations driven by `cargo metadata`.
    Cargo {
        /// Cache cargo failures the same way successes are cached. Off
        /// by default so a transient `cargo build` failure doesn't
        /// pin a red state.
        #[arg(long)]
        cache_failures: bool,

        /// Arguments forwarded to cargo. Use `--` to separate from
        /// fabrik flags, e.g. `fabrik cargo -- build --release -p foo`.
        #[arg(trailing_var_arg = true, required = true)]
        args: Vec<String>,
    },

    /// Cache management.
    Cache {
        #[command(subcommand)]
        cmd: CacheCmd,
    },
}

#[derive(Subcommand)]
enum CacheCmd {
    /// Print blob and action counts plus on-disk size.
    Stats,
}

fn parse_env(raw: &str) -> std::result::Result<(String, String), String> {
    let (k, v) = raw
        .split_once('=')
        .ok_or_else(|| format!("expected KEY=VALUE, got `{raw}`"))?;
    Ok((k.to_string(), v.to_string()))
}

fn parse_workspace_path(raw: &str) -> std::result::Result<WorkspacePath, String> {
    WorkspacePath::try_from(raw).map_err(|e| e.to_string())
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    match dispatch(cli).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("fabrik: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn init_tracing(verbose: u8) {
    // RUST_LOG always wins; otherwise -v sets the floor.
    let default = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

async fn dispatch(cli: Cli) -> Result<ExitCode> {
    let workspace = match cli.directory {
        Some(d) => d,
        None => env::current_dir().context("resolving workspace root")?,
    };
    let cas = Cas::open(workspace.join(".fabrik"));

    match cli.command {
        Cmd::Run {
            env,
            cwd,
            timeout_ms,
            cache_failures,
            argv,
        } => run_command(&workspace, &cas, env, cwd, timeout_ms, cache_failures, argv).await,
        Cmd::Cargo {
            cache_failures,
            args,
        } => cargo_command(&workspace, &cas, cache_failures, args).await,
        Cmd::Cache {
            cmd: CacheCmd::Stats,
        } => print_stats(&cas).await.map(|()| ExitCode::SUCCESS),
    }
}

async fn cargo_command(
    workspace: &std::path::Path,
    cas: &Cas,
    cache_failures: bool,
    args: Vec<String>,
) -> Result<ExitCode> {
    let toolchain = fabrik_rust::Toolchain::detect().context("probing rust toolchain")?;
    let action =
        fabrik_rust::cargo_action(workspace, &args, &toolchain).context("building cargo action")?;
    let opts = RunOpts { cache_failures };
    let outcome = fabrik_core::run(&action, workspace, cas, opts)
        .await
        .context("executing cargo action")?;

    let stdout = cas.get_blob(&outcome.result.stdout).await?;
    let stderr = cas.get_blob(&outcome.result.stderr).await?;
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
        "fabrik: cargo cache {tag} action={} exit={}\n",
        outcome.action, outcome.result.exit_code
    );
    err.write_all(trailer.as_bytes()).await?;
    err.flush().await?;

    Ok(exit_from(outcome.result.exit_code))
}

#[allow(clippy::too_many_arguments)]
async fn run_command(
    workspace: &std::path::Path,
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
    // the bytes reach the pipe before the process exits — without this,
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

async fn print_stats(cas: &Cas) -> Result<()> {
    let s = cas.stats().await?;
    let body = format!(
        "blobs:   {} ({} bytes)\nactions: {} ({} bytes)\n",
        s.blob_count, s.blob_bytes, s.action_count, s.action_bytes,
    );
    let mut out = tokio::io::stdout();
    out.write_all(body.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}

/// Map a subprocess exit code to a CLI [`ExitCode`].
///
/// `Command::status().code()` returns `None` when the child was killed
/// by a signal; we surface that as 255 (the lowest 8 bits of -1) which
/// is what most build tools do. We do *not* attempt the shell
/// convention of `128 + signo` — we don't have the signal number on
/// stable Rust without `std::os::unix`-specific code, and pretending
/// otherwise would be misleading.
fn exit_from(code: i32) -> ExitCode {
    let clamped = u8::try_from(code & 0xff).unwrap_or(1);
    ExitCode::from(clamped)
}
