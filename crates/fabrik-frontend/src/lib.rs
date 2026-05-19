//! Build-file frontend.
//!
//! Loads declarative `fabrik.toml` package files into a shared
//! [`Target`] IR. TOML keeps the build graph typed, literal, and
//! straightforward for humans and agents to patch.
//!
//! Module layout:
//! - [`target`]: the [`Target`] record.
//! - [`error`]: the [`Error`] enum and [`Result`] alias.
//! - [`manifest`]: TOML schema lowering into [`Target`]s.
//! - [`target_ref`]: target id normalization for CLI args and build-file deps.
//! - [`workspace`]: disk-side loaders ([`load_file`], [`load_workspace`]).

mod dependency;
mod error;
mod manifest;
mod target;
mod target_ref;
mod workspace;

/// The declarative per-package build file.
pub const TOML_BUILD_FILE_NAME: &str = "fabrik.toml";

pub const BUILD_FILE_NAME: &str = TOML_BUILD_FILE_NAME;

pub use dependency::{
    external_dep_id, external_dep_package, external_target_id, generated_external_format_header,
    parse_generated_external_format, DependencyEcosystem, DependencyEntry,
    EXTERNAL_PACKAGE_CACHE_ROOT, EXTERNAL_TARGET_PREFIX, GENERATED_EXTERNAL_FORMAT_VERSION,
};
pub use error::{Error, Result};
pub use manifest::{load_dependency_entries_toml_str, load_toml_str};
pub use target::{ExternalDependency, Target};
pub use target_ref::{
    absolutize, normalize_build_dep, normalize_cli_target, normalize_cli_target_from, target_id,
    validate_target_name, TargetIdError,
};
pub use workspace::{load_dependency_entries, load_file, load_workspace};
