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

    /// Inspect a built-in rule schema.
    Schema {
        /// Rule kind, e.g. `apple_application`.
        kind: String,
    },
}

impl QueryCmd {
    pub fn surface_path(&self) -> Vec<&'static str> {
        match self {
            Self::Targets { .. } => vec!["targets"],
            Self::Capabilities { .. } => vec!["capabilities"],
            Self::Schema { .. } => vec!["schema"],
        }
    }
}
