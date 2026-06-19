use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use once_cas::{ActionResult, CacheProvider, Digest};
use once_core::{
    Action, EvidenceCacheState, EvidenceSubject, InputDigestBuilder, OutputSymlinkMode,
    ResourceRequest, RunOpts, WorkspacePath,
};
use once_frontend::analysis::{AnalysisResult, DeclaredAction};
use once_frontend::GraphTarget;
use tokio::process::Command;

use super::BuildOutcome;

const FAILURE_OUTPUT_LIMIT: usize = 16 * 1024;

struct DeclaredActionRun<'a> {
    workspace: &'a Path,
    cache: &'a CacheProvider,
    module_source_digest: Digest,
    target_id: &'a str,
    capability: &'a str,
    index: usize,
    declared: DeclaredAction,
    input_action_digests: &'a [(String, Digest)],
}

struct DeclaredActionOutcome {
    digest: Digest,
    input_digest: Option<Digest>,
    cache_tag: &'static str,
    cache_state: EvidenceCacheState,
    result: ActionResult,
}

/// Materialise each declared action through the action cache, then
/// fold the analysis provider directly into the build outcome.
///
/// Returns a boxed future intentionally because the concrete future
/// captures declared action state and cache execution state. Boxing at
/// this boundary keeps parent graph futures small enough for
/// `clippy::large_futures` and centralizes the allocation.
pub(super) fn run_declared_actions<'a>(
    workspace: &'a Path,
    cache: &'a CacheProvider,
    module_source_digest: Digest,
    target: &'a GraphTarget,
    capability: &'a str,
    analysis: AnalysisResult,
    dep_action_digests: &'a [(String, Digest)],
) -> Pin<Box<dyn Future<Output = Result<BuildOutcome>> + Send + 'a>> {
    Box::pin(async move {
        let AnalysisResult {
            actions, provider, ..
        } = analysis;
        tracing::debug!(
            target = %target.label.id,
            declared_actions = actions.len(),
            dep_action_digests = dep_action_digests.len(),
            "running declared graph actions"
        );
        let mut action_digests = Vec::new();
        let mut input_digests = Vec::new();
        let mut last_cache_tag = "miss";
        let mut last_cache_state = EvidenceCacheState::Miss;
        let mut all_outputs = Vec::new();
        let mut aggregate_result = ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs: BTreeMap::new(),
        };

        for (index, declared) in actions.into_iter().enumerate() {
            all_outputs.extend(declared.outputs.iter().cloned());
            let mut input_action_digests = dep_action_digests.to_vec();
            input_action_digests.extend(
                action_digests
                    .iter()
                    .enumerate()
                    .map(|(prior_index, digest)| (format!("same-target:{prior_index}"), *digest)),
            );

            let outcome = run_declared_action(DeclaredActionRun {
                workspace,
                cache,
                module_source_digest,
                target_id: &target.label.id,
                capability,
                index,
                declared,
                input_action_digests: &input_action_digests,
            })
            .await?;
            action_digests.push(outcome.digest);
            if let Some(input_digest) = outcome.input_digest {
                input_digests.push(input_digest);
            }
            last_cache_tag = outcome.cache_tag;
            last_cache_state = outcome.cache_state;
            aggregate_result.stdout = outcome.result.stdout;
            aggregate_result.stderr = outcome.result.stderr;
            aggregate_result.outputs.extend(outcome.result.outputs);
        }

        Ok(BuildOutcome {
            provider,
            action_digest: compose_target_action_digest(&target.label.id, &action_digests),
            input_digest: compose_target_input_digest(&input_digests),
            outputs: all_outputs,
            cache_tag: last_cache_tag,
            cache_state: last_cache_state,
            result: aggregate_result,
        })
    })
}

async fn run_declared_action(run: DeclaredActionRun<'_>) -> Result<DeclaredActionOutcome> {
    let DeclaredActionRun {
        workspace,
        cache,
        module_source_digest,
        target_id,
        capability,
        index,
        declared,
        input_action_digests,
    } = run;
    let identifier_for_error = declared
        .identifier
        .clone()
        .unwrap_or_else(|| "<anonymous>".to_string());
    let cacheable = declared.cacheable;
    tracing::debug!(
        target = %target_id,
        action_index = index,
        identifier = %identifier_for_error,
        cacheable,
        inputs = declared.inputs.len(),
        outputs = declared.outputs.len(),
        "preparing declared graph action"
    );
    let action = declared_to_action(
        workspace,
        declared,
        module_source_digest,
        input_action_digests,
    )
    .with_context(|| format!("building action {index} for {target_id} ({identifier_for_error})"))?;

    if cacheable {
        run_cacheable_declared_action(
            workspace,
            cache,
            target_id,
            capability,
            index,
            &identifier_for_error,
            action,
        )
        .await
    } else {
        run_uncacheable_declared_action(
            workspace,
            cache,
            target_id,
            capability,
            index,
            &identifier_for_error,
            action,
        )
        .await
    }
}

