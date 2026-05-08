//! Build-file frontend.
//!
//! Loads declarative `fabrik.toml` package files and the existing
//! `fabrik.star` Starlark files into a shared [`Target`] IR. TOML is
//! the agent-friendly path: typed sections, literal values, and stable
//! diffs. Starlark remains supported for compatibility with existing
//! packages and tests.
//!
//! Module layout:
//! - [`target`]: the [`Target`] record.
//! - [`error`]: the [`Error`] enum and [`Result`] alias.
//! - [`eval`]: the per-thread evaluation state and the `eval_with` driver.
//! - [`globals`]: the `#[starlark_module]` definitions exposed to user code.
//! - [`manifest`]: TOML schema lowering into [`Target`]s.
//! - [`workspace`]: disk-side loaders ([`load_file`], [`load_workspace`]).

mod error;
mod eval;
mod globals;
mod manifest;
mod prelude;
mod target;
mod workspace;

/// The conventional filename for a per-package build file.
pub const STAR_BUILD_FILE_NAME: &str = "fabrik.star";

/// The preferred declarative per-package build file.
pub const TOML_BUILD_FILE_NAME: &str = "fabrik.toml";

/// Backwards-compatible alias for callers that still refer to the
/// historical Starlark filename.
pub const BUILD_FILE_NAME: &str = STAR_BUILD_FILE_NAME;

pub use error::{Error, Result};
pub use eval::load_str;
pub use manifest::load_toml_str;
pub use target::Target;
pub use workspace::{load_file, load_workspace};
