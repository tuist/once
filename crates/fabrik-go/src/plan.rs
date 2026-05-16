use std::path::Path;

use fabrik_core::{BuiltPlan, NodeInfo, Plan};
use fabrik_frontend::Target;

use crate::compile::{compile_binary, CompileError};

#[derive(Debug, thiserror::Error)]
pub enum PlanBuildError {
    #[error("no target matches `{0}`")]
    UnknownRoot(String),
    #[error("target {label} has unsupported kind `{kind}`")]
    UnsupportedKind { label: String, kind: String },
    #[error("go_binary target {label} does not support local Fabrik deps yet")]
    UnsupportedLocalDeps { label: String },
    #[error("external dep `{graph}` of target {label} must use a string module path, got {spec}")]
    InvalidExternalDepSpec {
        label: String,
        graph: String,
        spec: String,
    },
    #[error(transparent)]
    Compile(#[from] CompileError),
}

pub fn supports_kind(kind: &str) -> bool {
    kind == "go_binary"
}

pub fn build_plan(
    targets: &[Target],
    root_id: &str,
    workspace_root: &Path,
) -> Result<BuiltPlan, PlanBuildError> {
    let target = targets
        .iter()
        .find(|t| t.id() == root_id)
        .ok_or_else(|| PlanBuildError::UnknownRoot(root_id.to_string()))?;
    if !supports_kind(&target.kind) {
        return Err(PlanBuildError::UnsupportedKind {
            label: target.id(),
            kind: target.kind.clone(),
        });
    }
    if !target.deps.is_empty() {
        return Err(PlanBuildError::UnsupportedLocalDeps { label: target.id() });
    }
    validate_external_deps(target)?;

    let (node, output) = compile_binary(target, workspace_root)?;
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

fn validate_external_deps(target: &Target) -> Result<(), PlanBuildError> {
    for dep in &target.external_deps {
        if dep.spec.as_str().is_none() {
            return Err(PlanBuildError::InvalidExternalDepSpec {
                label: target.id(),
                graph: dep.graph.clone(),
                spec: dep.spec.to_string(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;

    fn go_target(name: &str, srcs: &[&str]) -> Target {
        Target {
            package: "cmd/app".to_string(),
            external_package: None,
            kind: "go_binary".to_string(),
            name: name.to_string(),
            srcs: srcs.iter().map(|src| (*src).to_string()).collect(),
            deps: Vec::new(),
            external_deps: Vec::new(),
            attrs: BTreeMap::new(),
        }
    }

    #[test]
    fn rejects_local_fabrik_deps() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut target = go_target("app", &[]);
        target.deps.push("lib/lib".to_string());

        let err = build_plan(&[target], "cmd/app/app", tmp.path()).unwrap_err();
        assert!(matches!(
            err,
            PlanBuildError::UnsupportedLocalDeps { label } if label == "cmd/app/app"
        ));
    }

    #[test]
    fn rejects_non_string_external_deps() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut target = go_target("app", &[]);
        target
            .external_deps
            .push(fabrik_frontend::ExternalDependency {
                graph: "go".to_string(),
                spec: json!({ "module": "github.com/acme/lib" }),
            });

        let err = build_plan(&[target], "cmd/app/app", tmp.path()).unwrap_err();
        assert!(matches!(
            err,
            PlanBuildError::InvalidExternalDepSpec { label, graph, spec }
                if label == "cmd/app/app"
                    && graph == "go"
                    && spec == r#"{"module":"github.com/acme/lib"}"#
        ));
    }
}
