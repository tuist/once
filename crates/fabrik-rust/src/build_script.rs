//! `cargo_build_script` support: compile a `build.rs`, run it under
//! the cargo build-script env, and capture its stdout (the
//! `cargo::rustc-cfg=`, `cargo::rustc-env=`, `cargo::rustc-link-lib=`,
//! and `cargo::rustc-link-search=` lines) into a deterministic text
//! artifact at `.fabrik/out/<package>/<name>_build_script.out`.
//!
//! What this primitive *does*: compile + run the script in one cached
//! action, with the captured stdout as a declared output that flows
//! through the CAS like any other artifact.
//!
//! Dependents consume the captured directives by reading the restored
//! output file at execution time. This keeps the planner single-pass
//! while still making the build script a visible cacheable graph node.

use std::collections::BTreeMap;
use std::path::Path;

use fabrik_cas::Digest;
use fabrik_core::{Action, PlanNode, ResourceRequest, WorkspacePath};
use fabrik_frontend::Target;

use crate::artifact::{build_script_outputs_path, out_dir};
use crate::compile::CompileError;

/// Suffix of the captured build-script stdout.
pub const BUILD_SCRIPT_OUTPUTS_FILENAME: &str = "build_script.out";

pub fn output_path(package: &str, name: &str) -> String {
    build_script_outputs_path(&out_dir(package, name), name)
}

pub fn compile_build_script(
    target: &Target,
    workspace_root: &Path,
) -> Result<PlanNode, CompileError> {
    if target.srcs.is_empty() {
        return Err(CompileError::NoSources { label: target.id() });
    }
    let build_rs = target
        .srcs
        .iter()
        .find(|s| s.ends_with("build.rs"))
        .cloned()
        .ok_or_else(|| CompileError::CrateRootMissing {
            label: target.id(),
            root: "build.rs".into(),
        })?;
    let build_rs_ws = if target.package.is_empty() {
        build_rs
    } else {
        format!("{}/{}", target.package, build_rs)
    };

    let crate_dir = target.attrs.get("crate_dir").cloned().unwrap_or_else(|| {
        if target.package.is_empty() {
            ".".to_string()
        } else {
            target.package.clone()
        }
    });

    let out_dir = out_dir(&target.package, &target.name);
    let script_bin = format!("{out_dir}/{}_build_script_bin", target.name);
    let cargo_out = format!("{out_dir}/{}_cargo_out", target.name);
    let stdout_capture = build_script_outputs_path(&out_dir, &target.name);

    // Single-action compile + run + capture. The shell uses `set -eu`
    // so a failed rustc or build-script run aborts the whole action;
    // a partial flags file would mislead downstream consumers worse
    // than no file at all.
    let inline = format!(
        r#"set -eu
mkdir -p "{out_dir}" "{cargo_out}"
rustc --edition=2021 \
  --crate-name={name}_build_script \
  --crate-type=bin \
  -o "{script_bin}" \
  "{build_rs_ws}"
HOST_TRIPLE="$(rustc -vV | awk '/^host:/ {{print $2}}')"
CARGO_MANIFEST_DIR="{crate_dir}" \
OUT_DIR="{cargo_out}" \
PROFILE="release" \
OPT_LEVEL="3" \
HOST="$HOST_TRIPLE" \
TARGET="$HOST_TRIPLE" \
"./{script_bin}" > "{stdout_capture}"
"#,
        name = target.name,
        out_dir = out_dir,
        cargo_out = cargo_out,
        script_bin = script_bin,
        build_rs_ws = build_rs_ws,
        crate_dir = crate_dir,
        stdout_capture = stdout_capture,
    );

    let argv = vec!["/bin/sh".into(), "-c".into(), inline];

    let input_digest = build_input_digest(target, workspace_root)?;
    let outputs = vec![
        WorkspacePath::try_from(stdout_capture.as_str()).map_err(|source| {
            CompileError::InvalidPath {
                label: target.id(),
                path: stdout_capture.clone(),
                source,
            }
        })?,
    ];

    let action = Action::RunCommand {
        argv,
        env: tool_env(),
        cwd: None,
        input_digest: Some(input_digest),
        outputs,
        resources: ResourceRequest::default(),
        timeout_ms: Some(300_000),
    };

    Ok(PlanNode {
        label: target.id(),
        action,
        deps: Vec::new(),
    })
}

