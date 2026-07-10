//! Turns a graph target and capability into a generic cacheable [`Action`].
//!
//! Target kinds with Starlark `impl` functions declare their own actions and
//! outputs. This module only covers legacy capability-only target kinds by
//! producing a small generic marker directory through the same action-cache
//! substrate.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use once_core::{Action, OutputSymlinkMode, ResourceRequest, SandboxMode, WorkspacePath};
use once_frontend::GraphTarget;
use serde::Serialize;

#[derive(Debug, Serialize)]
struct GraphActionManifest<'a> {
    target: &'a str,
    kind: &'a str,
    capability: &'a str,
    deps: &'a [String],
    srcs: &'a [String],
    outputs: Vec<&'a str>,
}

/// Build the cacheable action that produces a capability's outputs.
pub(super) fn action_for(
    target: &GraphTarget,
    capability: &str,
    outputs: &[WorkspacePath],
) -> Result<Action> {
    Ok(Action::RunCommand {
        argv: vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            action_script(target, capability, outputs)?,
        ],
        env: BTreeMap::new(),
        cwd: None,
        input_digest: None,
        inputs: source_inputs(target)?,
        outputs: outputs.to_vec(),
        stdout_path: None,
        stderr_path: None,
        output_symlink_mode: OutputSymlinkMode::default(),
        resources: ResourceRequest::default(),
        sandbox: SandboxMode::default(),
        timeout_ms: None,
        remote: None,
    })
}

fn source_inputs(target: &GraphTarget) -> Result<Vec<WorkspacePath>> {
    let mut inputs = target
        .srcs
        .iter()
        .map(|src| {
            let path = if target.label.package.is_empty() {
                src.clone()
            } else {
                format!("{}/{}", target.label.package, src)
            };
            WorkspacePath::try_from(path.as_str())
                .with_context(|| format!("invalid graph source path `{path}`"))
        })
        .collect::<Result<Vec<_>>>()?;
    inputs.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    inputs.dedup_by(|a, b| a.as_str() == b.as_str());
    Ok(inputs)
}

/// Workspace-relative marker directory for a generic capability fallback.
pub(super) fn output_paths(target: &GraphTarget, capability: &str) -> Result<Vec<WorkspacePath>> {
    let path = match capability {
        "build" => build_root(target),
        "run" => format!("{}/run", build_root(target)),
        "test" => format!("{}/test", build_root(target)),
        other => anyhow::bail!("unsupported graph capability `{other}`"),
    };
    Ok(vec![WorkspacePath::try_from(path.as_str()).with_context(
        || format!("invalid graph output path `{path}`"),
    )?])
}

fn action_script(
    target: &GraphTarget,
    capability: &str,
    outputs: &[WorkspacePath],
) -> Result<String> {
    let root = output_root(target, capability)?;
    let manifest = GraphActionManifest {
        target: &target.label.id,
        kind: &target.kind,
        capability,
        deps: &target.deps,
        srcs: &target.srcs,
        outputs: outputs.iter().map(WorkspacePath::as_str).collect(),
    };
    let manifest_json = serde_json::to_string_pretty(&manifest)?;
    let output_paths = outputs
        .iter()
        .map(|output| shell_quote(output.as_str()))
        .collect::<Vec<_>>()
        .join(" ");
    let manifest_path = shell_quote(&format!("{root}/manifest.json"));
    let write_manifest = write_manifest_cmd(&manifest_path, &manifest_json);
    let record_path = shell_quote(&format!("{root}/{capability}.json"));
    let record_json = shell_quote(
        &serde_json::json!({
            "target": target.label.id,
            "kind": target.kind,
            "capability": capability,
            "status": "completed",
        })
        .to_string(),
    );
    let write_record = format!("printf '%s\\n' {record_json} > {record_path}");
    let prepare_outputs = prepare_outputs_script(&output_paths);
    Ok(format!(
        r"set -eu
{prepare_outputs}
{write_manifest}
{write_record}
",
    ))
}

fn output_root(target: &GraphTarget, capability: &str) -> Result<String> {
    match capability {
        "build" => Ok(build_root(target)),
        "run" => Ok(format!("{}/run", build_root(target))),
        "test" => Ok(format!("{}/test", build_root(target))),
        other => anyhow::bail!("unsupported graph capability `{other}`"),
    }
}

fn prepare_outputs_script(output_paths: &str) -> String {
    format!(
        r#"for p in {output_paths}; do
  mkdir -p "$p"
done"#
    )
}

fn build_root(target: &GraphTarget) -> String {
    format!(".once/out/{}", target.label.id)
}

