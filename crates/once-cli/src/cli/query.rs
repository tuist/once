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

    /// List every target kind with its one-line docs and example slugs.
    #[command(alias = "rules")]
    TargetKinds,

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

    /// Validate a proposed `[[target]]` table against its target kind schema.
    ///
    /// Reads `{ "target": { ... } }` from `--file` or, if omitted,
    /// from stdin.
    ValidateTarget {
        /// Path to a JSON file. When omitted, the JSON document is read from stdin.
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
    },
}

impl QueryCmd {
    pub fn surface_path(&self) -> Vec<&'static str> {
        match self {
            Self::Targets { .. } => vec!["targets"],
            Self::Capabilities { .. } => vec!["capabilities"],
            Self::Schema { .. } => vec!["schema"],
            Self::Example { .. } => vec!["example"],
            Self::TargetKinds => vec!["target-kinds"],
            Self::Target { .. } => vec!["target"],
            Self::Tests => vec!["tests"],
            Self::AffectedTests { .. } => vec!["affected-tests"],
            Self::TestResults { .. } => vec!["test-results"],
            Self::ValidateTarget { .. } => vec!["validate-target"],
        }
    }
}