/// Parse a captured build-script stdout file into structured
/// directives. Exposed so a future planner pass (or a `fabrik
/// build-script-outputs` query verb) can consume the artifact without
/// re-parsing in every dependent.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BuildScriptOutputs {
    pub rustc_cfg: Vec<String>,
    pub rustc_env: Vec<String>,
    pub rustc_link_lib: Vec<String>,
    pub rustc_link_search: Vec<String>,
    /// Lines that didn't start with a known `cargo::` directive,
    /// preserved so consumers can surface them in diagnostics.
    pub other: Vec<String>,
}

impl BuildScriptOutputs {
    pub fn parse(stdout: &str) -> Self {
        let mut out = Self::default();
        for line in stdout.lines() {
            let Some(rest) = line
                .strip_prefix("cargo::")
                .or_else(|| line.strip_prefix("cargo:"))
            else {
                if !line.is_empty() {
                    out.other.push(line.to_string());
                }
                continue;
            };
            if let Some(v) = rest.strip_prefix("rustc-cfg=") {
                out.rustc_cfg.push(v.to_string());
            } else if let Some(v) = rest.strip_prefix("rustc-env=") {
                out.rustc_env.push(v.to_string());
            } else if let Some(v) = rest.strip_prefix("rustc-link-lib=") {
                out.rustc_link_lib.push(v.to_string());
            } else if let Some(v) = rest.strip_prefix("rustc-link-search=") {
                out.rustc_link_search.push(v.to_string());
            } else {
                out.other.push(line.to_string());
            }
        }
        out
    }
}

fn build_input_digest(target: &Target, workspace_root: &Path) -> Result<Digest, CompileError> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"fabrik.cargo_build_script.input.v1\0");
    let mut srcs: Vec<&String> = target.srcs.iter().collect();
    srcs.sort();
    for src in srcs {
        let ws_rel = if target.package.is_empty() {
            src.clone()
        } else {
            format!("{}/{}", target.package, src)
        };
        let abs = workspace_root.join(&ws_rel);
        let bytes = std::fs::read(&abs).map_err(|source| CompileError::ReadSource {
            label: target.id(),
            path: ws_rel.clone(),
            source,
        })?;
        let digest = Digest::of_bytes(&bytes);
        buf.extend_from_slice(ws_rel.as_bytes());
        buf.push(0);
        buf.extend_from_slice(digest.as_bytes());
        buf.push(0);
    }
    let toolchain = std::env::var("RUSTUP_TOOLCHAIN").unwrap_or_else(|_| "system-rustc".into());
    buf.extend_from_slice(b"toolchain:");
    buf.extend_from_slice(toolchain.as_bytes());
    buf.push(0);
    Ok(Digest::of_bytes(&buf))
}

fn tool_env() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for key in [
        "PATH",
        "HOME",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "RUSTUP_TOOLCHAIN",
    ] {
        if let Ok(value) = std::env::var(key) {
            env.insert(key.into(), value);
        }
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cargo_directives() {
        let stdout = "\
cargo::rustc-cfg=has_feature
cargo::rustc-env=FOO=bar
cargo::rustc-link-lib=z
cargo::rustc-link-search=/usr/lib
some other line
cargo::warning=ignored unknown directive
";
        let parsed = BuildScriptOutputs::parse(stdout);
        assert_eq!(parsed.rustc_cfg, vec!["has_feature".to_string()]);
        assert_eq!(parsed.rustc_env, vec!["FOO=bar".to_string()]);
        assert_eq!(parsed.rustc_link_lib, vec!["z".to_string()]);
        assert_eq!(parsed.rustc_link_search, vec!["/usr/lib".to_string()]);
        assert_eq!(parsed.other.len(), 2);
    }

    #[test]
    fn parses_empty_stdout() {
        let parsed = BuildScriptOutputs::parse("");
        assert!(parsed.rustc_cfg.is_empty());
        assert!(parsed.other.is_empty());
    }

    #[test]
    fn missing_build_rs_in_srcs_is_an_error() {
        let target = Target {
            package: "pkg".into(),
            kind: "cargo_build_script".into(),
            name: "build".into(),
            srcs: vec!["src/lib.rs".into()],
            deps: vec![],
            attrs: BTreeMap::new(),
        };
        let err = compile_build_script(&target, Path::new("/tmp")).unwrap_err();
        assert!(matches!(err, CompileError::CrateRootMissing { .. }));
    }
}
