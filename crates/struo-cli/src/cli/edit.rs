use std::path::PathBuf;

use usage::Subcommands;

#[derive(Subcommands)]
pub enum EditCmd {
    /// Atomically edit root trigger declarations without changing activation.
    Triggers {
        /// JSON document containing trigger operations. Reads stdin when omitted.
        #[usage(long, value_name = "PATH")]
        file: Option<PathBuf>,
    },

    /// Activate root trigger declarations from an exact committed revision.
    ActivateTriggers {
        /// Full source revision containing the declarations to activate.
        #[usage(long, value_name = "FULL_SHA")]
        revision: String,
    },

    /// Apply a batch of operations to one `struo.toml` atomically.
    ///
    /// Reads `{ "package": "...", "operations": [...] }` from `--file`
    /// or, if omitted, from stdin. On success, the manifest is
    /// rewritten and the resolved path is printed. On failure,
    /// structured diagnostics are emitted and the manifest is left
    /// untouched.
    Apply {
        /// Path to a JSON file. When omitted, the JSON document is read from stdin.
        #[usage(long, value_name = "PATH")]
        file: Option<PathBuf>,
    },

    /// Materialize a target kind starter inside the workspace.
    ///
    /// Copies the complete example bundle without printing file contents.
    /// Existing files with identical contents are kept. Any conflicting
    /// file rejects the complete operation before Struo writes anything.
    MaterializeExample {
        /// Target kind that owns the example.
        kind: String,
        /// Example slug from `struo query schema`.
        slug: String,
        /// Workspace-relative directory that receives the example.
        #[usage(long, default = "", value_name = "DIR")]
        destination: String,
    },
}

impl EditCmd {
    pub fn surface_path(&self) -> Vec<&'static str> {
        match self {
            Self::Triggers { .. } => vec!["triggers"],
            Self::ActivateTriggers { .. } => vec!["activate-triggers"],
            Self::Apply { .. } => vec!["apply"],
            Self::MaterializeExample { .. } => vec!["materialize-example"],
        }
    }
}
