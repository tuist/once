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
    /// `.fabrik/out/<package>/<name>`. For a `cargo_binary`, the
    /// action is `cargo build --locked --package <pkg> --bin <bin>`.
    /// The verb is uniform across target kinds: target-specific
    /// composition lives in build-file declarations, not in the CLI.
    Run {
        /// Target label, e.g. `//examples/hello:hello` or `//:hello`.
        label: String,
    },

    /// Build a target via the granular per-crate action graph.
    ///
    /// Walks the target's transitive Rust deps, compiles each into
    /// its own rustc action, and runs them through the cache-aware
    /// scheduler. Each crate is its own cache slot, so a one-line
    /// edit to a leaf crate invalidates only the affected nodes.
    /// Use `fabrik run` instead for `cargo_binary` and other
    /// single-action targets.
    Build {
        /// Target label, e.g. `//crates/fabrik-cli:fabrik`.
        label: String,
    },

    /// Build and execute a Rust test target.
    ///
    /// Compiles the target's transitive Rust deps with the granular
    /// planner, then runs the produced test binary as its own cached
    /// action. Extra args after the label are passed to the Rust test
    /// harness, for example `fabrik test //pkg:pkg_test -- --nocapture`.
    Test {
        /// Rust test target label, e.g. `//crates/foo:foo_test`.
        label: String,

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

    /// Generate `vendor/fabrik.star` from the workspace's Cargo.lock.
    ///
    /// Reads `cargo metadata` for the resolve graph and emits one
    /// `cargo_crate(...)` declaration per pure-rust dependency,
    /// pointing at the cargo registry source cache. Crates that need
    /// a `build.rs` are commented out (the granular pipeline does not
    /// yet thread build-script outputs through dependent rustc
    /// invocations); use the `cargo_binary` escape hatch for those.
    Vendor,

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
