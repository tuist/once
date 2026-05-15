//! Go rule planning.
//!
//! Go module resolution stays native: Fabrik models the dependency edge
//! for graph visibility and cache-key metadata, then invokes `go build`
//! from the target package so `go.mod`, `go.sum`, and `replace` entries
//! keep their normal meaning.

use std::collections::BTreeMap;
use std::path::Path;

use fabrik_core::{
    workspace_tool_env, Action, BuiltPlan, InputDigestBuilder, NodeInfo, Plan, PlanNode,
    WorkspacePath,
};
use fabrik_frontend::Target;

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

#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    #[error("target {label}: invalid path `{path}`: {source}")]
    InvalidPath {
        label: String,
        path: String,
        #[source]
        source: fabrik_core::WorkspacePathError,
    },
    #[error("failed to read source `{path}` for target {label}: {source}")]
    ReadSource {
        label: String,
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to resolve toolchain for target {label}: {source}")]
    Toolchain {
        label: String,
        #[source]
        source: fabrik_core::ToolEnvError,
    },
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

fn compile_binary(
    target: &Target,
    workspace_root: &Path,
) -> Result<(PlanNode, String), CompileError> {
    let output = executable_path(&target.package, &target.name);
    let build_package = target
        .attrs
        .get("package")
        .cloned()
        .unwrap_or_else(|| ".".to_string());
    let rel_output = path_from_package_to_workspace_path(&target.package, &output);
    let rel_output_parent = parent_dir(&rel_output);
    let script = format!(
        "set -eu\nmkdir -p {output_parent}\ngo build -o {rel_output} {build_package}\n",
        output_parent = sh_quote(&rel_output_parent),
        rel_output = sh_quote(&rel_output),
        build_package = sh_quote(&build_package),
    );
    let input_digest = build_input_digest(target, workspace_root)?;
    let outputs = vec![workspace_path(target, &output)?];
    let env = tool_env(workspace_root).map_err(|source| CompileError::Toolchain {
        label: target.id(),
        source,
    })?;
    let cwd = if target.package.is_empty() {
        None
    } else {
        Some(workspace_path(target, &target.package)?)
    };
    let action = Action::RunCommand {
        argv: vec!["/bin/sh".into(), "-c".into(), script],
        env,
        cwd,
        input_digest: Some(input_digest),
        outputs,
        resources: fabrik_core::ResourceRequest::new(2, 0),
        timeout_ms: Some(300_000),
    };
    Ok((
        PlanNode {
            label: target.id(),
            action,
            deps: Vec::new(),
        },
        output,
    ))
}

fn build_input_digest(
    target: &Target,
    workspace_root: &Path,
) -> Result<fabrik_cas::Digest, CompileError> {
    let mut builder = InputDigestBuilder::new(b"fabrik.go.input.v1\0");
    let mut srcs: Vec<&String> = target.srcs.iter().collect();
    srcs.sort();
    for src in srcs {
        let ws_rel =
            WorkspacePath::from_package_relative(&target.package, src).map_err(|source| {
                CompileError::InvalidPath {
                    label: target.id(),
                    path: src.clone(),
                    source,
                }
            })?;
        builder
            .push_source(workspace_root, ws_rel.as_str())
            .map_err(|source| CompileError::ReadSource {
                label: target.id(),
                path: ws_rel.as_str().to_string(),
                source,
            })?;
    }

    if let Some(package) = target.attrs.get("package") {
        builder.push_bytes(format!("package:{package}").as_bytes());
    }

    let mut deps = target
        .external_deps
        .iter()
        .map(|dep| {
            (
                dep.graph.as_str(),
                serde_json::to_string(&dep.spec).expect("external dep spec serializes"),
            )
        })
        .collect::<Vec<_>>();
    deps.sort();
    for (graph, spec) in deps {
        builder.push_bytes(format!("external:{graph}:{spec}").as_bytes());
    }

    Ok(builder.finish())
}

fn executable_path(package: &str, name: &str) -> String {
    if package.is_empty() {
        format!(".fabrik/out/{name}")
    } else {
        format!(".fabrik/out/{package}/{name}")
    }
}

fn parent_dir(path: &str) -> String {
    path.rsplit_once('/')
        .map_or_else(String::new, |(parent, _)| parent.to_string())
}

fn path_from_package_to_workspace_path(package: &str, path: &str) -> String {
    if package.is_empty() {
        return path.to_string();
    }
    let ups = package
        .split('/')
        .filter(|part| !part.is_empty())
        .map(|_| "..")
        .collect::<Vec<_>>();
    ups.into_iter().chain([path]).collect::<Vec<_>>().join("/")
}

fn workspace_path(target: &Target, path: &str) -> Result<WorkspacePath, CompileError> {
    WorkspacePath::try_from(path).map_err(|source| CompileError::InvalidPath {
        label: target.id(),
        path: path.to_string(),
        source,
    })
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn tool_env(workspace_root: &Path) -> Result<BTreeMap<String, String>, fabrik_core::ToolEnvError> {
    workspace_tool_env(
        workspace_root,
        &["go"],
        &[
            "CGO_ENABLED",
            "GOARCH",
            "GOCACHE",
            "GOENV",
            "GOFLAGS",
            "GOMODCACHE",
            "GOOS",
            "GOPATH",
            "GOROOT",
            "GOTOOLCHAIN",
            "TMPDIR",
            "XDG_CACHE_HOME",
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn go_target(name: &str, srcs: &[&str]) -> Target {
        Target {
            package: "cmd/app".to_string(),
            kind: "go_binary".to_string(),
            name: name.to_string(),
            srcs: srcs.iter().map(|src| (*src).to_string()).collect(),
            deps: Vec::new(),
            external_deps: Vec::new(),
            attrs: BTreeMap::new(),
        }
    }

    #[test]
    fn package_relative_output_walks_back_to_workspace_root() {
        assert_eq!(
            path_from_package_to_workspace_path("cmd/app", ".fabrik/out/cmd/app/app"),
            "../../.fabrik/out/cmd/app/app"
        );
        assert_eq!(
            path_from_package_to_workspace_path("", ".fabrik/out/app"),
            ".fabrik/out/app"
        );
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

    #[test]
    fn rejects_source_paths_that_escape_the_package() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = go_target("app", &["../go.mod"]);

        let err = build_plan(&[target], "cmd/app/app", tmp.path()).unwrap_err();
        assert!(matches!(
            err,
            PlanBuildError::Compile(CompileError::InvalidPath { label, path, .. })
                if label == "cmd/app/app" && path == "../go.mod"
        ));
    }
}
