//! CLI argument parsing - the `clap` types and the small helpers they use.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use once_core::{SandboxMode, WorkspacePath};

mod auth;
mod cache;
mod edit;
mod query;
mod runtime;
mod toolchain;

pub use auth::AuthCmd;
pub use cache::{CacheActionCmd, CacheBlobCmd, CacheCmd};
pub use edit::EditCmd;
pub use query::QueryCmd;
pub use runtime::RuntimeCmd;
pub use toolchain::ToolchainCmd;

/// Workspace-relative directory holding Once's CAS, action results,
/// runtime state, and action results. Hidden so VCS and editors ignore
/// it by default.
pub const CACHE_DIR: &str = ".once";

/// Output format for verbs that emit Once's own structured data
/// (`cache stats`, `run`, `exec` trailers). `human` is the
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
    /// When true, suppress human-mode success and progress trailers.
    /// Errors and the structured
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

/// Release pipeline sets `ONCE_VERSION` at build time so the binary
/// reports the actual release tag rather than the pre-1.0 root package
/// version. Falls back to the Cargo version for local dev builds.
pub const CLI_VERSION: &str = match option_env!("ONCE_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Parser)]
#[command(
    name = "once",
    version = CLI_VERSION,
    about = "Graph-aware, cacheable, remotely-executable repository automation",
    arg_required_else_help = true
)]
pub struct Cli {
    /// Project root. Defaults to the current directory; the cache
    /// lives under `<project>/.once/`. Mirrors `make -C`.
    #[arg(short = 'C', long = "directory", global = true, value_name = "DIR")]
    pub directory: Option<PathBuf>,

    /// Output format for Once's structured data (`cache
    /// stats`, `run`/`exec` trailers). Defaults to a human-readable
    /// rendering; pass `json` or `toon` to get machine-parseable
    /// output for scripting and for agent consumers.
    #[arg(long, global = true, value_enum, default_value_t = Format::Human)]
    pub format: Format,

    /// Increase log verbosity. Repeat for more (-v: info, -vv: debug,
    /// -vvv: trace). Overridden by `RUST_LOG`.
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Suppress human-mode success and progress trailers. Errors and the structured
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
    /// Build a declared target.
    ///
    /// Resolves the target id against the workspace graph, ensures
    /// every transitive dep is built first, and executes the target's
    /// `build` capability through the action cache. Targets that
    /// match a cached action key reuse the prior outputs; everything
    /// else runs and lands its declared outputs in
    /// `<workspace>/.once/out/<target>/`. Use `once query targets` to
    /// list available ids.
    #[command(arg_required_else_help = true)]
    Build {
        /// Local filesystem sandbox policy for command actions.
        #[arg(long, value_parser = parse_sandbox_mode, default_value = "off")]
        sandbox: SandboxMode,

        /// Target id, such as `services/api/Api` or `./Api`.
        #[arg(required_unless_present = "list")]
        target: Option<String>,
    },

    /// Run a declared target.
    ///
    /// Resolves the target id against the workspace graph and executes
    /// its `run` capability through the action cache. Use `--remote`
    /// to ask a compute provider to execute the command.
    #[command(arg_required_else_help = true)]
    Run {
        /// Local filesystem sandbox policy for command actions.
        #[arg(long, value_parser = parse_sandbox_mode, default_value = "off")]
        sandbox: SandboxMode,

        /// Ask graph target kinds to open a visible runtime interface when supported.
        #[arg(long)]
        visible: bool,

        /// Serve a local JSON-RPC runtime control socket for this run.
        #[arg(long)]
        runtime_rpc: bool,

        /// Runtime RPC socket path. Defaults to
        /// `.once/runtime/<session>/control.sock`.
        #[arg(long)]
        runtime_rpc_socket: Option<PathBuf>,

        /// Run the target's action on a compute provider.
        #[arg(long)]
        remote: bool,

        /// Compute provider used with --remote. Defaults to the configured execution provider.
        #[arg(long, value_name = "PROVIDER")]
        compute: Option<String>,

        /// Target id, e.g. `examples/hello/hello` or `./hello`.
        #[arg(required_unless_present = "list")]
        target: Option<String>,
    },

