//! `fabrik.star` build-file frontend.
//!
//! Embeds `starlark-rust` to load typed Starlark build files. Today the
//! evaluator recognises a fixed set of Rust target types (`rust_binary`,
//! `rust_library`, `rust_test`, `cargo_binary`) plus a `glob` primitive,
//! and records each call as a [`Target`]. The plugin contract that lets
//! external Starlark modules contribute target types lands later; the
//! current shape is the built-in slice of that same contract.
//!
//! Module layout:
//! - [`target`]: the [`Target`] record.
//! - [`error`]: the [`Error`] enum and [`Result`] alias.
//! - [`eval`]: the per-thread evaluation state and the `eval_with` driver.
//! - [`globals`]: the `#[starlark_module]` definitions exposed to user code.
//! - [`workspace`]: disk-side loaders ([`load_file`], [`load_workspace`]).

mod error;
mod eval;
mod globals;
mod prelude;
mod target;
mod workspace;

/// The conventional filename for a per-package build file.
pub const BUILD_FILE_NAME: &str = "fabrik.star";

pub use error::{Error, Result};
pub use eval::load_str;
pub use target::Target;
pub use workspace::{load_file, load_workspace};
