use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use once_cas::{CacheProvider, Digest};
use once_core::{
    Action, InputDigestBuilder, OutputSymlinkMode, ResourceRequest, RunOpts, WorkspacePath,
};
use once_frontend::analysis::{AnalysisResult, DeclaredAction};
use once_frontend::GraphTarget;
use tokio::process::Command;

use super::BuildOutcome;

/// Materialise each declared action through the action cache, then
/// fold the analysis provider directly into the build outcome.
pub(super) async fn run_declared_actions(
    workspace: &Path,
    cache: &CacheProvider,
    target: &GraphTarget,
    analysis: AnalysisResult,
    dep_action_digests: &[(String, Digest)],
) -> Result<BuildOutcome> {
    let AnalysisResult {
        actions, provider, ..
    } = analysis;
    let mut action_digests = Vec::new();
    let mut last_cache_tag = "miss";
    let mut all_outputs = Vec::new();

    for (index, declared) in actions.into_iter().enumerate() {
        let identifier_for_error = declared
            .identifier
            .clone()
            .unwrap_or_else(|| "<anonymous>".to_string());
        all_outputs.extend(declared.outputs.iter().cloned());
        let cacheable = declared.cacheable;
        let mut input_action_digests = dep_action_digests.to_vec();
        input_action_digests.extend(
            action_digests
                .iter()
                .enumerate()
                .map(|(prior_index, digest)| (format!("same-target:{prior_index}"), *digest)),
        );
        let action =
            declared_to_action(workspace, declared, &input_action_digests).with_context(|| {
                format!(
                    "building action {index} for {} ({identifier_for_error})",
                    target.label.id,
                )
            })?;
        if cacheable {
            let outcome = once_core::run_with_cache(&action, workspace, cache, RunOpts::default())
                .await
                .with_context(|| {
                    format!(
                        "executing action {index} for {} ({identifier_for_error})",
                        target.label.id,
                    )
                })?;
            let exit_code = outcome.result.exit_code;
            if exit_code != 0 {
                anyhow::bail!(
                    "{identifier_for_error} ({index}) failed for {} with exit code {exit_code}",
                    target.label.id,
                );
            }
            action_digests.push(outcome.action);
            last_cache_tag = crate::commands::util::cache_tag(outcome.cache);
        } else {
            let action_digest = action.digest();
            let exit_code = run_uncached_action(&action, workspace)
                .await
                .with_context(|| {
                    format!(
                        "executing action {index} for {} ({identifier_for_error})",
                        target.label.id,
                    )
                })?;
            if exit_code != 0 {
                anyhow::bail!(
                    "{identifier_for_error} ({index}) failed for {} with exit code {exit_code}",
                    target.label.id,
                );
            }
            action_digests.push(action_digest);
            last_cache_tag = "bypass";
        }
    }

    Ok(BuildOutcome {
        provider,
        action_digest: compose_target_action_digest(&target.label.id, &action_digests),
        outputs: all_outputs,
        cache_tag: last_cache_tag,
    })
}

async fn run_uncached_action(action: &Action, workspace: &Path) -> Result<i32> {
    match action {
        Action::RunCommand {
            argv,
            env,
            cwd,
            timeout_ms,
            outputs,
            ..
        } => {
            let (program, rest) = argv
                .split_first()
                .ok_or_else(|| anyhow::anyhow!("declared action has empty argv"))?;
            let mut command = Command::new(program);
            command.args(rest);
            command.env_clear();
            for (key, value) in env {
                command.env(key, value);
            }
            command.stdin(Stdio::null());
            command.stdout(Stdio::null());
            command.stderr(Stdio::piped());
            command.current_dir(
                cwd.as_ref()
                    .map_or_else(|| workspace.to_path_buf(), |path| path.resolve(workspace)),
            );
            command.kill_on_drop(true);

            let output = match timeout_ms {
                Some(ms) => tokio::time::timeout(Duration::from_millis(*ms), command.output())
                    .await
                    .with_context(|| format!("declared action timed out after {ms}ms"))??,
                None => command.output().await?,
            };
            if !output.status.success() {
                let code = output.status.code().unwrap_or(-1);
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("declared action exited with code {code}: {stderr}");
            }
            verify_declared_outputs(outputs, workspace)?;
            Ok(output.status.code().unwrap_or(0))
        }
    }
}