    /// Test a declared target.
    ///
    /// Builds the target as needed, then executes its `test`
    /// capability through the action cache. Output paths and result
    /// groups are owned by the target kind that exposes the capability.
    #[command(arg_required_else_help = true)]
    Test {
        /// Local filesystem sandbox policy for command actions.
        #[arg(long, value_parser = parse_sandbox_mode, default_value = "off")]
        sandbox: SandboxMode,

        /// Target id, such as `tests/unit` or `./unit`.
        #[arg(required_unless_present = "list")]
        target: Option<String>,
    },

    /// Execute a literal action through the cache.
    ///
    /// Low-level action surface for direct commands and script
    /// adapters. The cache key is the full argv, declared environment
    /// variables, optional working directory, and optional timeout. A
    /// second invocation with the same key reuses the captured stdout,
    /// stderr, and exit code. With `--script`, or when argv looks like
    /// `<runtime> <script> [args...]` and the file has `once`
    /// headers, Once applies script-aware parsing instead.
    #[command(arg_required_else_help = true)]
    Exec {
        /// Local filesystem sandbox policy for the command action.
        #[arg(long, value_parser = parse_sandbox_mode, default_value = "off")]
        sandbox: SandboxMode,

        /// Interpret argv as `<runtime> <script> [args...]` and apply
        /// `once` headers from the script file. Useful as the
        /// explicit form, for example `once exec --script bash
        /// scripts/build.sh`, and for directly executable scripts via
        /// a shebang such as `#!/usr/bin/env -S once exec -- bash`.
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

        /// Run the command on a compute provider.
        #[arg(long)]
        remote: bool,

        /// Compute provider used with --remote. Defaults to the configured execution provider.
        #[arg(long, value_name = "PROVIDER")]
        compute: Option<String>,

        /// Command and arguments. Use `--` to separate from once flags.
        #[arg(trailing_var_arg = true, required_unless_present = "list")]
        argv: Vec<String>,
    },

    /// Cache management.
    ///
    /// Inspect, read, and write the content-addressed cache that
    /// every Once action runs through. `cache stats` reports counts
    /// and on-disk size; `cache blob` and `cache action` expose the
    /// CAS and action-result tables as primitives for debugging,
    /// reproducibility checks, and external tooling. Useful for
    /// answering "did this run hit the cache?" without scraping
    /// command output.
    #[command(arg_required_else_help = true)]
    Cache {
        #[command(subcommand)]
        cmd: Option<CacheCmd>,
    },

    /// Authenticate with a configured provider.
    ///
    /// Stores or revokes the credentials Once uses when talking to
    /// remote cache providers (e.g. Tuist). `auth login` walks
    /// through a provider's OAuth or token flow and saves the result
    /// in the OS keychain; `auth logout` drops the stored token. The
    /// cache provider configuration itself lives in workspace
    /// `once.toml`.
    #[command(arg_required_else_help = true)]
    Auth {
        #[command(subcommand)]
        cmd: Option<AuthCmd>,
    },

    /// Inspect the project toolchain contract.
    ///
    /// Reports the toolchains a project pins (Rust, Swift, mise) and
    /// the resolved versions Once will use when running actions from
    /// script adapters or graph target kinds. Pair with `once query schema`
    /// when debugging "why did the cache miss?" questions where the
    /// toolchain identity is suspect.
    #[command(arg_required_else_help = true)]
    Toolchain {
        #[command(subcommand)]
        cmd: Option<ToolchainCmd>,
    },

    /// Query the typed build graph
    ///
    /// Inspectable-first surface for humans and agents. `query
    /// targets` lists every declared target id with its target kind
    /// and capabilities; `query capabilities` shows what a specific
    /// target exposes (`build`, `run`, `test`); `query schema`
    /// returns the typed attribute and provider shape for a target kind;
    /// `query example` materializes a chosen starter; and `query evidence`
    /// lists durable action evidence captured from prior executions. A quoted
    /// `MATCH ... RETURN ...` expression can explore the graph through
    /// a read-only Cypher-like pattern. All query surfaces respect
    /// `--format json` and `--format toon` so consumers can plan
    /// against the graph without scraping prose.
    ///
    /// ## Query Expressions
    ///
    /// `once query '<QUERY>'` accepts a read-only subset of Cypher backed
    /// by the Cypher tree-sitter grammar. The first supported shape is a
    /// single `MATCH` pattern with optional `WHERE` equality predicates
    /// and explicit `RETURN` projections.
    ///
    /// ```sh
    /// once query 'MATCH (app:Target {id: "services/api/Api"})-[:DEPENDS_ON*]->(dep:Target) RETURN dep.id, dep.kind'
    /// once query 'MATCH (t:Target)-[:EXPOSES]->(c:Capability {name: "test"}) RETURN t.id'
    /// ```
    ///
    /// Supported labels are `Target`, `Capability`, and `Provider`. Labels
    /// use the `:Label` form, for example `(t:Target)`. Bare node names
    /// without a colon are aliases, so `(Target)` binds a variable named
    /// `Target` instead of filtering by the `Target` label. Supported
    /// relationships are `DEPENDS_ON`, `EXPOSES`, and `EMITS`. The `*`
    /// suffix on a relationship performs transitive traversal, for example
    /// `[:DEPENDS_ON*]`.
    ///
    /// String literals can be quoted with single or double quotes and
    /// support `\n`, `\r`, `\t`, `\\`, `\"`, and `\'` escapes. Other
    /// escape forms, including Unicode escapes, are rejected.
    #[command(arg_required_else_help = true, verbatim_doc_comment)]
    Query {
        /// Read-only Cypher-like graph query expression.
        #[arg(value_name = "QUERY")]
        expression: Option<String>,
        #[command(subcommand)]
        cmd: Option<QueryCmd>,
    },

