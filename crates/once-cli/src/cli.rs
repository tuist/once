//! Command-line argument parsing and its small helper types.

use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;

use once_core::{LintSeverity, NetworkPolicy, SandboxMode, WorkspacePath};

mod auth;
mod cache;
mod edit;
mod native;
mod query;
mod runtime;
mod toolchain;

pub use auth::AuthCmd;
pub(crate) use cache::OutputDigest;
pub use cache::{CacheActionCmd, CacheBlobCmd, CacheCmd};
pub(crate) use cache::{CacheSize, DEFAULT_CACHE_SIZE_CAP_BYTES};
pub use edit::EditCmd;
pub use native::NativeCmd;
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
#[derive(Copy, Clone, Debug, usage::ValueEnum, Default, PartialEq, Eq)]
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

/// Map a subprocess exit code to a command-line interface [`ExitCode`].
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MemoryLimit(u64);

impl MemoryLimit {
    pub(crate) const fn bytes(self) -> u64 {
        self.0
    }
}

impl FromStr for MemoryLimit {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        parse_byte_size(raw).map(Self)
    }
}

#[derive(Debug)]
pub(crate) struct EnvironmentAssignment(String, String);

impl EnvironmentAssignment {
    pub(crate) fn into_inner(self) -> (String, String) {
        (self.0, self.1)
    }
}

impl FromStr for EnvironmentAssignment {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let (key, value) = raw
            .split_once('=')
            .ok_or_else(|| format!("expected KEY=VALUE, got `{raw}`"))?;
        Ok(Self(key.to_string(), value.to_string()))
    }
}

#[derive(usage::Cli)]
#[usage(
    bin = "once",
    version = CLI_VERSION,
    about = "Graph-aware, cacheable, remotely-executable repository automation",
    arg_required_else_help = true,
    unknown_flags = "error",
    args_override_self = false
)]
pub struct Cli {
    /// Project root. Defaults to the current directory; the cache
    /// lives under `<project>/.once/`. Mirrors `make -C`.
    #[usage(short = 'C', long = "directory", global = true, value_name = "DIR")]
    pub directory: Option<PathBuf>,

    /// Output format for Once's structured data (`cache
    /// stats`, `run`/`exec` trailers). Defaults to a human-readable
    /// rendering; pass `json` or `toon` to get machine-parseable
    /// output for scripting and for agent consumers.
    #[usage(long, global = true, value_enum, default = "human")]
    pub format: Format,

    /// Increase log verbosity. Repeat for more (-v: info, -vv: debug,
    /// -vvv: trace). Overridden by `RUST_LOG`.
    #[usage(short, long, count, global = true)]
    pub verbose: u8,

    /// Suppress human-mode success and progress trailers. Errors and the structured
    /// envelope of `--format json`/`toon` still print. Mirrors the
    /// `-q` flag of common build tools.
    #[usage(short = 'q', long, global = true)]
    pub quiet: bool,

    /// Print the command surface at the current command depth.
    #[usage(long, global = true)]
    pub list: bool,

    /// Maximum memory scheduling budget for local actions.
    /// Defaults to two thirds of the memory visible to the host or container.
    #[usage(long, global = true, value_name = "SIZE")]
    pub(crate) memory_limit: Option<MemoryLimit>,

    #[usage(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(usage::Subcommands)]
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
    Build {
        /// Local filesystem sandbox policy for command actions.
        #[usage(long, default = "off")]
        sandbox: SandboxMode,

        /// Override the workspace build configuration. Recognized keys are
        /// `os`, `arch`, and `token` (repeatable), supplied as `KEY=VALUE`.
        /// Targets configured with `select` resolve against the merged
        /// configuration, and their outputs are scoped so different values
        /// never collide.
        #[usage(long, value_name = "KEY=VALUE")]
        config: Vec<String>,

        /// Start the local Runs interface for this Once build.
        ///
        /// Once serves the client interface from this process. The page
        /// receives the build target, dependency graph, cache decision,
        /// duration, action digest, and output as the build progresses.
        #[usage(long)]
        ui: bool,

        /// Target id, such as `services/api/Api` or `./Api`.
        target: Option<String>,
    },