async fn run_cacheable_declared_action(
    workspace: &Path,
    cache: &CacheProvider,
    target_id: &str,
    capability: &str,
    index: usize,
    identifier_for_error: &str,
    action: Action,
) -> Result<DeclaredActionOutcome> {
    let outcome = once_core::run_with_cache(&action, workspace, cache, RunOpts::default())
        .await
        .with_context(|| {
            format!("executing action {index} for {target_id} ({identifier_for_error})")
        })?;
    let exit_code = outcome.result.exit_code;
    if exit_code != 0 {
        record_declared_action_evidence(
            workspace,
            target_id,
            capability,
            &action,
            outcome.action,
            EvidenceCacheState::from(outcome.cache),
            &outcome.result,
        )
        .await;
        anyhow::bail!(
            "{}",
            declared_action_failure_message(
                cache,
                identifier_for_error,
                index,
                target_id,
                exit_code,
                &outcome.result,
            )
            .await
        );
    }
    let cache_tag = crate::commands::util::cache_tag(outcome.cache);
    let cache_state = EvidenceCacheState::from(outcome.cache);
    tracing::debug!(
        target = %target_id,
        action_index = index,
        identifier = %identifier_for_error,
        cache = cache_tag,
        action_digest = %outcome.action,
        "completed cacheable declared graph action"
    );
    Ok(DeclaredActionOutcome {
        digest: outcome.action,
        input_digest: action.input_digest(),
        cache_tag,
        cache_state,
        result: outcome.result,
    })
}

async fn run_uncacheable_declared_action(
    workspace: &Path,
    cache: &CacheProvider,
    target_id: &str,
    capability: &str,
    index: usize,
    identifier_for_error: &str,
    action: Action,
) -> Result<DeclaredActionOutcome> {
    let action_digest = action.digest();
    let result = run_uncached_action(&action, workspace, cache)
        .await
        .with_context(|| {
            format!("executing action {index} for {target_id} ({identifier_for_error})")
        })?;
    let exit_code = result.exit_code;
    if exit_code != 0 {
        record_declared_action_evidence(
            workspace,
            target_id,
            capability,
            &action,
            action_digest,
            EvidenceCacheState::Bypass,
            &result,
        )
        .await;
        anyhow::bail!(
            "{}",
            declared_action_failure_message(
                cache,
                identifier_for_error,
                index,
                target_id,
                exit_code,
                &result,
            )
            .await
        );
    }
    tracing::debug!(
        target = %target_id,
        action_index = index,
        identifier = %identifier_for_error,
        action_digest = %action_digest,
        "completed uncached declared graph action"
    );
    Ok(DeclaredActionOutcome {
        digest: action_digest,
        input_digest: action.input_digest(),
        cache_tag: "bypass",
        cache_state: EvidenceCacheState::Bypass,
        result,
    })
}

async fn record_declared_action_evidence(
    workspace: &Path,
    target_id: &str,
    capability: &str,
    action: &Action,
    action_digest: Digest,
    cache: EvidenceCacheState,
    result: &ActionResult,
) {
    crate::commands::evidence::record_action_result(
        workspace,
        EvidenceSubject::target(target_id, capability),
        action_digest,
        action.input_digest(),
        cache,
        result,
    )
    .await;
}

async fn declared_action_failure_message(
    cache: &CacheProvider,
    identifier: &str,
    index: usize,
    target: &str,
    exit_code: i32,
    result: &ActionResult,
) -> String {
    let mut message =
        format!("{identifier} ({index}) failed for {target} with exit code {exit_code}");
    append_captured_output(cache, &mut message, "stdout", result.stdout.as_ref()).await;
    append_captured_output(cache, &mut message, "stderr", result.stderr.as_ref()).await;
    message
}

async fn append_captured_output(
    cache: &CacheProvider,
    message: &mut String,
    name: &str,
    digest: Option<&Digest>,
) {
    let Some(digest) = digest else {
        return;
    };
    let bytes = match cache.get_blob(digest).await {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::warn!(
                output = name,
                digest = %digest,
                error = %err,
                "failed to read captured declared action output"
            );
            return;
        }
    };
    if bytes.is_empty() {
        return;
    }
    let (prefix, slice) = if bytes.len() > FAILURE_OUTPUT_LIMIT {
        (
            format!("last {FAILURE_OUTPUT_LIMIT} bytes of "),
            &bytes[bytes.len() - FAILURE_OUTPUT_LIMIT..],
        )
    } else {
        (String::new(), bytes.as_slice())
    };
    message.push_str("\n\n");
    message.push_str(&prefix);
    message.push_str(name);
    message.push_str(":\n");
    message.push_str(&String::from_utf8_lossy(slice));
}

