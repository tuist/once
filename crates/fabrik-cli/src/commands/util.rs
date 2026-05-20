//! Cross-verb helpers: workspace target lookup and cache-state
//! rendering. These are tiny on their own; the value is forcing every
//! verb through one shape so adding a new verb (or a new build-system
//! plugin's verb) doesn't reinvent the same boilerplate slightly
//! differently.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use fabrik_cas::Cas;
use fabrik_core::{CacheState, ResourceLimits, RunOpts, Runner};
use fabrik_frontend::Target;

/// Load every target declared in the workspace and pick the one whose
/// id matches `target_id`. Returns an error if the workspace fails to
/// load or no target has the requested id. Verbs that operate on a
/// single target funnel through this so the error wording is uniform.
pub fn find_target(workspace: &Path, target_id: &str) -> Result<(Vec<Target>, usize)> {
    let targets = fabrik_frontend::load_workspace(workspace).context("loading workspace")?;
    let idx = targets
        .iter()
        .position(|t| t.id() == target_id)
        .ok_or_else(|| {
            anyhow!(
                "no target matches `{target_id}`. Run `fabrik targets` to list declared targets"
            )
        })?;
    Ok((targets, idx))
}

/// The short string Fabrik prints for a [`CacheState`]. Repeated
/// in every verb that emits structured output, so it lives here to
/// keep the spelling uniform (`hit` / `miss`, no trailing space).
#[must_use]
pub fn cache_tag(cache: CacheState) -> &'static str {
    match cache {
        CacheState::Hit => "hit",
        CacheState::Miss => "miss",
    }
}

/// Construct the runner every CLI verb uses, with the named-slot
/// pools each language plugin publishes pre-registered. Keeping the
/// wiring in one place ensures the daemon-bounded `ELIXIR_COMPILE_SLOT`
/// and any future plugin slots stay aligned across `build`, `run`, and
/// `test` instead of one verb forgetting to populate them.
pub fn runner(cas: &Cas, workspace: &Path) -> Runner {
    Runner::new(cas.clone(), workspace.to_path_buf(), RunOpts::default())
        .with_resource_limits(plugin_resource_limits())
}

fn plugin_resource_limits() -> ResourceLimits {
    ResourceLimits::default().with_slot_limit(
        fabrik_elixir::ELIXIR_COMPILE_SLOT,
        fabrik_elixir::default_compile_slot_limit(),
    )
}