    /// Runtime session inspection and control.
    ///
    /// Starts long-lived target runs under a small supervisor and
    /// persists their stdout, stderr, and status under
    /// `<workspace>/.once/runtime/<session>/`. `runtime status`,
    /// `runtime logs`, and `runtime stop` let agents and humans
    /// observe or stop a run after the original command has returned.
    /// `runtime rpc` serves a JSON-RPC control socket for a session
    /// that already has runtime metadata.
    #[command(arg_required_else_help = true)]
    Runtime {
        #[command(subcommand)]
        cmd: Option<RuntimeCmd>,
    },

    /// Mutate workspace manifests.
    ///
    /// `edit apply` runs a batch of `create` / `update` / `delete`
    /// operations against a single `once.toml` atomically. The CLI
    /// reads its input JSON from `--file` or stdin and emits
    /// structured diagnostics for failed edits.
    #[command(arg_required_else_help = true)]
    Edit {
        #[command(subcommand)]
        cmd: Option<EditCmd>,
    },

    /// Expose Once's graph and memory queries to a coding agent over MCP.
    ///
    /// Speaks the Model Context Protocol over stdio so an agent host
    /// (Claude Desktop, an IDE plug-in, the Anthropic SDK) can call
    /// `once_query_targets`, `once_query_capabilities`,
    /// `once_query_schema`, `once_query_example`, and
    /// `once_query_evidence` as tools and get JSON back without
    /// scraping prose. Mounts inspection tools by default; pass
    /// `--allow-run` to expose side-effectful build, run, and runtime
    /// session tools.
    Mcp {
        /// Workspace root the MCP tools resolve targets against.
        /// Defaults to the value of the global `-C/--directory` flag
        /// (or the current directory).
        #[arg(long, value_name = "DIR")]
        workspace: Option<PathBuf>,

        /// Advertise and allow side-effectful execution tools.
        #[arg(long)]
        allow_run: bool,
    },

    /// Internal: emit the markdown CLI reference into `out`. Hidden
    /// from `--help` because it is a documentation build hook, not a
    /// user-facing verb. Drives `docs/reference/cli/*.md` so the
    /// website's flag and synopsis sections never drift from the
    /// real clap definitions.
    #[command(hide = true, arg_required_else_help = true)]
    Reference {
        /// Directory to emit per-subcommand markdown files into.
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
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
            Self::Build { .. } => vec!["build"],
            Self::Run { .. } => vec!["run"],
            Self::Exec { .. } => vec!["exec"],
            Self::Test { .. } => vec!["test"],
            Self::Cache { cmd } => {
                let mut path = vec!["cache"];
                if let Some(cmd) = cmd {
                    path.extend(cmd.surface_path());
                }
                path
            }
            Self::Auth { cmd } => {
                let mut path = vec!["auth"];
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
            Self::Query { cmd, .. } => {
                let mut path = vec!["query"];
                if let Some(cmd) = cmd {
                    path.extend(cmd.surface_path());
                }
                path
            }
            Self::Edit { cmd } => {
                let mut path = vec!["edit"];
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
            Self::Mcp { .. } => vec!["mcp"],
            Self::Reference { .. } => vec!["reference"],
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

fn parse_sandbox_mode(raw: &str) -> std::result::Result<SandboxMode, String> {
    raw.parse()
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