async fn run_uncached_action(
    action: &Action,
    workspace: &Path,
    cache: &CacheProvider,
) -> Result<ActionResult> {
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
            command.stdout(Stdio::piped());
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
            let mut result = ActionResult {
                exit_code: output.status.code().unwrap_or(-1),
                stdout: Some(cache.put_blob(&output.stdout).await?),
                stderr: Some(cache.put_blob(&output.stderr).await?),
                outputs: BTreeMap::new(),
            };
            if result.exit_code == 0 {
                result.outputs = capture_uncached_outputs(outputs, workspace, cache).await?;
            }
            Ok(result)
        }
    }
}

async fn capture_uncached_outputs(
    outputs: &[WorkspacePath],
    workspace: &Path,
    cache: &CacheProvider,
) -> Result<BTreeMap<String, Digest>> {
    let mut captured = BTreeMap::new();
    for output in outputs {
        let absolute = output.resolve(workspace);
        let metadata = match tokio::fs::metadata(&absolute).await {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                anyhow::bail!(
                    "declared action completed without producing output `{}`",
                    output.as_str()
                );
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("reading declared action output `{}`", output.as_str())
                });
            }
        };
        if metadata.is_dir() {
            tracing::debug!(
                output = output.as_str(),
                "skipping uncached directory output evidence"
            );
            continue;
        }
        let bytes = tokio::fs::read(&absolute)
            .await
            .with_context(|| format!("reading declared action output `{}`", output.as_str()))?;
        captured.insert(output.as_str().to_string(), cache.put_blob(&bytes).await?);
    }
    Ok(captured)
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

fn compose_target_input_digest(input_digests: &[Digest]) -> Option<Digest> {
    match input_digests {
        [] => None,
        [digest] => Some(*digest),
        _ => {
            let mut builder = InputDigestBuilder::new(b"once.target.inputs.v1\0");
            for (index, digest) in input_digests.iter().enumerate() {
                let key = format!("input:{index}");
                builder.push_keyed(key.as_bytes(), digest);
            }
            Some(builder.finish())
        }
    }
}

