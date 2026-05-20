#![allow(clippy::implicit_hasher)]
//! Shared scaffolding for per-language `build_plan` implementations.
//!
//! Every language plugin walks a flat target list, filters to its own
//! kinds, topologically sorts what's reachable from a root, then drives
//! a language-specific compile step over the sorted order. The DFS,
//! the cycle detection, and the workspace-relative output path layout
//! are identical across plugins; only the compile step and the kind
//! predicate vary. This module exposes the invariants once so each
//! plugin's `plan.rs` reduces to "set up the predicate, then call the
//! shared sort + compile loop."

use std::collections::{BTreeSet, HashMap};

use crate::Target;

/// Errors the shared planner can surface without needing to know which
/// language plugin is calling it.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("no target matches `{0}`")]
    UnknownRoot(String),
    #[error("dependency cycle through `{0}`")]
    Cycle(String),
    #[error("dep `{dep}` of target {label} is not declared in any Fabrik build file")]
    MissingDep { label: String, dep: String },
}

/// Workspace-relative directory where a language plugin should place
/// the root output of `name` declared in `package`. Empty `package`
/// (workspace-root targets) collapses the layout to one level.
#[must_use]
pub fn workspace_output_dir(package: &str, name: &str) -> String {
    if package.is_empty() {
        format!(".fabrik/out/{name}")
    } else {
        format!(".fabrik/out/{package}/{name}")
    }
}

/// Build a `target id -> index` lookup over the workspace target list.
#[must_use]
pub fn target_index(targets: &[Target]) -> HashMap<String, usize> {
    targets
        .iter()
        .enumerate()
        .map(|(i, t)| (t.id(), i))
        .collect()
}

/// Resolve `root_id` to its index, or return an `UnknownRoot` error
/// shaped consistently across plugins.
pub fn resolve_root(
    target_index: &HashMap<String, usize>,
    root_id: &str,
) -> Result<usize, BuildError> {
    target_index
        .get(root_id)
        .copied()
        .ok_or_else(|| BuildError::UnknownRoot(root_id.to_string()))
}

/// Depth-first topological sort over the workspace target list.
///
/// `deps_by_target[i]` lists the declared dep ids of target `targets[i]`
/// (typically `target.deps` plus any externalized deps the caller has
/// already lowered to flat target ids). `traverse` is a predicate that
/// gates whether the walker descends through a dep target by kind: it
/// returns `true` for kinds the calling plugin compiles, and `false` for
/// kinds it should treat as an opaque leaf (so a rust target depending
/// on an elixir target doesn't make the elixir target part of the rust
/// plan). Returns a reverse-postorder traversal that is already a valid
/// topological sort.
pub fn topological_sort<F>(
    targets: &[Target],
    deps_by_target: &[Vec<String>],
    target_index: &HashMap<String, usize>,
    root_idx: usize,
    traverse: F,
) -> Result<Vec<usize>, BuildError>
where
    F: Fn(&str) -> bool,
{
    let mut order: Vec<usize> = Vec::new();
    let mut on_stack: BTreeSet<usize> = BTreeSet::new();
    let mut visited: BTreeSet<usize> = BTreeSet::new();
    dfs(
        root_idx,
        targets,
        deps_by_target,
        target_index,
        &traverse,
        &mut visited,
        &mut on_stack,
        &mut order,
    )?;
    Ok(order)
}

#[allow(clippy::too_many_arguments)]
fn dfs<F>(
    idx: usize,
    targets: &[Target],
    deps_by_target: &[Vec<String>],
    target_index: &HashMap<String, usize>,
    traverse: &F,
    visited: &mut BTreeSet<usize>,
    on_stack: &mut BTreeSet<usize>,
    order: &mut Vec<usize>,
) -> Result<(), BuildError>
where
    F: Fn(&str) -> bool,
{
    if visited.contains(&idx) {
        return Ok(());
    }
    if on_stack.contains(&idx) {
        return Err(BuildError::Cycle(targets[idx].id()));
    }
    on_stack.insert(idx);
    for dep in &deps_by_target[idx] {
        let dep_idx = target_index
            .get(dep)
            .copied()
            .ok_or_else(|| BuildError::MissingDep {
                label: targets[idx].id(),
                dep: dep.clone(),
            })?;
        if traverse(&targets[dep_idx].kind) {
            dfs(
                dep_idx,
                targets,
                deps_by_target,
                target_index,
                traverse,
                visited,
                on_stack,
                order,
            )?;
        }
    }
    on_stack.remove(&idx);
    visited.insert(idx);
    order.push(idx);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn target(pkg: &str, name: &str, kind: &str, deps: &[&str]) -> Target {
        Target {
            package: pkg.into(),
            external_package: None,
            kind: kind.into(),
            name: name.into(),
            srcs: Vec::new(),
            deps: deps.iter().map(|s| (*s).to_string()).collect(),
            external_deps: Vec::new(),
            attrs: BTreeMap::new(),
        }
    }

    #[test]
    fn workspace_output_dir_with_and_without_package() {
        assert_eq!(workspace_output_dir("", "hello"), ".fabrik/out/hello");
        assert_eq!(
            workspace_output_dir("pkg", "hello"),
            ".fabrik/out/pkg/hello"
        );
    }

    #[test]
    fn topological_sort_orders_deps_before_dependents() {
        let targets = vec![
            target("base", "base", "x_lib", &[]),
            target("a", "a", "x_lib", &["base/base"]),
            target("top", "top", "x_bin", &["a/a"]),
        ];
        let idx = target_index(&targets);
        let deps = vec![
            targets[0].deps.clone(),
            targets[1].deps.clone(),
            targets[2].deps.clone(),
        ];
        let order = topological_sort(&targets, &deps, &idx, 2, |k| k.starts_with("x_")).unwrap();
        let label = |i: usize| targets[i].id();
        let positions: Vec<_> = order.iter().map(|i| label(*i)).collect();
        assert_eq!(positions, vec!["base/base", "a/a", "top/top"]);
    }

    #[test]
    fn topological_sort_detects_cycles() {
        let targets = vec![
            target("a", "a", "x_lib", &["b/b"]),
            target("b", "b", "x_lib", &["a/a"]),
        ];
        let idx = target_index(&targets);
        let deps = vec![targets[0].deps.clone(), targets[1].deps.clone()];
        let err = topological_sort(&targets, &deps, &idx, 0, |k| k.starts_with("x_")).unwrap_err();
        assert!(matches!(err, BuildError::Cycle(_)));
    }

    #[test]
    fn topological_sort_skips_traversal_for_unsupported_kinds() {
        // `top` declares a dep on `other/other` whose kind isn't ours;
        // the predicate returns false so we don't descend, but we still
        // require the dep id to exist (so a typo on a foreign kind is
        // still a MissingDep).
        let targets = vec![
            target("other", "other", "y_lib", &[]),
            target("top", "top", "x_bin", &["other/other"]),
        ];
        let idx = target_index(&targets);
        let deps = vec![targets[0].deps.clone(), targets[1].deps.clone()];
        let order = topological_sort(&targets, &deps, &idx, 1, |k| k.starts_with("x_")).unwrap();
        assert_eq!(order, vec![1]);
    }

    #[test]
    fn topological_sort_reports_missing_dep() {
        let targets = vec![target("top", "top", "x_bin", &["ghost/ghost"])];
        let idx = target_index(&targets);
        let deps = vec![targets[0].deps.clone()];
        let err = topological_sort(&targets, &deps, &idx, 0, |_| true).unwrap_err();
        assert!(matches!(err, BuildError::MissingDep { .. }));
    }
}
