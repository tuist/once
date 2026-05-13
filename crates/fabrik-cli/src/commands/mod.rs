//! Subcommand implementations. Each verb lives in its own module; the
//! dispatcher in [`crate::main`] routes parsed [`crate::cli::Cmd`] into
//! these.

pub mod build;
pub mod cache;
pub mod deps;
#[cfg(unix)]
pub mod elixir_compile;
#[cfg(unix)]
pub mod elixir_daemon;
pub mod exec;
pub mod run;
pub mod runtime;
pub mod targets;
pub mod test;
pub mod toolchain;
pub mod util;
mod vendor_elixir;
mod vendor_go;
mod vendor_graph;
mod vendor_rust;
mod vendor_swift;