    /// Run static analysis for a declared target.
    ///
    /// Executes the target's `lint` capability, normalizes its report,
    /// and returns a failing status when a finding meets `--fail-on`.
    Lint {
        /// Local filesystem sandbox policy for command actions.
        #[usage(long, default = "off")]
        sandbox: SandboxMode,

        /// Override the workspace build configuration. See `once build --config`.
        #[usage(long, value_name = "KEY=VALUE")]
        config: Vec<String>,

        /// Lowest finding severity that makes this command fail.
        #[usage(long, default = "warning")]
        fail_on: LintSeverity,

        /// Target id, such as `quality/python` or `./python`.
        target: Option<String>,
    },

    /// Run a declared target.
    ///
    /// Resolves the target id against the workspace graph and executes
    /// its `run` capability through the action cache. Use `--remote`
    /// to ask a compute provider to execute the command.
    Run {
        /// Local filesystem sandbox policy for command actions.
        #[usage(long, default = "off")]
        sandbox: SandboxMode,

        /// Override the workspace build configuration. See `once build --config`.
        #[usage(long, value_name = "KEY=VALUE")]
        config: Vec<String>,

        /// Ask graph target kinds to open a visible runtime interface when supported.
        #[usage(long)]
        visible: bool,

        /// Serve a local JSON-RPC runtime control socket for this run.
        #[usage(long)]
        runtime_rpc: bool,

        /// Runtime RPC socket path. Defaults to
        /// `.once/runtime/<session>/control.sock`.
        #[usage(long)]
        runtime_rpc_socket: Option<PathBuf>,

        /// Run the target's action on a compute provider.
        #[usage(long)]
        remote: bool,

        /// Compute provider used with --remote. Defaults to the configured execution provider.
        #[usage(long, value_name = "PROVIDER")]
        compute: Option<String>,

        /// Target id, e.g. `examples/hello/hello` or `./hello`.
        target: Option<String>,

        /// Target-kind-specific arguments supplied to the generic run capability.
        #[usage(double_dash = "required", value_name = "ARG")]
        arguments: Vec<String>,
    },

