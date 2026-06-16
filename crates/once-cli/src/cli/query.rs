use std::path::PathBuf;

use clap::Subcommand;

#[derive(Subcommand)]
pub enum QueryCmd {
    /// List declared graph targets.
    Targets {
        /// Only include targets with this rule kind.
        #[arg(long)]
        kind: Option<String>,
    },

    /// List capabilities and output groups for a target.
    Capabilities {
        /// Target id, e.g. `apps/ios/App`.
        target: String,
    },

    /// Inspect a rule schema.
    Schema {
        /// Rule kind, e.g. `apple_application`.
        kind: String,
    },

    /// List every rule kind with its one-line docs and example slugs.
    Rules,

    /// Resolve a single target's full record (kind, srcs, deps, attrs, capabilities).
    Target {
        /// Target id, e.g. `apps/Hello/Hello`.
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
        /// Target id, e.g. `spec/cli_e2e`.
        target: String,
    },

    /// Validate a proposed `[[target]]` table against its rule schema.
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
            Self::Rules => vec!["rules"],
            Self::Target { .. } => vec!["target"],
            Self::Tests => vec!["tests"],
            Self::AffectedTests { .. } => vec!["affected-tests"],
            Self::TestResults { .. } => vec!["test-results"],
            Self::ValidateTarget { .. } => vec!["validate-target"],
        }
    }
}
