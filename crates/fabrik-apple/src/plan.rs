use std::path::Path;

use fabrik_core::Plan;
use fabrik_frontend::Target;

use crate::compile::{compile_ios_app, AppleError};

#[derive(Debug, Clone)]
pub struct BuiltPlan {
    pub plan: Plan,
    pub root_index: usize,
    pub root_label: String,
    pub output: String,
    pub nodes: Vec<NodeInfo>,
}

#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub label: String,
    pub kind: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PlanBuildError {
    #[error("no target matches `{0}`")]
    UnknownRoot(String),
    #[error("apple_ios_app target {label} does not support deps yet")]
    UnsupportedDeps { label: String },
    #[error(transparent)]
    Apple(#[from] AppleError),
}

pub fn build_plan(
    targets: &[Target],
    root_label: &str,
    workspace_root: &Path,
) -> Result<BuiltPlan, PlanBuildError> {
    let target = targets
        .iter()
        .find(|t| t.label() == root_label)
        .ok_or_else(|| PlanBuildError::UnknownRoot(root_label.to_string()))?;
    if !target.deps.is_empty() {
        return Err(PlanBuildError::UnsupportedDeps {
            label: target.label(),
        });
    }

    let node = compile_ios_app(target, workspace_root)?;
    let output = crate::app_bundle_path(&target.package, &target.name);
    let mut plan = Plan::new();
    let root_index = plan.push(node);
    Ok(BuiltPlan {
        plan,
        root_index,
        root_label: root_label.to_string(),
        output,
        nodes: vec![NodeInfo {
            label: target.label(),
            kind: target.kind.clone(),
        }],
    })
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
        attrs.insert("bundle_id".to_string(), "dev.fabrik.demo".to_string());
        let target = Target {
            package: "App".to_string(),
            kind: "apple_ios_app".to_string(),
            name: "Demo".to_string(),
            srcs: vec!["App.swift".to_string()],
            deps: Vec::new(),
            attrs,
        };
        let built = build_plan(&[target], "//App:Demo", tmp.path()).unwrap();
        assert_eq!(built.plan.nodes.len(), 1);
        assert_eq!(built.output, ".fabrik/out/App/Demo.app");
    }
}
