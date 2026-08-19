use std::path::PathBuf;

use clap::Subcommand;

#[derive(Subcommand)]
pub enum EditCmd {
    /// Apply a batch of operations to one `once.toml` atomically.
    ///
    /// Reads `{ "package": "...", "operations": [...] }` from `--file`
    /// or, if omitted, from stdin. On success, the manifest is
    /// rewritten and the resolved path is printed. On failure,
    /// structured diagnostics are emitted and the manifest is left
    /// untouched.
    Apply {
        /// Path to a JSON file. When omitted, the JSON document is read from stdin.
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
    },

    /// Materialize a target kind starter inside the workspace.
    ///
    /// Copies the complete example bundle without printing file contents.
    /// Existing files with identical contents are kept. Any conflicting
    /// file rejects the complete operation before Once writes anything.
    MaterializeExample {
        /// Target kind that owns the example.
        kind: String,
        /// Example slug from `once query schema`.
        slug: String,
        /// Workspace-relative directory that receives the example.
        #[arg(long, default_value = "", value_name = "DIR")]
        destination: String,
    },
}

impl EditCmd {
    pub fn surface_path(&self) -> Vec<&'static str> {
        match self {
            Self::Apply { .. } => vec!["apply"],
            Self::MaterializeExample { .. } => vec!["materialize-example"],
        }
    }
}