    /// Test a declared target.
    ///
    /// Builds the target as needed, then executes its `test`
    /// capability through the action cache. Output paths and result
    /// groups are owned by the target kind that exposes the capability.
    /// With `--changed-path` or `--all`, stable target batches are pulled
    /// from a duration-informed dynamic queue. `--jobs` caps local workers
    /// without changing the plan or batch identities.
    Test {
        /// Local filesystem sandbox policy for command actions.
        #[usage(long, default = "off")]
        sandbox: SandboxMode,

        /// Override the workspace build configuration. See `once build --config`.
        #[usage(long, value_name = "KEY=VALUE")]
        config: Vec<String>,

        /// Start the local Runs interface for this Once test run.
        #[usage(long)]
        ui: bool,

        /// Maximum number of test batches to execute concurrently.
        /// Defaults to the host's available parallelism for an affected plan.
        #[usage(short = 'j', long, value_name = "COUNT")]
        jobs: Option<usize>,

        /// Run every discovered test target through the dynamic scheduler.
        #[usage(long, conflicts("target", "changed_paths", "test_unit"))]
        all: bool,

        /// Select tests affected by a workspace-relative changed path.
        /// Repeat for multiple paths. Cannot be combined with a target id.
        #[usage(long = "changed-path", value_name = "PATH", conflicts = "target")]
        changed_paths: Vec<String>,

        /// Run one current, filterable unit from `once query test-manifest`.
        /// The request is rejected before scheduling when the target does not
        /// support exact filtering or the unit is absent from the manifest.
        #[usage(
            long = "test-unit",
            value_name = "UNIT",
            requires = "target",
            conflicts("changed_paths", "all", "batch_test_units")
        )]
        test_unit: Option<String>,

        #[usage(
            long = "batch-test-unit",
            hide = true,
            requires = "target",
            conflicts = "test_unit"
        )]
        batch_test_units: Vec<String>,

        #[usage(long, hide = true, requires = "batch_test_units")]
        test_batch_id: Option<String>,

        /// Target id, such as `tests/unit` or `./unit`.
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
    Exec {
        /// Local filesystem sandbox policy for the command action.
        #[usage(long, default = "off")]
        sandbox: SandboxMode,

        /// Interpret argv as `<runtime> <script> [args...]` and apply
        /// `once` headers from the script file. Useful as the
        /// explicit form, for example `once exec --script bash
        /// scripts/build.sh`, and for directly executable scripts via
        /// a shebang such as `#!/usr/bin/env -S once exec -- bash`.
        #[usage(long)]
        script: bool,

        /// Pass an environment variable to the command. Repeatable.
        #[usage(short = 'e')]
        env: Vec<EnvironmentAssignment>,

        /// Working directory, relative to the project root. Must not
        /// be absolute or escape the project.
        #[usage(long)]
        cwd: Option<WorkspacePath>,

        /// Per-action timeout in milliseconds. The child is killed if
        /// it exceeds the deadline.
        #[usage(long, value_name = "MS")]
        timeout_ms: Option<u64>,

        /// Cache non-zero exits the same way zero exits are cached.
        /// Off by default; transient failures shouldn't poison the
        /// cache.
        #[usage(long)]
        cache_failures: bool,

        /// Run the action twice while bypassing the cache and report whether
        /// the two trials produced identical results, instead of executing
        /// normally. Exits non-zero when any divergence is found. Useful for
        /// catching nondeterministic tools and undeclared inputs that leak
        /// through the cache key.
        #[usage(long)]
        verify_reproducible: bool,

        /// Run the command on a compute provider.
        #[usage(long)]
        remote: bool,

        /// Whether the command may reach the network. Defaults to
        /// `unrestricted` (the host network is available, matching existing
        /// behavior). `deny` isolates the command from the network on Linux
        /// so an undeclared fetch fails loudly instead of leaking into the
        /// cache key.
        #[usage(long, default = "unrestricted")]
        network: NetworkPolicy,

        /// Compute provider used with --remote. Defaults to the configured execution provider.
        #[usage(long, value_name = "PROVIDER")]
        compute: Option<String>,

        /// Command and arguments. Use `--` to separate from once flags.
        #[usage(trailing_var_arg = true)]
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
    Cache {
        #[usage(subcommand)]
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
    Auth {
        #[usage(subcommand)]
        cmd: Option<AuthCmd>,
    },

    /// Inspect the project toolchain contract.
    ///
    /// Reports the toolchains a project pins (Rust, Swift, mise) and
    /// the resolved versions Once will use when running actions from
    /// script adapters or graph target kinds. Pair with `once query schema`
    /// when debugging "why did the cache miss?" questions where the
    /// toolchain identity is suspect.
    Toolchain {
        #[usage(subcommand)]
        cmd: Option<ToolchainCmd>,
    },

    /// Query the typed build graph
    ///
    /// Inspectable-first surface for humans and agents. `query
    /// targets` lists every declared target id with its target kind
    /// and capabilities; `query capabilities` shows what a specific
    /// target exposes (`build`, `lint`, `run`, `test`); `query schema`
    /// returns the typed attribute and provider shape for a target kind;
    /// `query example` returns the files in a chosen starter; `query script` validates
    /// an annotated script contract; `query validate-workspace` checks the
    /// complete loaded graph; and `query evidence` lists durable action evidence
    /// captured from prior executions. A quoted
    /// `MATCH ... RETURN ...` expression can explore the graph through
    /// a read-only Cypher-like pattern. All query surfaces respect
    /// `--format json` and `--format toon` so consumers can plan
    /// against the graph without scraping prose.
    ///
    /// ## Query Expressions
    ///
    /// `once query '<QUERY>'` accepts a read-only subset of Cypher backed
    /// by the Cypher tree-sitter grammar. It accepts one `MATCH` pattern,
    /// optional `WHERE` predicates joined with `AND` and `OR`, and explicit
    /// `RETURN` projections.
    ///
    /// ```sh
    /// once query 'MATCH (app:Target {id: "services/api/Api"})-[:DEPENDS_ON*]->(dep:Target) RETURN dep.id, dep.kind'
    /// once query 'MATCH (t:Target)-[:EXPOSES]->(c:Capability {name: "test"}) RETURN t.id'
    /// once query 'MATCH (t:Target) WHERE t.visibility CONTAINS "public" OR t.attrs.tier IN ["core", "shared"] RETURN t.id, t.attrs.tier'
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
    /// Predicates support `=`, `<>`, `CONTAINS`, `IN`, `STARTS WITH`, and
    /// `ENDS WITH`. `CONTAINS` checks a string substring or array membership.
    /// `IN` checks whether a scalar is present in an array literal. Property
    /// paths can inspect nested maps, for example `t.attrs.bundle_id`.
    ///
    /// String literals can be quoted with single or double quotes and
    /// support `\n`, `\r`, `\t`, `\\`, `\"`, and `\'` escapes. Other
    /// escape forms, including Unicode escapes, are rejected.
    #[usage(verbatim_doc_comment)]
    Query {
        /// Read-only Cypher-like graph query expression.
        #[usage(value_name = "QUERY")]
        expression: Option<String>,
        #[usage(subcommand)]
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
    Runtime {
        #[usage(subcommand)]
        cmd: Option<RuntimeCmd>,
    },

    /// Mutate workspace manifests.
    ///
    /// `edit apply` runs a batch of `create` / `update` / `delete`
    /// operations against a single `once.toml` atomically. The CLI
    /// reads its input JSON from `--file` or stdin and emits
    /// structured diagnostics for failed edits. `edit
    /// materialize-example` copies a target kind starter without
    /// printing its file contents and refuses conflicting paths.
    Edit {
        #[usage(subcommand)]
        cmd: Option<EditCmd>,
    },

    /// Discover, inspect, and initialize native workspace roots.
    Native {
        #[usage(subcommand)]
        cmd: Option<NativeCmd>,
    },

    /// Accept an Xcode build invocation and use the Once graph when its
    /// semantics are supported. Other invocations pass through to the system
    /// Xcode build tool unchanged. Configure this as a mise command wrapper
    /// to make ordinary `xcodebuild` commands use this compatibility surface.
    #[usage(name = "xcodebuild")]
    Compatibility {
        /// Arguments supplied by the Xcode build invocation.
        #[usage(trailing_var_arg = true, value_name = "ARG")]
        argv: Vec<String>,
    },

    /// Accept a Swift Package Manager build or test invocation and use the
    /// Once graph when its semantics are supported. Other invocations pass
    /// through to the system Swift executable unchanged. Configure this as a
    /// mise command wrapper to make ordinary `swift` commands use this
    /// compatibility surface.
    #[usage(name = "swift")]
    PackageCompatibility {
        /// Arguments supplied by the Swift Package Manager invocation.
        #[usage(trailing_var_arg = true, value_name = "ARG")]
        argv: Vec<String>,
    },

    /// Accept a Bazel build or test invocation and use the Once graph when
    /// its semantics are supported. Other invocations pass through to the
    /// system Bazel executable unchanged. Configure this as a mise command
    /// wrapper to make ordinary `bazel` commands use this compatibility
    /// surface.
    #[usage(name = "bazel")]
    BazelCompatibility {
        /// Arguments supplied by the Bazel invocation.
        #[usage(trailing_var_arg = true, value_name = "ARG")]
        argv: Vec<String>,
    },

    /// Expose Once's graph and memory queries to a coding agent.
    ///
    /// Speaks the Model Context Protocol over standard input and output so a
    /// coding harness can discover schemas and starters, validate and edit
    /// typed graphs, inspect or execute annotated scripts, run graph
    /// capabilities, and query project evidence without scraping prose.
    /// Mounts inspection tools by default; pass `--allow-run` to expose
    /// manifest editing, test, build, run, and runtime session tools.
    Mcp {
        /// Workspace root the agent tools resolve targets against.
        /// Defaults to the value of the global `-C/--directory` flag
        /// (or the current directory).
        #[usage(long, value_name = "DIR")]
        workspace: Option<PathBuf>,

        /// Advertise and allow state-changing editing and execution tools.
        #[usage(long)]
        allow_run: bool,
    },

    /// Internal: emit the markdown CLI reference into `out`. Hidden
    /// from `--help` because it is a documentation build hook, not a
    /// user-facing verb. Drives `docs/reference/cli/*.md` so the
    /// website's flag and synopsis sections never drift from the
    /// real command declarations.
    #[usage(hide = true)]
    Reference {
        /// Directory to emit per-subcommand markdown files into.
        #[usage(long, value_name = "DIR")]
        out: PathBuf,
    },

    #[usage(name = "__change-tracker", hide = true)]
    ChangeTracker,
}

/// A compatibility invocation that preserves an established build-tool
/// command while routing equivalent requests through Once.
pub(crate) enum CompatibilityInvocation {
    Xcodebuild(Vec<String>),
    Swift(Vec<String>),
    Bazel(Vec<String>),
}

impl Cli {
    pub fn surface_path(&self) -> Vec<&'static str> {
        self.command
            .as_ref()
            .map_or_else(Vec::new, Cmd::surface_path)
    }

    pub(crate) fn incomplete_command_help_path(&self) -> Option<&'static [&'static str]> {
        if self.list {
            return None;
        }

        match self.command.as_ref()? {
            Cmd::Build { target: None, .. } => Some(&["build"]),
            Cmd::Lint { target: None, .. } => Some(&["lint"]),
            Cmd::Run { target: None, .. } => Some(&["run"]),
            Cmd::Test {
                target: None,
                changed_paths,
                all: false,
                ..
            } if changed_paths.is_empty() => Some(&["test"]),
            Cmd::Exec { argv, .. } if argv.is_empty() => Some(&["exec"]),
            Cmd::Cache { cmd: None } => Some(&["cache"]),
            Cmd::Cache {
                cmd: Some(CacheCmd::Blob { cmd: None }),
            } => Some(&["cache", "blob"]),
            Cmd::Cache {
                cmd: Some(CacheCmd::Action { cmd: None }),
            } => Some(&["cache", "action"]),
            Cmd::Auth { cmd: None } => Some(&["auth"]),
            Cmd::Toolchain { cmd: None } => Some(&["toolchain"]),
            Cmd::Query {
                expression: None,
                cmd: None,
            } => Some(&["query"]),
            Cmd::Runtime { cmd: None } => Some(&["runtime"]),
            Cmd::Runtime {
                cmd: Some(RuntimeCmd::Start { target: None }),
            } => Some(&["runtime", "start"]),
            Cmd::Edit { cmd: None } => Some(&["edit"]),
            Cmd::Native { cmd: None } => Some(&["native"]),
            _ => None,
        }
    }
}

