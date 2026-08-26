use usage::Subcommands;

#[derive(Subcommands)]
pub enum NativeCmd {
    /// List recognized native roots and their current matches.
    List,

    /// Show the typed graph derived from one native root.
    Show {
        /// Native integration name from `once native list`.
        name: String,
        /// Workspace-relative path of the matched root.
        #[usage(long, value_name = "PATH")]
        path: Option<String>,
    },

    /// Store one detected native seed in `once.toml`.
    Init {
        /// Native integration name from `once native list`.
        name: String,
        /// Workspace-relative path of the matched root.
        #[usage(long, value_name = "PATH")]
        path: Option<String>,
    },
}

impl NativeCmd {
    pub fn surface_path(&self) -> Vec<&'static str> {
        match self {
            Self::List => vec!["list"],
            Self::Show { .. } => vec!["show"],
            Self::Init { .. } => vec!["init"],
        }
    }
}
