use std::path::PathBuf;

use clap::Subcommand;

#[derive(Subcommand)]
pub enum EditCmd {
    /// Apply a batch of operations to one `once.toml` atomically.
    ///
    /// Reads a JSON document matching the `once_apply_edit` MCP tool
    /// input shape (`{ "package": "...", "operations": [...] }`) from
    /// `--file` or, if omitted, from stdin. On success, the manifest
    /// is rewritten and the resolved path is printed. On failure,
    /// structured diagnostics are emitted and the manifest is left
    /// untouched.
    Apply {
        /// Path to a JSON file. When omitted, the JSON document is read from stdin.
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
    },
}

impl EditCmd {
    pub fn surface_path(&self) -> Vec<&'static str> {
        match self {
            Self::Apply { .. } => vec!["apply"],
        }
    }
}
