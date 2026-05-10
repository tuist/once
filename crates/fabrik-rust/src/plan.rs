//! Workspace-wide plan assembly: take a flat list of declared targets
//! and a root target, resolve transitive deps, and produce a single
//! [`fabrik_core::Plan`] whose node ordering and edges respect every
//! declared dependency.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use fabrik_core::{Action, BuiltPlan, NodeInfo, Plan};
use fabrik_frontend::Target;

use crate::artifact::{DepArtifact, RustKind};
use crate::build_script::{compile_build_script, output_path as build_script_output_path};
use crate::compile::{compile_target, CompileError};

#[derive(Debug, thiserror::Error)]
pub enum PlanBuildError {
    #[error("no target matches `{0}`")]
    UnknownRoot(String),
    #[error("dependency cycle through `{0}`")]
    Cycle(String),
    #[error(transparent)]
    Compile(#[from] CompileError),
    #[error("dep `{dep}` of target {label} is not declared in any Fabrik build file")]
    MissingDep { label: String, dep: String },
    #[error("dep `{dep}` of target {label} is not a rust target (kind `{kind}`)")]
    NonRustDep {
        label: String,
        dep: String,
        kind: String,
    },
}

/// Build a plan for `root_id` from a flat target list.
///
/// `targets` is the workspace-wide vector returned by
/// `fabrik_frontend::load_workspace`. `root_id` selects the entry
/// target; every reachable Rust target becomes one plan node, with
/// edges following the declared `deps`. Targets with non-Rust kinds
/// referenced as deps surface as a typed error rather than being
/// silently dropped.
pub fn build_plan(
    targets: &[Target],
    root_id: &str,
    workspace_root: &Path,
) -> Result<BuiltPlan, PlanBuildError> {
    let target_index: HashMap<String, usize> = targets
        .iter()
        .enumerate()
        .map(|(i, t)| (t.id(), i))
        .collect();
    let root_target_idx = *target_index
        .get(root_id)
        .ok_or_else(|| PlanBuildError::UnknownRoot(root_id.to_string()))?;

    // DFS to collect every target reachable from root, returning a
    // reverse-postorder traversal that's already a valid topological
    // sort.
    let mut order: Vec<usize> = Vec::new();
    let mut on_stack: BTreeSet<usize> = BTreeSet::new();
    let mut visited: BTreeSet<usize> = BTreeSet::new();
    dfs(
        root_target_idx,
        targets,
        &target_index,
        &mut visited,
        &mut on_stack,
        &mut order,
    )?;

    // Compile in topological order so each target's deps are already
    // present in `dep_artifacts`.
    let mut plan = Plan::new();
    let mut node_info = Vec::with_capacity(order.len());
    let mut dep_artifacts: BTreeMap<String, DepArtifact> = BTreeMap::new();
    let mut id_to_plan_idx: HashMap<String, usize> = HashMap::new();
    let mut root_index: Option<usize> = None;

    for target_idx in &order {
        let target = &targets[*target_idx];

        // cargo_build_script targets compile via a different handler
        // and have no rustc-style outputs that dependents can --extern
        // against; they're build-order leaves with a captured-stdout
        // artifact. Treat them specially before the rust dispatch.
        if target.kind == "cargo_build_script" {
            let mut node = compile_build_script(target, workspace_root)?;
            node.deps = target
                .deps
                .iter()
                .filter_map(|d| id_to_plan_idx.get(d).copied())
                .collect();
            let label = target.id();
            let action_digest = node.action.digest();
            let plan_idx = plan.push(node);
            id_to_plan_idx.insert(label.clone(), plan_idx);
            node_info.push(NodeInfo {
                label: label.clone(),
                kind: RustKind::BuildScript.as_str().to_string(),
            });
            dep_artifacts.insert(
                label.clone(),
                DepArtifact {
                    crate_name: format!("{}_build_script", target.name),
                    extern_path: String::new(),
                    rmeta_path: String::new(),
                    out_dir: String::new(),
                    action_digest,
                    kind: RustKind::BuildScript,
                    build_script_outputs: Some(build_script_output_path(
                        &target.package,
                        &target.name,
                    )),
                },
            );
            if *target_idx == root_target_idx {
                root_index = Some(plan_idx);
            }
            continue;
        }

        // Walk the target's deps; missing deps and non-rust deps
        // produce typed errors here rather than failing inside
        // compile_target.
        for dep in &target.deps {
            let dep_target_idx =
                target_index
                    .get(dep)
                    .ok_or_else(|| PlanBuildError::MissingDep {
                        label: target.id(),
                        dep: dep.clone(),
                    })?;
            let dep_kind = &targets[*dep_target_idx].kind;
            if RustKind::parse(dep_kind).is_none() {
                return Err(PlanBuildError::NonRustDep {
                    label: target.id(),
                    dep: dep.clone(),
                    kind: dep_kind.clone(),
                });
            }
        }
        let (mut node, artifact) = compile_target(target, workspace_root, &dep_artifacts)?;

        // Attach plan-node deps now that we know their indices.
        node.deps = target
            .deps
            .iter()
            .filter_map(|d| id_to_plan_idx.get(d).copied())
            .collect();

        let plan_idx = plan.push(node);
        id_to_plan_idx.insert(target.id(), plan_idx);
        node_info.push(NodeInfo {
            label: target.id(),
            kind: artifact.kind.as_str().to_string(),
        });
        dep_artifacts.insert(target.id(), artifact);
        if *target_idx == root_target_idx {
            root_index = Some(plan_idx);
        }
    }

    let root_idx = root_index.expect("root was visited");
    let output = root_output_path(&plan.nodes[root_idx]);
    Ok(BuiltPlan {
        plan,
        root_index: root_idx,
        root_id: root_id.to_string(),
        output,
        nodes: node_info,
    })
}

/// First declared output of the root node, or empty if the plugin's
/// root has none (e.g. a build-script root). Mirrors the previous
/// behavior the CLI wrapped this plan in: callers wanted "the file the
/// build produced" and pulled it from `plan.nodes[root_index].action`.
fn root_output_path(node: &fabrik_core::PlanNode) -> String {
    match &node.action {
        Action::RunCommand { outputs, .. } => outputs
            .first()
            .map(|p| p.as_str().to_string())
            .unwrap_or_default(),
    }
}

fn dfs(
    idx: usize,
    targets: &[Target],
    target_index: &HashMap<String, usize>,
    visited: &mut BTreeSet<usize>,
    on_stack: &mut BTreeSet<usize>,
    order: &mut Vec<usize>,
) -> Result<(), PlanBuildError> {
    if visited.contains(&idx) {
        return Ok(());
    }
    if on_stack.contains(&idx) {
        return Err(PlanBuildError::Cycle(targets[idx].id()));
    }
    on_stack.insert(idx);
    for dep in &targets[idx].deps {
        let dep_idx = target_index
            .get(dep)
            .ok_or_else(|| PlanBuildError::MissingDep {
                label: targets[idx].id(),
                dep: dep.clone(),
            })?;
        // Non-rust deps would fail at compile time too; we only walk
        // them here when they are rust-shaped targets so the cycle
        // detector does not misreport a rust -> non-rust edge.
        let dep_kind = &targets[*dep_idx].kind;
        if RustKind::parse(dep_kind).is_some() {
            dfs(*dep_idx, targets, target_index, visited, on_stack, order)?;
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
    use tempfile::TempDir;

    fn write(workspace: &std::path::Path, rel: &str, body: &str) {
        let p = workspace.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn lib(pkg: &str, name: &str, srcs: &[&str], deps: &[&str]) -> Target {
        Target {
            package: pkg.into(),
            kind: "rust_library".into(),
            name: name.into(),
            srcs: srcs.iter().map(|s| (*s).to_string()).collect(),
            deps: deps.iter().map(|s| (*s).to_string()).collect(),
            attrs: BTreeMap::new(),
        }
    }

    fn bin(pkg: &str, name: &str, srcs: &[&str], deps: &[&str]) -> Target {
        Target {
            package: pkg.into(),
            kind: "rust_binary".into(),
            name: name.into(),
            srcs: srcs.iter().map(|s| (*s).to_string()).collect(),
            deps: deps.iter().map(|s| (*s).to_string()).collect(),
            attrs: BTreeMap::new(),
        }
    }

    #[test]
    fn unknown_root_id_is_an_error() {
        let tmp = TempDir::new().unwrap();
        let err = build_plan(&[], "nope/nope", tmp.path()).unwrap_err();
        assert!(matches!(err, PlanBuildError::UnknownRoot(_)));
    }

    #[test]
    fn diamond_dep_graph_topologically_sorts() {
        // top -> {a, b} -> base
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "base/src/lib.rs", "pub fn x() {}");
        write(tmp.path(), "a/src/lib.rs", "pub use base::x;");
        write(tmp.path(), "b/src/lib.rs", "pub use base::x;");
        write(tmp.path(), "top/src/main.rs", "fn main() {}");
        let targets = vec![
            lib("base", "base", &["src/lib.rs"], &[]),
            lib("a", "a", &["src/lib.rs"], &["base/base"]),
            lib("b", "b", &["src/lib.rs"], &["base/base"]),
            bin("top", "top", &["src/main.rs"], &["a/a", "b/b"]),
        ];
        let built = build_plan(&targets, "top/top", tmp.path()).unwrap();
        assert_eq!(built.plan.nodes.len(), 4);
        // The root is the binary; its deps must precede it in the
        // plan and reference the right indices.
        let root = &built.plan.nodes[built.root_index];
        assert_eq!(root.label, "top/top");
        assert_eq!(root.deps.len(), 2);
        for d in &root.deps {
            assert!(*d < built.root_index, "deps must precede root");
        }
    }

    #[test]
    fn cycle_through_rust_targets_surfaces_as_typed_error() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "a/src/lib.rs", "");
        write(tmp.path(), "b/src/lib.rs", "");
        let targets = vec![
            lib("a", "a", &["src/lib.rs"], &["b/b"]),
            lib("b", "b", &["src/lib.rs"], &["a/a"]),
        ];
        let err = build_plan(&targets, "a/a", tmp.path()).unwrap_err();
        assert!(matches!(err, PlanBuildError::Cycle(_)));
    }

    #[test]
    fn missing_dep_id_is_an_error() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "a/src/lib.rs", "");
        let targets = vec![lib("a", "a", &["src/lib.rs"], &["ghost/ghost"])];
        let err = build_plan(&targets, "a/a", tmp.path()).unwrap_err();
        assert!(matches!(err, PlanBuildError::MissingDep { .. }));
    }

    #[test]
    fn cargo_build_script_target_becomes_a_plan_node() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "pkg/build.rs", "fn main() {}");
        let target = Target {
            package: "pkg".into(),
            kind: "cargo_build_script".into(),
            name: "build".into(),
            srcs: vec!["build.rs".into()],
            deps: vec![],
            attrs: BTreeMap::new(),
        };
        let built = build_plan(&[target], "pkg/build", tmp.path()).unwrap();
        assert_eq!(built.plan.nodes.len(), 1);
        assert_eq!(built.plan.nodes[0].label, "pkg/build");
        assert_eq!(built.nodes[0].kind, "cargo_build_script");
        // The action's argv must reference build.rs and run via /bin/sh
        // (the build_script handler emits a single shell pipeline).
        let fabrik_core::Action::RunCommand { argv, outputs, .. } = &built.plan.nodes[0].action;
        assert_eq!(argv[0], "/bin/sh");
        assert_eq!(argv[1], "-c");
        assert!(
            argv[2].contains("pkg/build.rs"),
            "argv[2] missing build.rs: {}",
            argv[2]
        );
        assert_eq!(outputs.len(), 1);
        assert!(outputs[0].as_str().ends_with("build_script.out"));
    }

    #[test]
    fn unreachable_targets_are_omitted_from_plan() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "a/src/lib.rs", "");
        write(tmp.path(), "b/src/lib.rs", "");
        let targets = vec![
            lib("a", "a", &["src/lib.rs"], &[]),
            lib("b", "b", &["src/lib.rs"], &[]),
        ];
        let built = build_plan(&targets, "a/a", tmp.path()).unwrap();
        assert_eq!(built.plan.nodes.len(), 1);
        assert_eq!(built.plan.nodes[0].label, "a/a");
    }
}
