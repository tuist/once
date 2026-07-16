use std::path::PathBuf;

use clap::Subcommand;

#[derive(Subcommand)]
pub enum QueryCmd {
    /// List declared graph targets.
    Targets {
        /// Only include targets with this target kind.
        #[arg(long)]
        kind: Option<String>,
    },

    /// List capabilities and output groups for a target.
    Capabilities {
        /// Target id, such as `apps/service/Service`.
        target: String,
    },

    /// Inspect a target kind schema.
    Schema {
        /// Target kind to inspect. Discover names with `once query target-kinds`.
        kind: String,
    },

    /// Materialize a target kind starter example.
    Example {
        /// Target kind that owns the example.
        kind: String,
        /// Example slug from `once query schema`.
        slug: String,
    },

    /// List target kinds with their one-line docs and example slugs.
    #[command(alias = "rules")]
    TargetKinds {
        /// Match an ecosystem, target-kind family, or intent against the catalog.
        ///
        /// A family term takes priority over generic intent words. When no family
        /// or kind segment matches, Once searches docs, examples, and source references.
        #[arg(long, value_name = "TEXT")]
        query: Option<String>,
    },

    /// Return the project-module authoring contract and starter.
    ModuleContract,

    /// Fetch public external rule, plugin, or build-system source text.
    ExternalSource {
        /// Public HTTPS address for source code, metadata, or documentation.
        url: String,
        /// Maximum response bytes to return.
        #[arg(long, default_value_t = 256 * 1024, value_name = "COUNT")]
        max_bytes: usize,
    },

    /// Resolve a single target's full record (kind, srcs, deps, attrs, capabilities).
    Target {
        /// Target id, such as `packages/core/Core`.
        target: String,
    },

    /// List targets that expose the generic test capability.
    Tests,

    /// List test targets likely affected by changed workspace paths.
    AffectedTests {
        /// Changed workspace-relative path. Repeat for multiple paths.
        #[arg(long = "changed-path", value_name = "PATH")]
        changed_paths: Vec<String>,
    },

    /// Read normalized `once.test_results.v1` results for a target.
    TestResults {
        /// Target id, such as `tests/unit`.
        target: String,
    },

    /// List durable evidence records, optionally filtered by subject.
    ///
    /// Evidence records are provenance for action outcomes. They record
    /// what happened after `once exec`, `once run`, `once build`, or
    /// `once test`: the subject, status, action digest, input digest
    /// when available, cache state, exit code, and captured output
    /// digests when available. Evidence is queryable history; it does
    /// not change action-cache reuse rules.
    Evidence {
        /// Subject id, e.g. `cli` or `cli:test`.
        subject: Option<String>,
        /// Return only the newest matching records.
        #[arg(long, value_name = "COUNT")]
        limit: Option<usize>,
    },

    /// Validate a proposed `[[target]]` table against its target kind schema.
    ///
    /// Reads `{ "target": { ... } }` from `--file` or, if omitted,
    /// from stdin.
    ValidateTarget {
        /// Path to a JSON file. When omitted, the JSON document is read from stdin.
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
    },

    /// Validate and inspect an annotated script contract.
    Script {
        /// Workspace-relative script path.
        path: String,
    },

    /// Validate target schemas, dependency edges, providers, sources, and cycles across the workspace.
    ValidateWorkspace,

    /// Validate one project-local Starlark module without registering it.
    ValidateModule {
        /// Workspace-relative module path.
        path: String,
    },
}

impl QueryCmd {
    pub fn surface_path(&self) -> Vec<&'static str> {
        match self {
            Self::Targets { .. } => vec!["targets"],
            Self::Capabilities { .. } => vec!["capabilities"],
            Self::Schema { .. } => vec!["schema"],
            Self::Example { .. } => vec!["example"],
            Self::TargetKinds { .. } => vec!["target-kinds"],
            Self::ModuleContract => vec!["module-contract"],
            Self::ExternalSource { .. } => vec!["external-source"],
            Self::Target { .. } => vec!["target"],
            Self::Tests => vec!["tests"],
            Self::AffectedTests { .. } => vec!["affected-tests"],
            Self::TestResults { .. } => vec!["test-results"],
            Self::Evidence { .. } => vec!["evidence"],
            Self::ValidateTarget { .. } => vec!["validate-target"],
            Self::Script { .. } => vec!["script"],
            Self::ValidateWorkspace => vec!["validate-workspace"],
            Self::ValidateModule { .. } => vec!["validate-module"],
        }
    }
}
