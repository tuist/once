//! CLI argument parsing — the `clap` types and the small helpers they use.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use fabrik_core::WorkspacePath;

/// Release pipeline sets `FABRIK_VERSION` at build time so the binary
/// reports the actual release tag rather than the pre-1.0
/// workspace.package version. Falls back to the Cargo version for
/// local dev builds.
pub const CLI_VERSION: &str = match option_env!("FABRIK_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Parser)]
#[command(
    name = "fabrik",
    version = CLI_VERSION,
    about = "Polyglot, agent-native build system"
)]
pub struct Cli {
    /// Workspace root. Defaults to the current directory; the cache
    /// lives under `<workspace>/.fabrik/`. Mirrors `make -C`.
    #[arg(short = 'C', long = "directory", global = true, value_name = "DIR")]
    pub directory: Option<PathBuf>,

    /// Increase log verbosity. Repeat for more (-v: info, -vv: debug,
    /// -vvv: trace). Overridden by `RUST_LOG`.
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Execute the action(s) that produce a target.
    ///
    /// Loads the workspace, finds the matching target by label, and
    /// runs its action(s) through the cache. For a `rust_binary`,
    /// that action is the rustc invocation; the binary lands at
    /// `.fabrik/out/<package>/<name>`. The verb is uniform across
    /// target kinds: target-specific composition (e.g. compile then
    /// exec the produced binary) lives in the build-file declarations,
    /// not in the CLI.
    Run {
        /// Target label, e.g. `//examples/hello:hello` or `//:hello`.
        label: String,
    },

    /// Cache and execute a literal command (substrate escape hatch).
    ///
    /// Bypasses the target graph and puts any argv through the action
    /// cache. The cache key is the full argv, declared environment
    /// variables, optional working directory, and optional timeout. A
    /// second invocation with the same key reuses the captured stdout,
    /// stderr, and exit code. Most users want `fabrik run` against a
    /// declared target instead.
    Exec {
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
        /// Off by default; transient failures shouldn't poison the
        /// cache.
        #[arg(long)]
        cache_failures: bool,

        /// Command and arguments. Use `--` to separate from fabrik flags.
        #[arg(trailing_var_arg = true, required = true)]
        argv: Vec<String>,
    },

    /// Cache management.
    Cache {
        #[command(subcommand)]
        cmd: CacheCmd,
    },

    /// List targets declared across the workspace.
    ///
    /// Walks the workspace root for `fabrik.star` files, evaluates each,
    /// and prints one line per declared target as `<kind> <label>`, in
    /// package then source order. Useful for scripting and for
    /// sanity-checking that build files evaluate before more expensive
    /// commands consume them.
    Targets,
}

#[derive(Subcommand)]
pub enum CacheCmd {
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

/// Map a subprocess exit code to a CLI [`ExitCode`].
///
/// `Command::status().code()` returns `None` when the child was killed
/// by a signal; we surface that as 255 (the lowest 8 bits of -1) which
/// is what most build tools do. We do not attempt the shell convention
/// of `128 + signo` since we don't have the signal number on stable
/// Rust without `std::os::unix`-specific code, and pretending otherwise
/// would be misleading.
#[must_use]
pub fn exit_from(code: i32) -> ExitCode {
    let clamped = u8::try_from(code & 0xff).unwrap_or(1);
    ExitCode::from(clamped)
}