impl Cmd {
    pub(crate) fn compatibility(&self) -> Option<CompatibilityInvocation> {
        match self {
            Self::Compatibility { argv } => Some(CompatibilityInvocation::Xcodebuild(argv.clone())),
            Self::PackageCompatibility { argv } => {
                Some(CompatibilityInvocation::Swift(argv.clone()))
            }
            Self::BazelCompatibility { argv } => Some(CompatibilityInvocation::Bazel(argv.clone())),
            _ => None,
        }
    }

    pub fn surface_path(&self) -> Vec<&'static str> {
        match self {
            Self::Build { .. } => vec!["build"],
            Self::Lint { .. } => vec!["lint"],
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
            Self::Native { cmd } => {
                let mut path = vec!["native"];
                if let Some(cmd) = cmd {
                    path.extend(cmd.surface_path());
                }
                path
            }
            Self::Compatibility { .. } => vec!["xcodebuild"],
            Self::PackageCompatibility { .. } => vec!["swift"],
            Self::BazelCompatibility { .. } => vec!["bazel"],
            Self::Runtime { cmd } => {
                let mut path = vec!["runtime"];
                if let Some(cmd) = cmd {
                    path.extend(cmd.surface_path());
                }
                path
            }
            Self::Mcp { .. } => vec!["mcp"],
            Self::Reference { .. } => vec!["reference"],
            Self::ChangeTracker => vec!["__change-tracker"],
        }
    }
}

