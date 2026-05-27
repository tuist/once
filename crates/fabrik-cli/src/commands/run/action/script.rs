//! Lower script targets into concrete command actions.
//!
//! File-backed scripts carry their execution contract in FABRIK
//! headers inside the script file. Manifest-backed scripts carry the
//! same contract in the target attrs. Both lower to `RunCommand`.

use std::collections::BTreeMap;
use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use fabrik_cas::Digest;
use fabrik_core::{
    tool_env, workspace_tool, workspace_tool_env, Action, RemoteExecution, ResourceRequest,
    WorkspacePath,
};

use super::{input_digest, parse_attr, ActionPlan};
use crate::commands::util::relative_path;

pub(super) fn script_action(
    workspace: &std::path::Path,
    target: &fabrik_frontend::Target,
) -> Result<ActionPlan> {
    let cache = target
        .attrs
        .get("cache")
        .map_or(Ok(true), |raw| raw.parse::<bool>())
        .with_context(|| format!("parsing cache setting for script {}", target.id()))?;
    let input_digest = if cache {
        input_digest(workspace, target)?
    } else {
        Some(uncached_script_digest(target))
    };

    let timeout_ms = parse_attr::<u64>(target, "timeout_ms")?;
    let outputs = outputs(target)?;
    let cwd = cwd(target)?;
    let resources = resources(target)?;
    let remote = remote(target);

    if target.attrs.contains_key("script_path") {
        return file_script_action(
            workspace,
            target,
            input_digest,
            outputs,
            cwd,
            resources,
            timeout_ms,
            remote,
        );
    }

    manifest_script_action(
        target,
        input_digest,
        outputs,
        cwd,
        resources,
        timeout_ms,
        remote,
    )
}

#[allow(clippy::too_many_arguments)]
fn file_script_action(
    workspace: &std::path::Path,
    target: &fabrik_frontend::Target,
    input_digest: Option<Digest>,
    outputs: Vec<WorkspacePath>,
    cwd: Option<WorkspacePath>,
    resources: ResourceRequest,
    timeout_ms: Option<u64>,
    remote: Option<RemoteExecution>,
) -> Result<ActionPlan> {
    let runtime = target
        .attrs
        .get("script_runtime")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("script {} has no runtime", target.id()))?;
    let runtime_args = runtime_args(target)?;
    let script_path = target
        .attrs
        .get("script_path")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("script {} has no script_path", target.id()))?;

    let program = resolve_host_runtime(workspace, &runtime)?;
    let mut argv = vec![program];
    argv.extend(runtime_args);
    argv.push(host_script_path(&script_path, cwd.as_ref())?);

    Ok(ActionPlan {
        action: Action::RunCommand {
            argv,
            env: host_env(workspace, target, &runtime)?,
            cwd,
            input_digest,
            outputs,
            resources,
            timeout_ms,
            remote,
        },
        output: String::new(),
        output_dir: None,
    })
}

fn manifest_script_action(
    target: &fabrik_frontend::Target,
    input_digest: Option<Digest>,
    outputs: Vec<WorkspacePath>,
    cwd: Option<WorkspacePath>,
    resources: ResourceRequest,
    timeout_ms: Option<u64>,
    remote: Option<RemoteExecution>,
) -> Result<ActionPlan> {
    let argv = manifest_script_argv(target)?;

    Ok(ActionPlan {
        action: Action::RunCommand {
            argv,
            env: host_manifest_env(target)?,
            cwd,
            input_digest,
            outputs,
            resources,
            timeout_ms,
            remote,
        },
        output: String::new(),
        output_dir: None,
    })
}

fn remote(target: &fabrik_frontend::Target) -> Option<RemoteExecution> {
    target
        .attrs
        .get("remote_provider")
        .map(|provider| RemoteExecution {
            provider: provider.to_string(),
        })
}

fn runtime_args(target: &fabrik_frontend::Target) -> Result<Vec<String>> {
    match target.attrs.get("script_runtime_args_json") {
        Some(raw) => serde_json::from_str(raw)
            .with_context(|| format!("parsing runtime args for script {}", target.id())),
        None => Ok(Vec::new()),
    }
}

fn tracked_env_names(target: &fabrik_frontend::Target) -> Result<Vec<String>> {
    match target.attrs.get("script_env_json") {
        Some(raw) => serde_json::from_str(raw)
            .with_context(|| format!("parsing env for script {}", target.id())),
        None => Ok(Vec::new()),
    }
}

