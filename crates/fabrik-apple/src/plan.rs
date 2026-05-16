use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use fabrik_core::{BuiltPlan, NodeInfo, Plan};
use fabrik_frontend::{external_dep_id, Target};

use crate::artifact::{AppleKind, SwiftArtifact};
use crate::compile::{compile_ios_app, AppleError};
use crate::swift::{compile_swift_target, SwiftError, TargetDepMode};

#[derive(Debug, thiserror::Error)]
pub enum PlanBuildError {
    #[error("no target matches `{0}`")]
    UnknownRoot(String),
    #[error("apple_simulator_app target {label} does not support deps yet")]
    UnsupportedDeps { label: String },
    #[error("dependency cycle through `{0}`")]
    Cycle(String),
    #[error("dep `{dep}` of target {label} is not declared in any Fabrik build file")]
    MissingDep { label: String, dep: String },
    #[error("dep `{dep}` of target {label} is not an Apple target (kind `{kind}`)")]
    NonAppleDep {
        label: String,
        dep: String,
        kind: String,
    },
    #[error(
        "external dep `{graph}` of target {label} must use a string product name or product object"
    )]
    InvalidExternalDepSpec { label: String, graph: String },
    #[error(transparent)]
    Apple(#[from] AppleError),
    #[error(transparent)]
    Swift(#[from] SwiftError),
}

pub fn supports_kind(kind: &str) -> bool {
    AppleKind::parse(kind).is_some()
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
    let target = targets
        .iter()
        .find(|t| t.id() == root_id)
        .ok_or_else(|| PlanBuildError::UnknownRoot(root_id.to_string()))?;
    let kind = AppleKind::parse(&target.kind).ok_or_else(|| PlanBuildError::NonAppleDep {
        label: target.id(),
        dep: root_id.to_string(),
        kind: target.kind.clone(),
    })?;
    if kind == AppleKind::SimulatorApp {
        return build_ios_plan(target, root_id, workspace_root);
    }
    let deps_by_target = target_dep_ids(targets)?;

    let mut order = Vec::new();
    let mut on_stack = BTreeSet::new();
    let mut visited = BTreeSet::new();
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
    let mut nodes = Vec::with_capacity(order.len() * 2);
    let mut dep_artifacts: BTreeMap<String, SwiftArtifact> = BTreeMap::new();
    let mut id_to_plan_idx: HashMap<String, usize> = HashMap::new();
    let mut id_to_import_idx: HashMap<String, usize> = HashMap::new();
    let mut root_index = None;
    let mut output = String::new();

    for target_idx in &order {
        let target = &targets[*target_idx];
        let deps = &deps_by_target[*target_idx];
        validate_swift_deps(target, deps, targets, &target_index)?;
        let mut target_for_compile = target.clone();
        target_for_compile.deps.clone_from(deps);
        target_for_compile.external_deps = Vec::new();
        let (plan_idx, import_idx, artifact) = push_swift_target(
            &mut plan,
            &mut nodes,
            &target_for_compile,
            workspace_root,
            &dep_artifacts,
            &id_to_plan_idx,
            &id_to_import_idx,
        )?;
        let label = target.id();
        id_to_plan_idx.insert(label.clone(), plan_idx);
        id_to_import_idx.insert(label.clone(), import_idx);
        if *target_idx == root_target_idx {
            root_index = Some(plan_idx);
            output.clone_from(&artifact.output);
        }
        dep_artifacts.insert(label, artifact);
    }

    Ok(BuiltPlan {
        plan,
        root_index: root_index.expect("root was visited"),
        root_id: root_id.to_string(),
        output,
        nodes,
    })
}

fn build_ios_plan(
    target: &Target,
    root_id: &str,
    workspace_root: &Path,
) -> Result<BuiltPlan, PlanBuildError> {
    let dep_ids = target_dep_id(target)?;
    if !dep_ids.is_empty() {
        return Err(PlanBuildError::UnsupportedDeps { label: target.id() });
    }
    let node = compile_ios_app(target, workspace_root)?;
    let output_package = target.output_package();
    let output = crate::app_bundle_path(output_package.as_ref(), &target.name);
    let mut plan = Plan::new();
    let root_index = plan.push(node);
    Ok(BuiltPlan {
        plan,
        root_index,
        root_id: root_id.to_string(),
        output,
        nodes: vec![NodeInfo {
            label: target.id(),
            kind: target.kind.clone(),
        }],
    })
}

fn validate_swift_deps(
    target: &Target,
    dep_ids: &[String],
    targets: &[Target],
    target_index: &HashMap<String, usize>,
) -> Result<(), PlanBuildError> {
    for dep in dep_ids {
        let dep_target_idx = target_index
            .get(dep)
            .ok_or_else(|| PlanBuildError::MissingDep {
                label: target.id(),
                dep: dep.clone(),
            })?;
        let dep_kind = &targets[*dep_target_idx].kind;
        if !supports_kind(dep_kind) || is_simulator_app_kind(dep_kind) {
            return Err(PlanBuildError::NonAppleDep {
                label: target.id(),
                dep: dep.clone(),
                kind: dep_kind.clone(),
            });
        }
    }
    Ok(())
}

fn target_dep_ids(targets: &[Target]) -> Result<Vec<Vec<String>>, PlanBuildError> {
    targets.iter().map(target_dep_id).collect()
}

fn target_dep_id(target: &Target) -> Result<Vec<String>, PlanBuildError> {
    let mut deps = target.deps.clone();
    for dep in &target.external_deps {
        let Some(product_name) = swift_product_name(&dep.spec) else {
            return Err(PlanBuildError::InvalidExternalDepSpec {
                label: target.id(),
                graph: dep.graph.clone(),
            });
        };
        deps.push(external_dep_id(&dep.graph, product_name));
    }
    Ok(deps)
}

fn swift_product_name(spec: &serde_json::Value) -> Option<&str> {
    spec.as_str()
        .or_else(|| spec.get("product").and_then(serde_json::Value::as_str))
}

fn push_swift_target(
    plan: &mut Plan,
    nodes: &mut Vec<NodeInfo>,
    target: &Target,
    workspace_root: &Path,
    dep_artifacts: &BTreeMap<String, SwiftArtifact>,
    id_to_plan_idx: &HashMap<String, usize>,
    id_to_import_idx: &HashMap<String, usize>,
) -> Result<(usize, usize, SwiftArtifact), PlanBuildError> {
    let swift_plan = compile_swift_target(target, workspace_root, dep_artifacts)?;
    let root_dep_indices = target
        .deps
        .iter()
        .filter_map(|d| id_to_plan_idx.get(d).copied())
        .collect::<Vec<_>>();
    let import_dep_indices = target
        .deps
        .iter()
        .filter_map(|d| id_to_import_idx.get(d).copied())
        .collect::<Vec<_>>();
    let base_index = plan.nodes.len();
    let emitted_node_count = swift_plan.nodes.len();
    let import_index = base_index + swift_plan.import_node;
    let mut target_root_index = None;

    for (local_index, mut swift_node) in swift_plan.nodes.into_iter().enumerate() {
        let local_deps = std::mem::take(&mut swift_node.node.deps);
        swift_node.node.deps = local_deps
            .into_iter()
            .map(|dep| base_index + dep)
            .collect::<Vec<_>>();
        match swift_node.target_dep_mode {
            TargetDepMode::None => {}
            TargetDepMode::Root => {
                swift_node
                    .node
                    .deps
                    .extend(root_dep_indices.iter().copied());
            }
            TargetDepMode::Import => {
                swift_node
                    .node
                    .deps
                    .extend(import_dep_indices.iter().copied());
            }
        }

        let node_label = swift_node.node.label.clone();
        let node_kind = swift_node.kind;
        let plan_idx = plan.push(swift_node.node);
        if local_index + 1 == emitted_node_count {
            target_root_index = Some(plan_idx);
        }
        nodes.push(NodeInfo {
            label: node_label,
            kind: node_kind,
        });
    }

    Ok((
        target_root_index.expect("swift target emitted at least one node"),
        import_index,
        swift_plan.artifact,
    ))
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
        if supports_kind(dep_kind) && !is_simulator_app_kind(dep_kind) {
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

fn is_simulator_app_kind(kind: &str) -> bool {
    matches!(kind, "apple_ios_app" | "apple_simulator_app")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    #[test]
    fn builds_single_node_plan_for_ios_app() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("App")).unwrap();
        std::fs::write(tmp.path().join("App/App.swift"), "import SwiftUI").unwrap();
        let mut attrs = BTreeMap::new();
        attrs.insert("platform".to_string(), "ios".to_string());
        attrs.insert("bundle_id".to_string(), "dev.fabrik.demo".to_string());
        let target = Target {
            package: "App".to_string(),
            external_package: None,
            kind: "apple_simulator_app".to_string(),
            name: "Demo".to_string(),
            srcs: vec!["App.swift".to_string()],
            deps: Vec::new(),
            external_deps: Vec::new(),
            attrs,
        };
        let built = build_plan(&[target], "App/Demo", tmp.path()).unwrap();
        assert_eq!(built.plan.nodes.len(), 1);
        assert_eq!(built.output, ".fabrik/out/App/Demo.app");
    }

    #[derive(Clone, Copy)]
    struct SwiftTargetFixture<'a> {
        package: &'a str,
        external_package: Option<&'a str>,
        kind: &'a str,
        name: &'a str,
        srcs: &'a [&'a str],
        deps: &'a [&'a str],
    }

    fn swift_target(fixture: SwiftTargetFixture<'_>) -> Target {
        Target {
            package: fixture.package.into(),
            external_package: fixture.external_package.map(str::to_string),
            kind: fixture.kind.into(),
            name: fixture.name.into(),
            srcs: fixture.srcs.iter().map(|s| (*s).to_string()).collect(),
            deps: fixture.deps.iter().map(|s| (*s).to_string()).collect(),
            external_deps: Vec::new(),
            attrs: BTreeMap::new(),
        }
    }

    fn dependency_labels(built: &BuiltPlan, node_index: usize) -> Vec<&str> {
        built.plan.nodes[node_index]
            .deps
            .iter()
            .map(|index| built.nodes[*index].label.as_str())
            .collect()
    }

    #[test]
    fn builds_swift_dependency_graph() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("Lib")).unwrap();
        std::fs::create_dir_all(tmp.path().join("Mid")).unwrap();
        std::fs::create_dir_all(tmp.path().join("App")).unwrap();
        std::fs::write(
            tmp.path().join("Lib/Lib.swift"),
            "public func greeting() {}",
        )
        .unwrap();
        std::fs::write(tmp.path().join("Mid/Mid.swift"), "import Lib").unwrap();
        std::fs::write(tmp.path().join("App/main.swift"), "import Mid").unwrap();
        let targets = vec![
            swift_target(SwiftTargetFixture {
                package: "Lib",
                external_package: None,
                kind: "swift_library",
                name: "Lib",
                srcs: &["Lib.swift"],
                deps: &[],
            }),
            swift_target(SwiftTargetFixture {
                package: "Mid",
                external_package: None,
                kind: "swift_library",
                name: "Mid",
                srcs: &["Mid.swift"],
                deps: &["Lib/Lib"],
            }),
            swift_target(SwiftTargetFixture {
                package: "App",
                external_package: None,
                kind: "macos_command_line_application",
                name: "hello",
                srcs: &["main.swift"],
                deps: &["Mid/Mid"],
            }),
        ];
        let built = build_plan(&targets, "App/hello", tmp.path()).unwrap();
        assert_eq!(built.plan.nodes.len(), 5);
        assert_eq!(built.output, ".fabrik/out/App/hello");
        assert_eq!(built.plan.nodes[1].deps, vec![0]);
        assert_eq!(built.plan.nodes[2].deps, vec![0]);
        assert_eq!(built.plan.nodes[3].deps, vec![2]);
        assert_eq!(built.plan.nodes[built.root_index].deps, vec![3]);
        assert_eq!(built.nodes[0].label, "Lib/Lib#compile");
        assert_eq!(built.nodes[0].kind, "swift_compile");
        assert_eq!(built.nodes[1].label, "Lib/Lib#archive");
        assert_eq!(built.nodes[1].kind, "swift_archive");
        assert_eq!(built.nodes[2].label, "Mid/Mid#compile");
        assert_eq!(built.nodes[2].kind, "swift_compile");
        assert_eq!(built.nodes[3].label, "Mid/Mid#archive");
        assert_eq!(built.nodes[3].kind, "swift_archive");
        assert_eq!(built.nodes[4].kind, "macos_command_line_application");
    }

    #[test]
    fn external_swiftpm_product_depends_on_external_target() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("App")).unwrap();
        std::fs::create_dir_all(tmp.path().join(".fabrik/external/swiftpm")).unwrap();
        std::fs::write(tmp.path().join("App/main.swift"), "import ArgumentParser").unwrap();
        std::fs::write(
            tmp.path()
                .join(".fabrik/external/swiftpm/ArgumentParser.swift"),
            "public struct Parser {}",
        )
        .unwrap();
        let external_dep = swift_target(SwiftTargetFixture {
            package: ".fabrik/external/swiftpm",
            external_package: Some("swiftpm"),
            kind: "swift_library",
            name: "ArgumentParser",
            srcs: &["ArgumentParser.swift"],
            deps: &[],
        });
        let mut app = swift_target(SwiftTargetFixture {
            package: "App",
            external_package: None,
            kind: "macos_command_line_application",
            name: "app",
            srcs: &["main.swift"],
            deps: &[],
        });
        app.external_deps.push(fabrik_frontend::ExternalDependency {
            graph: "swiftpm".to_string(),
            spec: serde_json::json!({
                "package": "swift-argument-parser",
                "product": "ArgumentParser",
            }),
        });
        let built = build_plan(&[external_dep, app], "App/app", tmp.path()).unwrap();

        assert_eq!(
            built
                .nodes
                .iter()
                .map(|node| node.label.as_str())
                .collect::<Vec<_>>(),
            vec![
                "external:swiftpm/ArgumentParser#compile",
                "external:swiftpm/ArgumentParser#archive",
                "App/app",
            ]
        );
        assert_eq!(
            dependency_labels(&built, built.root_index),
            vec!["external:swiftpm/ArgumentParser#archive"]
        );
    }
}