fn declared_to_action(
    workspace: &Path,
    declared: DeclaredAction,
    module_source_digest: Digest,
    dep_action_digests: &[(String, Digest)],
) -> Result<Action> {
    let env_keys = declared.env.keys().cloned().collect::<Vec<_>>();
    tracing::trace!(
        identifier = ?declared.identifier,
        argv_len = declared.argv.len(),
        env_keys = ?env_keys,
        inputs = declared.inputs.len(),
        outputs = declared.outputs.len(),
        "declared graph action"
    );
    let input_digest = compose_input_digest(
        workspace,
        &declared,
        module_source_digest,
        dep_action_digests,
    )?;
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
    module_source_digest: Digest,
    dep_action_digests: &[(String, Digest)],
) -> Result<Digest> {
    let mut builder = InputDigestBuilder::new(b"once.declared_action.input.v1\0");
    // Keep the legacy namespace so terminology-only renames do not
    // invalidate existing declared-action cache entries.
    builder.push_keyed(b"rules", &module_source_digest);
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

    fn module_digest() -> Digest {
        Digest::of_bytes(b"modules")
    }

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

        let action = declared_to_action(workspace.path(), declared, module_digest(), &[]).unwrap();

        assert!(workspace.path().join(".once/out/x").is_dir());
        assert!(workspace.path().join(".once/out/x/sub").is_dir());
        let Action::RunCommand { argv, .. } = action;
        assert_eq!(argv, vec!["tool".to_string(), "--version".to_string()]);
    }

    #[tokio::test]
    async fn declared_action_failure_message_appends_captured_output() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let stdout = cache.put_blob(b"visible stdout").await.unwrap();
        let stderr = cache.put_blob(b"visible stderr").await.unwrap();
        let result = ActionResult {
            exit_code: 7,
            stdout: Some(stdout),
            stderr: Some(stderr),
            outputs: BTreeMap::new(),
        };

        let message =
            declared_action_failure_message(&cache, "target:action", 2, "target", 7, &result).await;

        assert!(message.contains("target:action (2) failed for target with exit code 7"));
        assert!(message.contains("stdout:\nvisible stdout"));
        assert!(message.contains("stderr:\nvisible stderr"));
    }

    #[tokio::test]
    async fn declared_action_failure_message_truncates_large_output() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let mut bytes = b"drop-me".to_vec();
        bytes.extend(std::iter::repeat_n(b'x', FAILURE_OUTPUT_LIMIT));
        let stdout = cache.put_blob(&bytes).await.unwrap();
        let result = ActionResult {
            exit_code: 1,
            stdout: Some(stdout),
            stderr: None,
            outputs: BTreeMap::new(),
        };

        let message = declared_action_failure_message(&cache, "id", 0, "target", 1, &result).await;

        assert!(message.contains("last 16384 bytes of stdout:\n"));
        assert!(!message.contains("drop-me"));
        assert!(message.ends_with(&"x".repeat(FAILURE_OUTPUT_LIMIT)));
    }

    #[tokio::test]
    async fn append_captured_output_ignores_missing_digest_and_missing_blob() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let missing = Digest::of_bytes(b"missing");
        let mut message = "base".to_string();

        append_captured_output(&cache, &mut message, "stdout", None).await;
        append_captured_output(&cache, &mut message, "stdout", Some(&missing)).await;

        assert_eq!(message, "base");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uncached_action_executes_each_time() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
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

        run_uncached_action(&action, workspace.path(), &cache)
            .await
            .unwrap();
        run_uncached_action(&action, workspace.path(), &cache)
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(workspace.path().join("counter")).unwrap(),
            "xx"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uncached_action_succeeds_when_declared_output_exists() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let action = Action::RunCommand {
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf ok > .once/out/result.txt".to_string(),
            ],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            outputs: vec![WorkspacePath::try_from(".once/out/result.txt").unwrap()],
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            timeout_ms: None,
            remote: None,
        };
        std::fs::create_dir_all(workspace.path().join(".once/out")).unwrap();

        let result = run_uncached_action(&action, workspace.path(), &cache)
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.outputs.contains_key(".once/out/result.txt"));
        assert_eq!(
            std::fs::read_to_string(workspace.path().join(".once/out/result.txt")).unwrap(),
            "ok"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uncached_action_errors_when_declared_output_is_missing() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let action = Action::RunCommand {
            argv: vec!["/bin/sh".to_string(), "-c".to_string(), ":".to_string()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            outputs: vec![WorkspacePath::try_from(".once/out/missing.txt").unwrap()],
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            timeout_ms: None,
            remote: None,
        };

        let err = run_uncached_action(&action, workspace.path(), &cache)
            .await
            .unwrap_err()
            .to_string();

        assert!(
            err.contains(
                "declared action completed without producing output `.once/out/missing.txt`"
            ),
            "{err}"
        );
    }

    #[test]
    fn input_digest_changes_with_toolchain_identity() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("input.txt"), b"content").unwrap();
        let declared = DeclaredAction {
            argv: vec!["tool".to_string()],
            inputs: vec!["input.txt".to_string()],
            outputs: vec![".once/out/A.a".to_string()],
            env: BTreeMap::new(),
            cacheable: true,
            toolchain_identity: Some("id-1".to_string()),
            identifier: None,
        };
        let one = compose_input_digest(workspace.path(), &declared, module_digest(), &[]).unwrap();
        let declared2 = DeclaredAction {
            toolchain_identity: Some("id-2".to_string()),
            ..declared.clone()
        };
        let two = compose_input_digest(workspace.path(), &declared2, module_digest(), &[]).unwrap();
        assert_ne!(one, two);
    }

    #[test]
    fn input_digest_changes_with_module_source_digest() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("input.txt"), b"content").unwrap();
        let declared = DeclaredAction {
            argv: vec!["tool".to_string()],
            inputs: vec!["input.txt".to_string()],
            outputs: vec![".once/out/A.a".to_string()],
            env: BTreeMap::new(),
            cacheable: true,
            toolchain_identity: None,
            identifier: None,
        };
        let one = compose_input_digest(
            workspace.path(),
            &declared,
            Digest::of_bytes(b"modules-1"),
            &[],
        )
        .unwrap();
        let two = compose_input_digest(
            workspace.path(),
            &declared,
            Digest::of_bytes(b"modules-2"),
            &[],
        )
        .unwrap();

        assert_ne!(one, two);
    }

    #[test]
    fn input_digest_stable_under_dep_reordering() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("input.txt"), b"content").unwrap();
        let declared = DeclaredAction {
            argv: vec!["tool".to_string()],
            inputs: vec!["input.txt".to_string()],
            outputs: vec![".once/out/A.a".to_string()],
            env: BTreeMap::new(),
            cacheable: true,
            toolchain_identity: None,
            identifier: None,
        };
        let a = compose_input_digest(
            workspace.path(),
            &declared,
            module_digest(),
            &[
                ("dep1".to_string(), Digest::of_bytes(b"d1")),
                ("dep2".to_string(), Digest::of_bytes(b"d2")),
            ],
        )
        .unwrap();
        let b = compose_input_digest(
            workspace.path(),
            &declared,
            module_digest(),
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
