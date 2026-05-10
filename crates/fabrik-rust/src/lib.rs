//! Rust language support: compile [`fabrik_frontend::Target`]s into
//! [`fabrik_core::Plan`]s of direct rustc invocations.
//!
//! The compilation model mirrors `rules_rust`: one rustc invocation
//! per crate, dependencies passed through `--extern`, libraries emit
//! both `.rlib` and `.rmeta` for downstream consumption. The plan
//! scheduler in `fabrik-core` runs the resulting actions in dependency
//! order with the configured concurrency cap, and the action cache
//! keys each invocation on the digest of its sources plus the digests
//! of every transitive dependency action.
//!
//! Module layout:
//! - [`artifact`]: deterministic output paths and crate-name normalization.
//! - [`compile`]: target-kind handlers that emit a [`PlanNode`].
//! - [`plan`]: the workspace-wide topological plan builder.

mod artifact;
mod build_script;
mod compile;
mod plan;

pub use artifact::{DepArtifact, RustKind};
pub use build_script::{compile_build_script, BuildScriptOutputs, BUILD_SCRIPT_OUTPUTS_FILENAME};
pub use compile::{compile_target, CompileError};
pub use plan::{build_plan, PlanBuildError};
