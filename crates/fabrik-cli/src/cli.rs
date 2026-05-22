//! CLI argument parsing - the `clap` types and the small helpers they use.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use fabrik_core::WorkspacePath;

/// Workspace-relative directory holding Fabrik's CAS, action results,
/// and build outputs. Hidden so VCS and editors ignore it by default.
pub const CACHE_DIR: &str = ".fabrik";

/// Output format for verbs that emit Fabrik's own structured data
/// (`targets`, `cache stats`, `run`, `exec` trailers). `human` is the
/// readable default; `json` and `toon` let agents and scripts consume
/// output without scraping prose.
#[derive(Copy, Clone, Debug, ValueEnum, Default, PartialEq, Eq)]
pub enum Format {
    #[default]
    Human,
    Json,
    Toon,
}

/// Output policy passed to command handlers. Bundles the chosen
/// [`Format`] with the global `--quiet` flag so commands have one
/// argument to consult instead of two. Cheap to copy; future flags
/// that affect rendering (e.g. `--no-color`) drop in here.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Output {
    pub format: Format,
    /// When true, suppress human-mode success and progress trailers
    /// (e.g. "fabrik: built ..." lines). Errors and the structured
    /// envelope of `--format json`/`toon` are never suppressed.
    pub quiet: bool,
}

impl Output {
    #[must_use]
    pub fn new(format: Format, quiet: bool) -> Self {
        Self { format, quiet }
    }

    /// Whether human-mode progress and success trailers should print.
    /// Always false in non-human formats, since those don't produce
    /// trailers in the first place; combining the checks here keeps
    /// call sites readable.
    #[must_use]
    pub fn show_human_trailers(self) -> bool {
        self.format == Format::Human && !self.quiet
    }
}

/// Release pipeline sets `FABRIK_VERSION` at build time so the binary
/// reports the actual release tag rather than the pre-1.0 root package
/// version. Falls back to the Cargo version for local dev builds.
pub const CLI_VERSION: &str = match option_env!("FABRIK_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Parser)]
#[command(
    name = "fabrik",
    version = CLI_VERSION,
    about = "Polyglot, agent-native build system",
    arg_required_else_help = true
)]
pub struct Cli {
    /// Project root. Defaults to the current directory; the cache
    /// lives under `<project>/.fabrik/`. Mirrors `make -C`.
    #[arg(short = 'C', long = "directory", global = true, value_name = "DIR")]
    pub directory: Option<PathBuf>,

    /// Output format for Fabrik's structured data (`targets`, `cache
    /// stats`, `run`/`exec` trailers). Defaults to a human-readable
    /// rendering; pass `json` or `toon` to get machine-parseable
    /// output for scripting and for agent consumers.
    #[arg(long, global = true, value_enum, default_value_t = Format::Human)]
    pub format: Format,

