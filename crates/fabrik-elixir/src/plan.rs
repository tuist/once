//! Workspace-wide plan assembly: take a flat list of declared targets
//! and a root target, resolve transitive deps, and produce a single
//! [`fabrik_core::Plan`] whose node ordering and edges respect every
//! declared dependency.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use fabrik_core::{Action, BuiltPlan, NodeInfo, Plan};
use fabrik_frontend::Target;

use crate::artifact::{BeamArtifact, ElixirKind};
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
    #[error("dep `{dep}` of target {label} is not an elixir target (kind `{kind}`)")]
    NonElixirDep {
        label: String,
        dep: String,
        kind: String,
    },
    #[error("external dep `{graph}` of target {label} must use a string package name")]
    InvalidExternalDepSpec { label: String, graph: String },
}

/// Quick check used by the CLI dispatcher to pick between language
/// planners.
pub fn supports_kind(kind: &str) -> bool {
    ElixirKind::parse(kind).is_some()
}

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
    let deps_by_target = target_dep_ids(targets)?;

    let mut order: Vec<usize> = Vec::new();
    let mut on_stack: BTreeSet<usize> = BTreeSet::new();
    let mut visited: BTreeSet<usize> = BTreeSet::new();
    dfs(
        root_target_idx,
        targets,
        &deps_by_target,
        &target_index,
        &mut visited,
        &mut on_stack,
        &mut order,
    )?;

    let mut plan = Plan::new();
    let mut node_info: Vec<NodeInfo> = Vec::with_capacity(order.len());
    let mut dep_artifacts: BTreeMap<String, BeamArtifact> = BTreeMap::new();
    let mut id_to_plan_idx: HashMap<String, usize> = HashMap::new();
    let mut root_index: Option<usize> = None;

    for target_idx in &order {
        let target = &targets[*target_idx];
        let deps = &deps_by_target[*target_idx];

        for dep in deps {
            let dep_target_idx =
                target_index
                    .get(dep)
                    .ok_or_else(|| PlanBuildError::MissingDep {
                        label: target.id(),
                        dep: dep.clone(),
                    })?;
            let dep_kind = &targets[*dep_target_idx].kind;
            if ElixirKind::parse(dep_kind).is_none() {
                return Err(PlanBuildError::NonElixirDep {
                    label: target.id(),
                    dep: dep.clone(),
                    kind: dep_kind.clone(),
                });
            }
        }

        let mut target_for_compile = target.clone();
        target_for_compile.deps.clone_from(deps);
        target_for_compile.external_deps = Vec::new();
        let (mut node, artifact) =
            compile_target(&target_for_compile, workspace_root, &dep_artifacts)?;
        node.deps = deps
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

fn root_output_path(node: &fabrik_core::PlanNode) -> String {
    match &node.action {
        Action::RunCommand { outputs, .. } => outputs
            .first()
            .map(|p| p.as_str().to_string())
            .unwrap_or_default(),
    }
}

fn target_dep_ids(targets: &[Target]) -> Result<Vec<Vec<String>>, PlanBuildError> {
    targets.iter().map(target_dep_id).collect()
}

fn target_dep_id(target: &Target) -> Result<Vec<String>, PlanBuildError> {
    let mut deps = target.deps.clone();
    for dep in &target.external_deps {
        let Some(package_name) = dep.spec.as_str() else {
            return Err(PlanBuildError::InvalidExternalDepSpec {
                label: target.id(),
                graph: dep.graph.clone(),
            });
        };
        deps.push(format!("vendor/{}/{package_name}", dep.graph));
    }
    Ok(deps)
}

fn dfs(
    idx: usize,
    targets: &[Target],
    deps_by_target: &[Vec<String>],
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
    for dep in &deps_by_target[idx] {
        let dep_idx = target_index
            .get(dep)
            .ok_or_else(|| PlanBuildError::MissingDep {
                label: targets[idx].id(),
                dep: dep.clone(),
            })?;
        let dep_kind = &targets[*dep_idx].kind;
        if ElixirKind::parse(dep_kind).is_some() {
            dfs(
                *dep_idx,
                targets,
                deps_by_target,
                target_index,
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
    use tempfile::TempDir;

    fn write(workspace: &std::path::Path, rel: &str, body: &str) {
        let p = workspace.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn lib(pkg: &str, name: &str, srcs: &[&str], deps: &[&str]) -> Target {
        Target {
            package: pkg.into(),
            kind: "elixir_library".into(),
            name: name.into(),
            srcs: srcs.iter().map(|s| (*s).to_string()).collect(),
            deps: deps.iter().map(|s| (*s).to_string()).collect(),
            external_deps: Vec::new(),
            attrs: BTreeMap::new(),
        }
    }

    fn bin(pkg: &str, name: &str, srcs: &[&str], deps: &[&str], entry: &str) -> Target {
        let mut attrs = BTreeMap::new();
        attrs.insert("entry".into(), entry.into());
        Target {
            package: pkg.into(),
            kind: "elixir_binary".into(),
            name: name.into(),
            srcs: srcs.iter().map(|s| (*s).to_string()).collect(),
            deps: deps.iter().map(|s| (*s).to_string()).collect(),
            external_deps: Vec::new(),
            attrs,
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
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "base/lib/base.ex", "defmodule Base do\nend\n");
        write(tmp.path(), "a/lib/a.ex", "defmodule A do\nend\n");
        write(tmp.path(), "b/lib/b.ex", "defmodule B do\nend\n");
        write(
            tmp.path(),
            "top/lib/top.ex",
            "defmodule Top do\n  def main(_), do: :ok\nend\n",
        );
        let targets = vec![
            lib("base", "base", &["lib/base.ex"], &[]),
            lib("a", "a", &["lib/a.ex"], &["base/base"]),
            lib("b", "b", &["lib/b.ex"], &["base/base"]),
            bin("top", "top", &["lib/top.ex"], &["a/a", "b/b"], "Top"),
        ];
        let built = build_plan(&targets, "top/top", tmp.path()).unwrap();
        assert_eq!(built.plan.nodes.len(), 4);
        let root = &built.plan.nodes[built.root_index];
        assert_eq!(root.label, "top/top");
        for d in &root.deps {
            assert!(*d < built.root_index, "deps must precede root");
        }
    }

    #[test]
    fn cycle_through_elixir_targets_surfaces_as_typed_error() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "a/lib/a.ex", "");
        write(tmp.path(), "b/lib/b.ex", "");
        let targets = vec![
            lib("a", "a", &["lib/a.ex"], &["b/b"]),
            lib("b", "b", &["lib/b.ex"], &["a/a"]),
        ];
        let err = build_plan(&targets, "a/a", tmp.path()).unwrap_err();
        assert!(matches!(err, PlanBuildError::Cycle(_)));
    }

    #[test]
    fn missing_dep_id_is_an_error() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "a/lib/a.ex", "");
        let targets = vec![lib("a", "a", &["lib/a.ex"], &["ghost/ghost"])];
        let err = build_plan(&targets, "a/a", tmp.path()).unwrap_err();
        assert!(matches!(err, PlanBuildError::MissingDep { .. }));
    }

    #[test]
    fn unreachable_targets_are_omitted_from_plan() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "a/lib/a.ex", "");
        write(tmp.path(), "b/lib/b.ex", "");
        let targets = vec![
            lib("a", "a", &["lib/a.ex"], &[]),
            lib("b", "b", &["lib/b.ex"], &[]),
        ];
        let built = build_plan(&targets, "a/a", tmp.path()).unwrap();
        assert_eq!(built.plan.nodes.len(), 1);
        assert_eq!(built.plan.nodes[0].label, "a/a");
    }

    #[test]
    fn external_mix_dep_lowers_to_generated_vendor_target() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "app/lib/app.ex", "defmodule App do\nend\n");
        write(
            tmp.path(),
            "vendor/mix/decimal/lib/decimal.ex",
            "defmodule Decimal do\nend\n",
        );
        let mut app = lib("app", "app", &["lib/app.ex"], &[]);
        app.external_deps.push(fabrik_frontend::ExternalDependency {
            graph: "mix".to_string(),
            spec: serde_json::Value::String("decimal".to_string()),
        });
        let targets = vec![
            lib("vendor/mix", "decimal", &["decimal/lib/decimal.ex"], &[]),
            app,
        ];

        let built = build_plan(&targets, "app/app", tmp.path()).unwrap();

        assert_eq!(built.plan.nodes.len(), 2);
        assert_eq!(built.nodes[0].label, "vendor/mix/decimal");
        assert_eq!(built.plan.nodes[built.root_index].deps, vec![0]);
    }

    #[test]
    fn supports_kind_recognises_elixir_kinds_only() {
        assert!(supports_kind("elixir_library"));
        assert!(supports_kind("elixir_binary"));
        assert!(!supports_kind("elixir_test"));
        assert!(!supports_kind("rust_library"));
        assert!(!supports_kind("apple_ios_app"));
    }
}
