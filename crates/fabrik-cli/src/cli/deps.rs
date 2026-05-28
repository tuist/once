use clap::Subcommand;

#[derive(Subcommand)]
pub enum DepsCmd {
    /// Synchronize generated dependency artifacts from `fabrik.toml`.
    Sync {
        /// Sync one dependency entry by name. Defaults to all entries.
        name: Option<String>,
    },
}

impl DepsCmd {
    pub(super) fn surface_path(&self) -> Vec<&'static str> {
        match self {
            Self::Sync { .. } => vec!["sync"],
        }
    }
}
