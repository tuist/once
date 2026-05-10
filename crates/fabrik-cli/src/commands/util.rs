//! Cross-verb helpers: workspace target lookup and cache-state
//! rendering. These are tiny on their own; the value is forcing every
//! verb through one shape so adding a new verb (or a new build-system
//! plugin's verb) doesn't reinvent the same boilerplate slightly
//! differently.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use fabrik_core::CacheState;
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
        .ok_or_else(|| anyhow!("no target matches `{target_id}`"))?;
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