/// Emit the command that writes the artifact manifest.
///
/// The manifest is single-quoted with the same escaping every other dynamic
/// value uses, rather than embedded in a heredoc. This keeps all generated
/// content on the quoted side of the shell: no manifest value can terminate
/// the script body or be interpreted by the shell. `manifest_path` is already
/// shell quoted by the caller.
fn write_manifest_cmd(manifest_path: &str, manifest_json: &str) -> String {
    format!(
        "printf '%s\\n' {} > {manifest_path}",
        shell_quote(manifest_json)
    )
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    // POSIX single-quoted strings treat every byte literally except `'`,
    // which is represented by closing, emitting an escaped quote, and reopening.
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_frontend::{Capability, TargetLabel};

    fn graph_target(kind: &str, name: &str) -> GraphTarget {
        GraphTarget {
            label: TargetLabel {
                package: "apps/ios".to_string(),
                name: name.to_string(),
                id: format!("apps/ios/{name}"),
            },
            kind: kind.to_string(),
            deps: Vec::new(),
            srcs: Vec::new(),
            attrs: BTreeMap::new(),
            capabilities: vec![Capability {
                name: "build".to_string(),
                output_groups: Vec::new(),
                requires_outputs: Vec::new(),
            }],
            providers: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("App'; echo pwn"), "'App'\"'\"'; echo pwn'");
    }

    #[test]
    fn shell_quote_handles_empty_string() {
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn generic_script_uses_shell_quoted_dynamic_text() {
        let target = graph_target("custom_runner", "LaunchApp");

        let script = action_script(&target, "run", &output_paths(&target, "run").unwrap()).unwrap();

        assert!(!script.contains("mkdir -p \".once/out/apps/ios/LaunchApp/run\""));
        assert!(script.contains("mkdir -p \"$p\""));
        assert!(script.contains("> '.once/out/apps/ios/LaunchApp/run/manifest.json'"));
        assert!(script.contains("> '.once/out/apps/ios/LaunchApp/run/run.json'"));
        assert!(script.contains(r#""kind":"custom_runner""#));
    }

    #[test]
    fn prepare_outputs_creates_marker_dirs() {
        let script = prepare_outputs_script("'.once/out/x' '.once/out/x/run'");
        assert!(script.contains(r#"mkdir -p "$p""#));
        assert!(!script.contains("dirname"));
    }

    fn output_strings(target: &GraphTarget, capability: &str) -> Vec<String> {
        output_paths(target, capability)
            .unwrap()
            .into_iter()
            .map(|path| path.as_str().to_string())
            .collect()
    }

    #[test]
    fn output_paths_are_generic_capability_marker_dirs() {
        let target = graph_target("custom_runner", "LaunchApp");
        assert_eq!(
            output_strings(&target, "build"),
            vec![".once/out/apps/ios/LaunchApp".to_string()]
        );
        assert_eq!(
            output_strings(&target, "run"),
            vec![".once/out/apps/ios/LaunchApp/run".to_string()]
        );
        assert_eq!(
            output_strings(&target, "test"),
            vec![".once/out/apps/ios/LaunchApp/test".to_string()]
        );
    }

    #[test]
    fn output_paths_reject_unknown_capability() {
        let err = output_paths(&graph_target("custom_runner", "LaunchApp"), "lint")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unsupported graph capability `lint`"));
    }

    #[test]
    fn output_paths_reject_target_ids_that_escape_workspace() {
        let mut target = graph_target("custom_runner", "LaunchApp");
        target.label.id = "../LaunchApp".to_string();
        let err = output_paths(&target, "build").unwrap_err().to_string();
        assert!(err.contains("invalid graph output path"));
    }

    #[test]
    fn action_for_wraps_script_in_sh_invocation() {
        let target = graph_target("custom_runner", "LaunchApp");
        let outputs = output_paths(&target, "build").unwrap();
        let Action::RunCommand {
            argv,
            outputs: action_outputs,
            input_digest,
            ..
        } = action_for(&target, "build", &outputs).unwrap()
        else {
            panic!("graph fallback action should produce a command action");
        };
        assert_eq!(argv[0], "/bin/sh");
        assert_eq!(argv[1], "-c");
        assert!(argv[2].contains("manifest.json"));
        assert_eq!(action_outputs, outputs);
        assert!(input_digest.is_none());
    }

    #[test]
    fn manifest_is_single_quoted_not_heredoc() {
        let target = graph_target("custom_runner", "LaunchApp");
        let outputs = output_paths(&target, "build").unwrap();
        let script = action_script(&target, "build", &outputs).unwrap();

        // The manifest is written through the same single-quote escaping as
        // every other value, so no heredoc terminator can appear in the body.
        assert!(!script.contains("ONCE_MANIFEST"));
        assert!(!script.contains("<<"));
        assert!(script.contains("printf '%s\\n' '"));
        assert!(script.contains("> '.once/out/apps/ios/LaunchApp/manifest.json'"));
    }

    #[test]
    fn write_manifest_cmd_single_quotes_dynamic_content() {
        // A manifest value that would close a quoted heredoc, plus a single
        // quote, must stay inert inside the generated command.
        let cmd = write_manifest_cmd("'out/manifest.json'", "{\n\"k\": \"ONCE_MANIFEST'x\"\n}");
        assert!(cmd.starts_with("printf '%s\\n' '"));
        assert!(cmd.ends_with("> 'out/manifest.json'"));
        // The embedded single quote is escaped via the close/escape/reopen form.
        assert!(cmd.contains("'\"'\"'"));
    }

    #[test]
    fn action_script_rejects_unknown_capability() {
        let target = graph_target("custom_runner", "LaunchApp");
        let err = action_script(&target, "lint", &[]).unwrap_err().to_string();
        assert!(err.contains("unsupported graph capability `lint`"));
    }
}
