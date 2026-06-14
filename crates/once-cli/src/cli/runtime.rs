use std::path::PathBuf;

use clap::Subcommand;

#[derive(Subcommand)]
pub enum RuntimeCmd {
    /// Start a target in a persisted runtime session.
    Start {
        /// Target id, e.g. `tools/demo/LaunchApp` or `./LaunchApp`.
        #[arg(required_unless_present = "list")]
        target: Option<String>,
    },

    /// Show the latest status for a runtime session.
    Status {
        /// Session id returned by `once runtime start`.
        session: String,
    },

    /// Read persisted stdout or stderr records for a runtime session.
    Logs {
        /// Session id returned by `once runtime start`.
        session: String,

        /// Log source to read: `stdout` or `stderr`.
        #[arg(long, value_parser = ["stdout", "stderr"])]
        source: Option<String>,

        /// Cursor returned by a previous logs call.
        #[arg(long)]
        cursor: Option<String>,

        /// Maximum number of log records to return.
        #[arg(long)]
        limit: Option<usize>,
    },

    /// Request that a runtime session stop.
    Stop {
        /// Session id returned by `once runtime start`.
        session: String,
    },

    /// Serve the local runtime JSON-RPC endpoint for a session directory.
    Rpc {
        /// Runtime session directory containing session.json and logs.
        session_dir: PathBuf,

        /// Socket path. Defaults to `<session-dir>/control.sock`.
        #[arg(long)]
        socket: Option<PathBuf>,
    },

    /// Internal: supervise a target process for a runtime session.
    #[command(hide = true)]
    Supervise {
        /// Runtime session directory containing session.json and logs.
        #[arg(long)]
        session_dir: PathBuf,

        /// Canonical target id to run under supervision.
        #[arg(long)]
        target: String,
    },
}

impl RuntimeCmd {
    pub(super) fn surface_path(&self) -> Vec<&'static str> {
        match self {
            Self::Start { .. } => vec!["start"],
            Self::Status { .. } => vec!["status"],
            Self::Logs { .. } => vec!["logs"],
            Self::Stop { .. } => vec!["stop"],
            Self::Rpc { .. } => vec!["rpc"],
            Self::Supervise { .. } => vec!["supervise"],
        }
    }
}