fn tracked_env(target: &fabrik_frontend::Target) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for name in tracked_env_names(target)? {
        if let Ok(value) = env::var(&name) {
            out.insert(name, value);
        }
    }
    Ok(out)
}

fn host_manifest_env(target: &fabrik_frontend::Target) -> Result<BTreeMap<String, String>> {
    let env_names = tracked_env_names(target)?;
    let env_keys = env_names.iter().map(String::as_str).collect::<Vec<_>>();
    Ok(tool_env(&env_keys))
}

fn host_env(
    workspace: &std::path::Path,
    target: &fabrik_frontend::Target,
    runtime: &str,
) -> Result<BTreeMap<String, String>> {
    let env_names = tracked_env_names(target)?;
    let env_keys = env_names.iter().map(String::as_str).collect::<Vec<_>>();
    let mut out = if runtime.contains('/') {
        tool_env(&env_keys)
    } else {
        workspace_tool_env(workspace, &[runtime], &env_keys)
            .with_context(|| format!("building tool environment for script {}", target.id()))?
    };
    for (key, value) in tracked_env(target)? {
        out.insert(key, value);
    }
    Ok(out)
}

fn resolve_host_runtime(workspace: &std::path::Path, runtime: &str) -> Result<String> {
    if runtime.contains('/') {
        return Ok(runtime.to_string());
    }
    workspace_tool(workspace, runtime)
        .with_context(|| format!("resolving script runtime `{runtime}`"))
}

fn cwd(target: &fabrik_frontend::Target) -> Result<Option<WorkspacePath>> {
    target
        .attrs
        .get("cwd")
        .map(|raw| {
            WorkspacePath::try_from(raw.as_str())
                .with_context(|| format!("invalid cwd for script {}", target.id()))
        })
        .transpose()
}

fn outputs(target: &fabrik_frontend::Target) -> Result<Vec<WorkspacePath>> {
    let Some(raw) = target.attrs.get("outputs_json") else {
        return Ok(Vec::new());
    };
    serde_json::from_str::<Vec<String>>(raw)
        .with_context(|| format!("parsing outputs for script {}", target.id()))?
        .iter()
        .map(|value| {
            WorkspacePath::try_from(value.as_str())
                .with_context(|| format!("invalid output `{value}` in {}", target.id()))
        })
        .collect()
}

fn resources(target: &fabrik_frontend::Target) -> Result<ResourceRequest> {
    let cpu_slots = parse_attr::<usize>(target, "cpu_slots")?.unwrap_or(1);
    let memory_bytes = parse_attr::<u64>(target, "memory_bytes")?.unwrap_or(0);
    Ok(ResourceRequest::new(cpu_slots, memory_bytes))
}

fn manifest_script_argv(target: &fabrik_frontend::Target) -> Result<Vec<String>> {
    let argv_json = target
        .attrs
        .get("script_argv_json")
        .ok_or_else(|| anyhow::anyhow!("script {} has no argv", target.id()))?;
    let argv: Vec<String> = serde_json::from_str(argv_json)
        .with_context(|| format!("parsing argv for script {}", target.id()))?;
    if argv.is_empty() {
        anyhow::bail!("script {} has empty argv", target.id());
    }
    Ok(argv)
}

fn host_script_path(script_path: &str, cwd: Option<&WorkspacePath>) -> Result<String> {
    let script = WorkspacePath::try_from(script_path)
        .with_context(|| format!("invalid script path `{script_path}`"))?;
    let Some(cwd) = cwd else {
        return Ok(script.as_str().to_string());
    };
    Ok(relative_path(cwd.as_str(), script.as_str()))
}

fn uncached_script_digest(target: &fabrik_frontend::Target) -> Digest {
    let nonce = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos(),
        Err(err) => err.duration().as_nanos(),
    };
    uncached_script_digest_with_nonce(target, nonce)
}

