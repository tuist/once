//! Elixir language support: compile [`fabrik_frontend::Target`]s into
//! [`fabrik_core::Plan`]s of `elixirc` invocations.
//!
//! Each target is one cacheable action that compiles every source in
//! `srcs` into a per-target `.ebin` directory of `.beam` files. Deps
//! flow into downstream invocations through `-pa` so they appear on
//! the BEAM code path at compile time.
//!
//! The directory of `.beam` files is declared as a single output so
//! the cache restores the whole module set atomically. A future pass
//! can split this into per-module actions once the compile tracer
//! gives the planner precise per-module dep edges; the action shape
//! here is intentionally stable across that change.
//!
//! Module layout:
//! - [`artifact`]: deterministic output paths and dep-artifact records.
//! - [`compile`]: target-kind handlers that emit a [`PlanNode`].
//! - [`plan`]: the workspace-wide topological plan builder.

mod artifact;
mod compile;
#[cfg(unix)]
pub mod daemon;
mod plan;
pub mod protocol;

pub use artifact::{ebin_dir, escript_path, BeamArtifact, ElixirKind};
pub use compile::{compile_target, CompileError};
pub use plan::{build_plan, supports_kind, PlanBuildError};

/// Name of the shared `ResourcePool` slot that elixir compile actions
/// consume. Published by the CLI when constructing the runner so the
/// scheduler caps how many elixir actions can run concurrently in
/// step with the daemon's own bounded queue. Keeping the key here
/// rather than in fabrik-core avoids forcing the core to know about
/// individual plugins; the CLI's plugin-wiring helper imports it.
pub const ELIXIR_COMPILE_SLOT: &str = "elixir_compile";

/// Default size of the elixir compile slot pool. Aligns with the
/// daemon's default bounded queue so the scheduler never admits more
/// elixir actions than the daemon would accept in one batch. Plugins
/// that want headroom can override via the CLI; the daemon's
/// `FABRIK_ELIXIR_DAEMON_MAX_QUEUE` env var should be set in lock
/// step to keep the two limits aligned.
#[must_use]
pub fn default_compile_slot_limit() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(4)
}