    /// Increase log verbosity. Repeat for more (-v: info, -vv: debug,
    /// -vvv: trace). Overridden by `RUST_LOG`.
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Suppress human-mode success and progress trailers ("fabrik:
    /// built ...", per-node hit/miss lines). Errors and the structured
    /// envelope of `--format json`/`toon` still print. Mirrors the
    /// `-q` flag of common build tools.
    #[arg(short = 'q', long, global = true)]
    pub quiet: bool,

    /// Print the command surface at the current command depth.
    #[arg(long, global = true)]
    pub list: bool,

    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Execute the action(s) that produce a target.
    ///
    /// Loads the project, finds the matching target, and
    /// runs its action(s) through the cache. For a `rust_binary`,
    /// that action is the rustc invocation; the binary lands at
    /// `.fabrik/out/<package>/<name>`. For a `cargo_binary`, the
    /// action is `cargo build --locked --package <pkg> --bin <bin>`.
    /// The verb is uniform across target kinds: target-specific
    /// composition lives in build-file declarations, not in the CLI.
    #[command(arg_required_else_help = true)]
    Run {
        /// Serve a local JSON-RPC runtime control socket for this run.
        #[arg(long)]
        runtime_rpc: bool,

        /// Runtime RPC socket path. Defaults to
        /// `.fabrik/runtime/<session>/control.sock`.
        #[arg(long)]
        runtime_rpc_socket: Option<PathBuf>,

        /// Target id, e.g. `examples/hello/hello` or `./hello`.
        #[arg(required_unless_present = "list")]
        target: Option<String>,
    },

    /// Build a target via the granular per-crate action graph.
    ///
    /// Walks the target's transitive Rust deps, compiles each into
    /// its own rustc action, and runs them through the cache-aware
    /// scheduler. Each crate is its own cache slot, so a one-line
    /// edit to a leaf crate invalidates only the affected nodes.
    /// Use `fabrik run` instead for `cargo_binary` and other
    /// single-action targets.
    #[command(arg_required_else_help = true)]
    Build {
        /// Target id, e.g. `crates/fabrik-cli/fabrik`.
        #[arg(required_unless_present = "list")]
        target: Option<String>,
    },

    /// Build and execute a Rust test target.
    ///
    /// Compiles the target's transitive Rust deps with the granular
    /// planner, then runs the produced test binary as its own cached
    /// action. Extra args after the target are passed to the Rust test
    /// harness, for example `fabrik test pkg/pkg_test -- --nocapture`.
    #[command(arg_required_else_help = true)]
    Test {
        /// Rust test target id, e.g. `crates/foo/foo_test`.
        #[arg(required_unless_present = "list")]
        target: Option<String>,

        /// Arguments passed to the compiled test binary.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        test_args: Vec<String>,
    },

    /// Cache and execute a literal command (substrate escape hatch).
    ///
    /// Bypasses the target graph and puts any argv through the action
    /// cache. The cache key is the full argv, declared environment
    /// variables, optional working directory, and optional timeout. A
    /// second invocation with the same key reuses the captured stdout,
    /// stderr, and exit code. With `--script`, or when argv looks like
    /// `<runtime> <script> [args...]` and the file has `FABRIK`
    /// headers, Fabrik applies script-aware parsing instead. Most
    /// users want `fabrik run` against a declared target instead.
    #[command(arg_required_else_help = true)]
    Exec {
        /// Interpret argv as `<runtime> <script> [args...]` and apply
        /// `FABRIK` headers from the script file. Useful as the
        /// explicit form, for example `fabrik exec --script bash
        /// scripts/build.sh`, and for directly executable scripts via
        /// a shebang such as `#!/usr/bin/env -S fabrik exec -- bash`.
        #[arg(long)]
        script: bool,

        /// Pass an environment variable to the command. Repeatable.
        #[arg(short = 'e', value_parser = parse_env)]
        env: Vec<(String, String)>,

        /// Working directory, relative to the project root. Must not
        /// be absolute or escape the project.
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
        #[arg(trailing_var_arg = true, required_unless_present = "list")]
        argv: Vec<String>,
    },

    /// Cache management.
    #[command(arg_required_else_help = true)]
    Cache {
        #[command(subcommand)]
        cmd: Option<CacheCmd>,
    },

    /// Inspect the project toolchain contract.
    #[command(arg_required_else_help = true)]
    Toolchain {
        #[command(subcommand)]
        cmd: Option<ToolchainCmd>,
    },

    /// Runtime session inspection and control.
    #[command(arg_required_else_help = true)]
    Runtime {
        #[command(subcommand)]
        cmd: Option<RuntimeCmd>,
    },

    /// Dependency graph maintenance.
    #[command(arg_required_else_help = true)]
    Deps {
        #[command(subcommand)]
        cmd: Option<DepsCmd>,
    },

    /// Create a new Fabrik project from a vendored template.
    Init(crate::commands::init::InitArgs),

    /// Deprecated alias for `fabrik deps sync`. Removed in v0.8.0.
    #[command(hide = true)]
    Vendor,

    /// List targets declared across the project.
    ///
    /// Walks the project root for build files, evaluates each,
    /// and prints one line per declared target as `<kind> <id>`, in
    /// package then source order. Useful for scripting and for
    /// sanity-checking that build files evaluate before more expensive
    /// commands consume them.
    Targets,

    /// Compile elixir sources via the daemon, or direct elixirc as
    /// fallback. Invoked by elixir build actions; not typically run by
    /// users.
    #[cfg(unix)]
    #[command(name = "elixir-compile")]
    ElixirCompile(crate::commands::elixir_compile::ElixirCompileArgs),

    /// Long-lived compile daemon for elixir targets.
    #[cfg(unix)]
    #[command(name = "elixir-daemon")]
    ElixirDaemon {
        #[command(subcommand)]
        cmd: Option<crate::commands::elixir_daemon::ElixirDaemonCmd>,
    },
}

#[derive(Subcommand)]
pub enum CacheCmd {
    /// Print blob and action counts plus on-disk size.
    Stats,
}

#[derive(Subcommand)]
pub enum ToolchainCmd {
    /// Print the toolchain contract derived from mise.toml.
    Inspect {
        /// Mise platform key to inspect, e.g. linux-x64. Defaults to
        /// the current host platform.
        #[arg(long)]
        platform: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum RuntimeCmd {
    /// Serve the local runtime JSON-RPC endpoint for a session directory.
    Rpc {
        /// Runtime session directory containing session.json and logs.
        session_dir: PathBuf,

        /// Socket path. Defaults to `<session-dir>/control.sock`.
        #[arg(long)]
        socket: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum DepsCmd {
    /// Synchronize generated dependency artifacts from `fabrik.toml`.
    Sync {
        /// Sync one dependency entry by name. Defaults to all entries.
        name: Option<String>,
    },
}

impl Cli {
    pub fn surface_path(&self) -> Vec<&'static str> {
        self.command
            .as_ref()
            .map_or_else(Vec::new, Cmd::surface_path)
    }
}

impl Cmd {
    pub fn surface_path(&self) -> Vec<&'static str> {
        match self {
            Self::Run { .. } => vec!["run"],
            Self::Build { .. } => vec!["build"],
            Self::Test { .. } => vec!["test"],
            Self::Exec { .. } => vec!["exec"],
            Self::Cache { cmd } => {
                let mut path = vec!["cache"];
                if let Some(cmd) = cmd {
                    path.extend(cmd.surface_path());
                }
                path
            }
            Self::Toolchain { cmd } => {
                let mut path = vec!["toolchain"];
                if let Some(cmd) = cmd {
                    path.extend(cmd.surface_path());
                }
                path
            }
            Self::Runtime { cmd } => {
                let mut path = vec!["runtime"];
                if let Some(cmd) = cmd {
                    path.extend(cmd.surface_path());
                }
                path
            }
            Self::Deps { cmd } => {
                let mut path = vec!["deps"];
                if let Some(cmd) = cmd {
                    path.extend(cmd.surface_path());
                }
                path
            }
            Self::Init(_) => vec!["init"],
            Self::Vendor => vec!["vendor"],
            Self::Targets => vec!["targets"],
            #[cfg(unix)]
            Self::ElixirCompile(_) => vec!["elixir-compile"],
            #[cfg(unix)]
            Self::ElixirDaemon { cmd } => {
                let mut path = vec!["elixir-daemon"];
                if let Some(cmd) = cmd {
                    path.extend(cmd.surface_path());
                }
                path
            }
        }
    }
}

impl CacheCmd {
    fn surface_path(&self) -> Vec<&'static str> {
        match self {
            Self::Stats => vec!["stats"],
        }
    }
}

impl ToolchainCmd {
    fn surface_path(&self) -> Vec<&'static str> {
        match self {
            Self::Inspect { .. } => vec!["inspect"],
        }
    }
}

impl RuntimeCmd {
    fn surface_path(&self) -> Vec<&'static str> {
        match self {
            Self::Rpc { .. } => vec!["rpc"],
        }
    }
}

impl DepsCmd {
    fn surface_path(&self) -> Vec<&'static str> {
        match self {
            Self::Sync { .. } => vec!["sync"],
        }
    }
}

#[cfg(unix)]
impl crate::commands::elixir_daemon::ElixirDaemonCmd {
    fn surface_path(&self) -> Vec<&'static str> {
        match self {
            Self::Start { .. } => vec!["start"],
            Self::Status { .. } => vec!["status"],
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_show_human_trailers_only_when_human_and_not_quiet() {
        assert!(Output::new(Format::Human, false).show_human_trailers());
        assert!(!Output::new(Format::Human, true).show_human_trailers());
        // Structured formats never emit human trailers, so quiet has no
        // effect on the predicate either way.
        assert!(!Output::new(Format::Json, false).show_human_trailers());
        assert!(!Output::new(Format::Json, true).show_human_trailers());
        assert!(!Output::new(Format::Toon, false).show_human_trailers());
    }
}