fn uncached_script_digest_with_nonce(target: &fabrik_frontend::Target, nonce: u128) -> Digest {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"fabrik.script.uncached.v1\0");
    buf.extend_from_slice(target.id().as_bytes());
    buf.push(0);
    buf.extend_from_slice(&nonce.to_le_bytes());
    Digest::of_bytes(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabrik_cas::Digest;
    use tempfile::TempDir;

    fn script_target(package: &str) -> fabrik_frontend::Target {
        fabrik_frontend::Target {
            package: package.to_string(),
            external_package: None,
            kind: "script".to_string(),
            name: "build".to_string(),
            srcs: vec!["scripts/build.sh".to_string(), "src/input.txt".to_string()],
            deps: Vec::new(),
            external_deps: Vec::new(),
            attrs: BTreeMap::new(),
        }
    }

    fn test_nonce(seed: &str) -> u128 {
        let digest = Digest::of_bytes(seed.as_bytes());
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest.as_bytes()[..16]);
        u128::from_le_bytes(bytes)
    }

    #[test]
    fn host_script_action_uses_cwd_relative_script_path() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("pkg/scripts")).unwrap();
        std::fs::create_dir_all(tmp.path().join("pkg/src")).unwrap();
        std::fs::write(tmp.path().join("pkg/scripts/build.sh"), "#!/bin/sh\n").unwrap();
        std::fs::write(tmp.path().join("pkg/src/input.txt"), "hello\n").unwrap();

        let mut target = script_target("pkg");
        target
            .attrs
            .insert("script_runtime".into(), "/bin/sh".into());
        target
            .attrs
            .insert("script_path".into(), "pkg/scripts/build.sh".into());
        target.attrs.insert("cwd".into(), "pkg/scripts".into());
        target
            .attrs
            .insert("outputs_json".into(), "[\"pkg/dist\"]".into());
        target.attrs.insert(
            "script_env_json".into(),
            "[\"FABRIK_TEST_HOST_SCRIPT_ENV\"]".into(),
        );
        std::env::set_var("FABRIK_TEST_HOST_SCRIPT_ENV", "present");

        let plan = script_action(tmp.path(), &target).unwrap();
        match plan.action {
            Action::RunCommand {
                argv,
                env,
                cwd,
                input_digest,
                ..
            } => {
                assert_eq!(argv, vec!["/bin/sh".to_string(), "build.sh".to_string()]);
                assert_eq!(cwd.unwrap().as_str(), "pkg/scripts");
                assert!(input_digest.is_some());
                assert_eq!(
                    env.get("FABRIK_TEST_HOST_SCRIPT_ENV").map(String::as_str),
                    Some("present")
                );
            }
        }
    }

    #[test]
    fn host_manifest_script_action_uses_declared_argv_env_and_resources() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("pkg")).unwrap();
        std::fs::write(tmp.path().join("pkg/input.txt"), "hello\n").unwrap();

        let mut target = fabrik_frontend::Target {
            package: "pkg".to_string(),
            external_package: None,
            kind: "script".to_string(),
            name: "build".to_string(),
            srcs: vec!["input.txt".to_string()],
            deps: Vec::new(),
            external_deps: Vec::new(),
            attrs: BTreeMap::new(),
        };
        target.attrs.insert(
            "script_argv_json".into(),
            r#"["/bin/sh","-c","printf \"$INLINE_FROM_HOST\""]"#.into(),
        );
        target
            .attrs
            .insert("script_env_json".into(), r#"["INLINE_FROM_HOST"]"#.into());
        target.attrs.insert("cwd".into(), "pkg".into());
        std::env::set_var("INLINE_FROM_HOST", "present");

        let plan = script_action(tmp.path(), &target).unwrap();
        match plan.action {
            Action::RunCommand {
                argv,
                env,
                cwd,
                resources,
                input_digest,
                ..
            } => {
                assert_eq!(
                    argv,
                    vec![
                        "/bin/sh".to_string(),
                        "-c".to_string(),
                        "printf \"$INLINE_FROM_HOST\"".to_string()
                    ]
                );
                assert_eq!(cwd.unwrap().as_str(), "pkg");
                assert_eq!(
                    env.get("INLINE_FROM_HOST").map(String::as_str),
                    Some("present")
                );
                assert!(env.contains_key("PATH"));
                assert_eq!(resources, ResourceRequest::new(1, 0));
                assert!(input_digest.is_some());
            }
        }
    }

    #[test]
    fn uncached_script_digest_is_stable_for_the_same_nonce() {
        let target = script_target("pkg");
        let nonce = test_nonce("same");
        let digest_a = uncached_script_digest_with_nonce(&target, nonce);
        let digest_b = uncached_script_digest_with_nonce(&target, nonce);

        assert_eq!(digest_a, digest_b);
    }

    #[test]
    fn uncached_script_digest_changes_when_the_nonce_changes() {
        let target = script_target("pkg");
        let digest_a = uncached_script_digest_with_nonce(&target, test_nonce("first"));
        let digest_b = uncached_script_digest_with_nonce(&target, test_nonce("second"));

        assert_ne!(digest_a, digest_b);
    }
}
