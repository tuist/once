//! Subcommand implementations. Each verb lives in its own module; the
//! dispatcher in [`crate::main`] routes parsed [`crate::cli::Cmd`] into
//! these.

pub mod build;
pub mod cache;
pub mod exec;
pub mod run;
pub mod targets;
pub mod test;
pub mod toolchain;
pub mod util;
pub mod vendor;
