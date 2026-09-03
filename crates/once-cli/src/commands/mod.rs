//! Subcommand implementations. Each verb lives in its own module; the
//! dispatcher in [`crate::main`] routes parsed [`crate::cli::Cmd`] into
//! these.

pub mod auth;
pub mod bazel;
pub mod cache;
pub mod change_tracker;
pub mod compatibility;
pub mod edit;
pub mod evidence;
pub mod exec;
pub mod fingerprint;
pub mod graph;
pub mod mcp;
pub mod query;
pub mod run;
pub mod runtime;
pub mod surface;
pub mod swift;
pub mod test_schedule;
pub mod toolchain;
pub mod ui;
pub mod util;
pub mod xcodebuild;