fn verify_declared_outputs(outputs: &[WorkspacePath], workspace: &Path) -> Result<()> {
    for output in outputs {
        if !output.resolve(workspace).exists() {
            anyhow::bail!(
                "declared action completed without producing output `{}`",
                output.as_str()
            );
        }
    }
    Ok(())
}

fn compose_target_action_digest(target_id: &str, action_digests: &[Digest]) -> Digest {
    match action_digests {
        [] => Digest::of_bytes(format!("empty:{target_id}").as_bytes()),
        [digest] => *digest,
        _ => {
            let mut builder = InputDigestBuilder::new(b"once.target.actions.v1\0");
            builder.push_bytes(target_id.as_bytes());
            for (index, digest) in action_digests.iter().enumerate() {
                let key = format!("action:{index}");
                builder.push_keyed(key.as_bytes(), digest);
            }
            builder.finish()
        }
    }
}

fn declared_to_action(
    workspace: &Path,
    declared: DeclaredAction,
    dep_action_digests: &[(String, Digest)],
) -> Result<Action> {
    tracing::trace!(
        identifier = ?declared.identifier,
        argv = ?declared.argv,
        env = ?declared.env,
        outputs = ?declared.outputs,
        "declared graph action"
    );
    let input_digest = compose_input_digest(workspace, &declared, dep_action_digests)?;
    let outputs = declared
        .outputs
        .iter()
        .map(|path| {
            WorkspacePath::try_from(path.as_str())
                .with_context(|| format!("invalid declared output path `{path}`"))
        })
        .collect::<Result<Vec<_>>>()?;
    ensure_output_parent_dirs(workspace, &outputs)?;
    let DeclaredAction { argv, env, .. } = declared;
    Ok(Action::RunCommand {
        argv,
        env,
        cwd: None,
        input_digest: Some(input_digest),
        outputs,
        output_symlink_mode: OutputSymlinkMode::default(),
        resources: ResourceRequest::default(),
        timeout_ms: None,
        remote: None,
    })
}

fn ensure_output_parent_dirs(workspace: &Path, outputs: &[WorkspacePath]) -> Result<()> {
    for output in outputs {
        let absolute = output.resolve(workspace);
        if let Some(parent) = absolute.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("creating parent directory for output `{}`", output.as_str())
            })?;
        }
    }
    Ok(())
}

