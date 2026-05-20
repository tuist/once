//! Language planner registry.
//!
//! Each language crate exports a `supports_kind` predicate and a
//! `build_plan` constructor. The CLI used to dispatch on `target.kind`
//! with a hand-coded `if/else if` ladder, so adding a language meant
//! editing every dispatcher in step. The registry below collects the
//! per-language entries once; verbs that need a planner consult it
//! through [`plan_for_target`] and the dispatch ladder disappears.

use std::path::Path;

use anyhow::{anyhow, Result};
use fabrik_core::BuiltPlan;
use fabrik_frontend::Target;

/// One language's planner entry. `name` is for diagnostics only; the
/// dispatcher matches via [`supports_kind`].
pub struct LanguagePlanner {
    /// Diagnostic name. Surfaced in tests and (in the future) tracing.
    #[allow(dead_code)]
    pub name: &'static str,
    pub supports_kind: fn(&str) -> bool,
    pub build_plan: fn(&[Target], &str, &Path) -> Result<BuiltPlan>,
}

/// Static set of language planners. The last entry doubles as the
/// fallback when no other entry recognises a kind: today that is the
/// Rust planner, which accepts every rust-shaped kind and is the
/// expected target for hand-authored projects. Adding a language means
/// adding one entry here, no other dispatcher edits required.
const PLANNERS: &[LanguagePlanner] = &[
    LanguagePlanner {
        name: "apple",
        supports_kind: fabrik_apple::supports_kind,
        build_plan: apple_build_plan,
    },
    LanguagePlanner {
        name: "elixir",
        supports_kind: fabrik_elixir::supports_kind,
        build_plan: elixir_build_plan,
    },
    LanguagePlanner {
        name: "go",
        supports_kind: fabrik_go::supports_kind,
        build_plan: go_build_plan,
    },
    LanguagePlanner {
        name: "rust",
        supports_kind: |_| true,
        build_plan: rust_build_plan,
    },
];

/// Look up the planner that owns `kind`. Returns the first matching
/// entry in [`PLANNERS`]; order is intentional so apple/elixir/go win
/// before the rust fallback claims everything else.
#[must_use]
pub fn planner_for(kind: &str) -> &'static LanguagePlanner {
    PLANNERS
        .iter()
        .find(|p| (p.supports_kind)(kind))
        .expect("rust planner is the catch-all and must always match")
}

/// Build a plan for `target_id`, dispatching by kind.
pub fn plan_for_target(
    targets: &[Target],
    target_id: &str,
    workspace_root: &Path,
) -> Result<BuiltPlan> {
    let target = targets
        .iter()
        .find(|t| t.id() == target_id)
        .ok_or_else(|| {
            anyhow!(
                "no target matches `{target_id}`. Run `fabrik targets` to list declared targets"
            )
        })?;
    let planner = planner_for(&target.kind);
    (planner.build_plan)(targets, target_id, workspace_root)
}

fn apple_build_plan(
    targets: &[Target],
    target_id: &str,
    workspace_root: &Path,
) -> Result<BuiltPlan> {
    Ok(fabrik_apple::build_plan(
        targets,
        target_id,
        workspace_root,
    )?)
}

fn elixir_build_plan(
    targets: &[Target],
    target_id: &str,
    workspace_root: &Path,
) -> Result<BuiltPlan> {
    Ok(fabrik_elixir::build_plan(
        targets,
        target_id,
        workspace_root,
    )?)
}

fn go_build_plan(targets: &[Target], target_id: &str, workspace_root: &Path) -> Result<BuiltPlan> {
    Ok(fabrik_go::build_plan(targets, target_id, workspace_root)?)
}

fn rust_build_plan(
    targets: &[Target],
    target_id: &str,
    workspace_root: &Path,
) -> Result<BuiltPlan> {
    Ok(fabrik_rust::build_plan(targets, target_id, workspace_root)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planner_lookup_routes_each_supported_kind_to_its_owner() {
        assert_eq!(planner_for("apple_simulator_app").name, "apple");
        assert_eq!(planner_for("elixir_library").name, "elixir");
        assert_eq!(planner_for("go_binary").name, "go");
        assert_eq!(planner_for("rust_library").name, "rust");
        assert_eq!(planner_for("rust_binary").name, "rust");
    }

    #[test]
    fn planner_lookup_falls_back_to_rust_for_unknown_kinds() {
        // The rust planner is the catch-all; an unknown kind reaches it
        // and lets the per-crate planner surface the real error.
        assert_eq!(planner_for("totally_made_up_kind").name, "rust");
    }
}