fn parse_byte_size(raw: &str) -> std::result::Result<u64, String> {
    let value = raw.trim();
    let digit_count = value.bytes().take_while(u8::is_ascii_digit).count();
    if digit_count == 0 {
        return Err(format!("expected a positive byte size, got `{raw}`"));
    }
    let (number, suffix) = value.split_at(digit_count);
    let number = number
        .parse::<u64>()
        .map_err(|_| format!("invalid byte size `{raw}`"))?;
    if number == 0 {
        return Err("memory limit must be greater than zero".to_string());
    }
    let multiplier = match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "kb" => 1_000,
        "mb" => 1_000_000,
        "gb" => 1_000_000_000,
        "tb" => 1_000_000_000_000,
        "kib" => 1 << 10,
        "mib" => 1 << 20,
        "gib" => 1 << 30,
        "tib" => 1_u64 << 40,
        _ => return Err(format!("unsupported byte-size suffix in `{raw}`")),
    };
    number
        .checked_mul(multiplier)
        .ok_or_else(|| format!("byte size `{raw}` is too large"))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    fn parse(argv: &[&str]) -> Cli {
        let argv = argv.iter().map(OsStr::new).collect::<Vec<_>>();
        Cli::try_parse_from(&argv).unwrap()
    }

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

    #[test]
    fn byte_sizes_accept_decimal_binary_and_raw_values() {
        assert_eq!(MemoryLimit::from_str("1024").unwrap().bytes(), 1024);
        assert_eq!(MemoryLimit::from_str("2MB").unwrap().bytes(), 2_000_000);
        assert_eq!(
            MemoryLimit::from_str("2 MiB").unwrap().bytes(),
            2 * 1024 * 1024
        );
    }

    #[test]
    fn byte_sizes_reject_zero_unknown_suffixes_and_overflow() {
        assert!(MemoryLimit::from_str("0").is_err());
        assert!(MemoryLimit::from_str("1XB").is_err());
        assert!(MemoryLimit::from_str("18446744073709551615TiB").is_err());
    }

    #[test]
    fn run_accepts_target_arguments_after_separator() {
        let cli = parse(&[
            "once",
            "run",
            "server/application_dev",
            "--",
            "phx.server",
            "--port",
            "4001",
        ]);

        let Some(Cmd::Run {
            target, arguments, ..
        }) = cli.command
        else {
            panic!("expected run command");
        };
        assert_eq!(target.as_deref(), Some("server/application_dev"));
        assert_eq!(arguments, ["phx.server", "--port", "4001"]);
    }

    #[test]
    fn compatibility_accepts_xcodebuild_arguments_after_separator() {
        let cli = parse(&["once", "xcodebuild", "--", "-showBuildSettings"]);

        let Some(Cmd::Compatibility { argv }) = cli.command else {
            panic!("expected compatibility command");
        };
        assert_eq!(argv, ["-showBuildSettings"]);
    }

    #[test]
    fn compatibility_accepts_swift_arguments_after_separator() {
        let cli = parse(&["once", "swift", "--", "build"]);

        let Some(Cmd::PackageCompatibility { argv }) = cli.command else {
            panic!("expected Swift compatibility command");
        };
        assert_eq!(argv, ["build"]);
    }

    #[test]
    fn rejects_unknown_options() {
        let argv = ["once", "build", "target", "--unrecognized"].map(OsStr::new);

        assert!(Cli::try_parse_from(&argv).is_err());
    }

    #[test]
    fn identifies_commands_that_need_their_help_rendered() {
        assert_eq!(
            parse(&["once", "build"]).incomplete_command_help_path(),
            Some(&["build"][..])
        );
        assert_eq!(
            parse(&["once", "cache", "blob"]).incomplete_command_help_path(),
            Some(&["cache", "blob"][..])
        );
        assert_eq!(
            parse(&["once", "test", "--all"]).incomplete_command_help_path(),
            None
        );
        assert_eq!(
            parse(&["once", "run", "--list"]).incomplete_command_help_path(),
            None
        );
    }
}