fn compose_input_digest(
    workspace: &Path,
    declared: &DeclaredAction,
    dep_action_digests: &[(String, Digest)],
) -> Result<Digest> {
    let mut builder = InputDigestBuilder::new(b"once.declared_action.input.v1\0");
    if let Some(identity) = &declared.toolchain_identity {
        builder.push_bytes(identity.as_bytes());
    }
    if let Some(identifier) = &declared.identifier {
        builder.push_bytes(identifier.as_bytes());
    }
    for arg in &declared.argv {
        builder.push_bytes(arg.as_bytes());
    }
    for (key, value) in &declared.env {
        builder.push_bytes(key.as_bytes());
        builder.push_bytes(value.as_bytes());
    }

    let mut sorted_inputs = declared
        .inputs
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    sorted_inputs.sort_unstable();
    sorted_inputs.dedup();
    for input in &sorted_inputs {
        builder
            .push_source(workspace, input)
            .with_context(|| format!("hashing declared input `{input}`"))?;
    }

    let mut dep_order = (0..dep_action_digests.len()).collect::<Vec<_>>();
    dep_order.sort_unstable_by(|&a, &b| dep_action_digests[a].0.cmp(&dep_action_digests[b].0));
    for index in dep_order {
        let (label, digest) = &dep_action_digests[index];
        let key = format!("dep:{label}");
        builder.push_keyed(key.as_bytes(), digest);
    }
    Ok(builder.finish())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn declared_action_uses_direct_argv_and_creates_output_parents() {
        let workspace = tempfile::tempdir().unwrap();
        let declared = DeclaredAction {
            argv: vec!["tool".to_string(), "--version".to_string()],
            inputs: Vec::new(),
            outputs: vec![
                ".once/out/x/A.out".to_string(),
                ".once/out/x/sub/B.meta".to_string(),
            ],
            env: BTreeMap::new(),
            cacheable: true,
            toolchain_identity: None,
            identifier: None,
        };

        let action = declared_to_action(workspace.path(), declared, &[]).unwrap();

        assert!(workspace.path().join(".once/out/x").is_dir());
        assert!(workspace.path().join(".once/out/x/sub").is_dir());
        let Action::RunCommand { argv, .. } = action;
        assert_eq!(argv, vec!["tool".to_string(), "--version".to_string()]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uncached_action_executes_each_time() {
        let workspace = tempfile::tempdir().unwrap();
        let action = Action::RunCommand {
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf x >> counter".to_string(),
            ],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            outputs: Vec::new(),
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            timeout_ms: None,
            remote: None,
        };

        run_uncached_action(&action, workspace.path())
            .await
            .unwrap();
        run_uncached_action(&action, workspace.path())
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(workspace.path().join("counter")).unwrap(),
            "xx"
        );
    }

    #[test]
    fn input_digest_changes_with_toolchain_identity() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("a.swift"), b"print(1)").unwrap();
        let declared = DeclaredAction {
            argv: vec!["swiftc".to_string()],
            inputs: vec!["a.swift".to_string()],
            outputs: vec![".once/out/A.a".to_string()],
            env: BTreeMap::new(),
            cacheable: true,
            toolchain_identity: Some("id-1".to_string()),
            identifier: None,
        };
        let one = compose_input_digest(workspace.path(), &declared, &[]).unwrap();
        let declared2 = DeclaredAction {
            toolchain_identity: Some("id-2".to_string()),
            ..declared.clone()
        };
        let two = compose_input_digest(workspace.path(), &declared2, &[]).unwrap();
        assert_ne!(one, two);
    }

    #[test]
    fn input_digest_stable_under_dep_reordering() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("a.swift"), b"print(1)").unwrap();
        let declared = DeclaredAction {
            argv: vec!["swiftc".to_string()],
            inputs: vec!["a.swift".to_string()],
            outputs: vec![".once/out/A.a".to_string()],
            env: BTreeMap::new(),
            cacheable: true,
            toolchain_identity: None,
            identifier: None,
        };
        let a = compose_input_digest(
            workspace.path(),
            &declared,
            &[
                ("dep1".to_string(), Digest::of_bytes(b"d1")),
                ("dep2".to_string(), Digest::of_bytes(b"d2")),
            ],
        )
        .unwrap();
        let b = compose_input_digest(
            workspace.path(),
            &declared,
            &[
                ("dep2".to_string(), Digest::of_bytes(b"d2")),
                ("dep1".to_string(), Digest::of_bytes(b"d1")),
            ],
        )
        .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn target_action_digest_preserves_single_action_digest() {
        let action = Digest::of_bytes(b"action");

        assert_eq!(compose_target_action_digest("Root", &[action]), action);
    }

    #[test]
    fn target_action_digest_for_empty_actions_is_target_specific() {
        let root = compose_target_action_digest("Root", &[]);
        let same_root = compose_target_action_digest("Root", &[]);
        let other = compose_target_action_digest("Other", &[]);

        assert_eq!(root, same_root);
        assert_ne!(root, other);
    }

    #[test]
    fn target_action_digest_includes_all_declared_actions_in_order() {
        let first = Digest::of_bytes(b"first");
        let second = Digest::of_bytes(b"second");
        let changed_second = Digest::of_bytes(b"changed-second");

        let original = compose_target_action_digest("Root", &[first, second]);
        let changed = compose_target_action_digest("Root", &[first, changed_second]);
        let reordered = compose_target_action_digest("Root", &[second, first]);

        assert_ne!(original, changed);
        assert_ne!(original, reordered);
    }
}
