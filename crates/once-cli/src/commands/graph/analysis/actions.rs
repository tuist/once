use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use once_cas::{ActionResult, CacheProvider, Digest};
use once_core::{
    Action, CopyPathMode, EvidenceCacheState, EvidenceSubject, InputDigestBuilder,
    OutputSymlinkMode, PreparePathMode, ResourceRequest, RunOpts, WorkspacePath,
};
use once_frontend::analysis::{
    AnalysisResult, DeclaredAction, DeclaredActionOperation, DeclaredArgFile,
    DeclaredArgFileFormat, DeclaredCopyPathMode, DeclaredPreparePathMode,
};
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
    record_success_evidence: bool,
}

struct DeclaredActionOutcome {
    digest: Digest,
    input_digest: Option<Digest>,
    cache_tag: &'static str,
    cache_state: EvidenceCacheState,
    result: ActionResult,
}

struct DeclaredActionContext<'a> {
    workspace: &'a Path,
    cache: &'a CacheProvider,
    target_id: &'a str,
    capability: &'a str,
    index: usize,
    identifier: &'a str,
    arg_files: &'a [DeclaredArgFile],
    record_success_evidence: bool,
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
        // A single-action target is fully represented by the caller's
        // capability-level record. Multi-action targets need per-action
        // success evidence so individual streams and outputs stay visible.
        let record_success_evidence = actions.len() > 1;

        for (index, declared) in actions.into_iter().enumerate() {
            all_outputs.extend(declared.outputs.iter().cloned());
            let mut input_action_digests = dep_action_digests.to_vec();
            input_action_digests.extend(
                action_digests
                    .iter()
                    .enumerate()
                    .map(|(prior_index, digest)| (format!("same-target:{prior_index}"), *digest)),
            );

            let outcome = Box::pin(run_declared_action(DeclaredActionRun {
                workspace,
                cache,
                module_source_digest,
                target_id: &target.label.id,
                capability,
                index,
                declared,
                input_action_digests: &input_action_digests,
                record_success_evidence,
            }))
            .await?;
            action_digests.push(outcome.digest);
            if let Some(input_digest) = outcome.input_digest {
                input_digests.push(input_digest);
            }
            last_cache_tag = outcome.cache_tag;
            last_cache_state = outcome.cache_state;
            if !record_success_evidence {
                aggregate_result.stdout = outcome.result.stdout;
                aggregate_result.stderr = outcome.result.stderr;
            }
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
        record_success_evidence,
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
    materialize_declared_arg_files(workspace, &declared.arg_files).with_context(|| {
        format!("writing arg files for action {index} for {target_id} ({identifier_for_error})")
    })?;
    let action = declared_to_action(
        workspace,
        &declared,
        module_source_digest,
        input_action_digests,
    )
    .with_context(|| format!("building action {index} for {target_id} ({identifier_for_error})"))?;
    let context = DeclaredActionContext {
        workspace,
        cache,
        target_id,
        capability,
        index,
        identifier: &identifier_for_error,
        arg_files: &declared.arg_files,
        record_success_evidence,
    };

    if cacheable {
        run_cacheable_declared_action(context, action).await
    } else {
        run_uncacheable_declared_action(context, action).await
    }
}

async fn run_cacheable_declared_action(
    context: DeclaredActionContext<'_>,
    action: Action,
) -> Result<DeclaredActionOutcome> {
    let outcome = once_core::run_with_cache(
        &action,
        context.workspace,
        context.cache,
        RunOpts::default(),
    )
    .await
    .with_context(|| {
        format!(
            "executing action {} for {} ({})",
            context.index, context.target_id, context.identifier
        )
    })?;
    let exit_code = outcome.result.exit_code;
    if exit_code != 0 {
        record_declared_action_evidence(
            context.workspace,
            context.target_id,
            context.capability,
            &action,
            outcome.action,
            EvidenceCacheState::from(outcome.cache),
            &outcome.result,
        )
        .await;
        anyhow::bail!(
            "{}",
            declared_action_failure_message(
                context.cache,
                context.identifier,
                context.index,
                context.target_id,
                exit_code,
                context.arg_files,
                &outcome.result,
            )
            .await
        );
    }
    let cache_tag = crate::commands::util::cache_tag(outcome.cache);
    let cache_state = EvidenceCacheState::from(outcome.cache);
    tracing::debug!(
        target = %context.target_id,
        action_index = context.index,
        identifier = %context.identifier,
        cache = cache_tag,
        action_digest = %outcome.action,
        "completed cacheable declared graph action"
    );
    if context.record_success_evidence {
        record_declared_action_evidence(
            context.workspace,
            context.target_id,
            context.capability,
            &action,
            outcome.action,
            cache_state,
            &outcome.result,
        )
        .await;
    }
    Ok(DeclaredActionOutcome {
        digest: outcome.action,
        input_digest: action.input_digest(),
        cache_tag,
        cache_state,
        result: outcome.result,
    })
}

async fn run_uncacheable_declared_action(
    context: DeclaredActionContext<'_>,
    action: Action,
) -> Result<DeclaredActionOutcome> {
    let action_digest = action.digest();
    let result = run_uncached_action(&action, context.workspace, context.cache)
        .await
        .with_context(|| {
            format!(
                "executing action {} for {} ({})",
                context.index, context.target_id, context.identifier
            )
        })?;
    let exit_code = result.exit_code;
    if exit_code != 0 {
        record_declared_action_evidence(
            context.workspace,
            context.target_id,
            context.capability,
            &action,
            action_digest,
            EvidenceCacheState::Bypass,
            &result,
        )
        .await;
        anyhow::bail!(
            "{}",
            declared_action_failure_message(
                context.cache,
                context.identifier,
                context.index,
                context.target_id,
                exit_code,
                context.arg_files,
                &result,
            )
            .await
        );
    }
    tracing::debug!(
        target = %context.target_id,
        action_index = context.index,
        identifier = %context.identifier,
        action_digest = %action_digest,
        "completed uncached declared graph action"
    );
    if context.record_success_evidence {
        record_declared_action_evidence(
            context.workspace,
            context.target_id,
            context.capability,
            &action,
            action_digest,
            EvidenceCacheState::Bypass,
            &result,
        )
        .await;
    }
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
    arg_files: &[DeclaredArgFile],
    result: &ActionResult,
) -> String {
    let mut message =
        format!("{identifier} ({index}) failed for {target} with exit code {exit_code}");
    append_declared_arg_files(&mut message, arg_files);
    append_captured_output(cache, &mut message, "stdout", result.stdout.as_ref()).await;
    append_captured_output(cache, &mut message, "stderr", result.stderr.as_ref()).await;
    message
}

fn append_declared_arg_files(message: &mut String, arg_files: &[DeclaredArgFile]) {
    if arg_files.is_empty() {
        return;
    }

    message.push_str("\n\narg files:");
    for arg_file in arg_files {
        let _ = write!(
            message,
            "\n{} [{}], {} args",
            arg_file.path,
            declared_arg_file_format_name(arg_file.format),
            arg_file.args.len()
        );
        append_arg_file_arg_list(message, "first args", arg_file.args.iter().take(32));
        let start = arg_file.args.len().saturating_sub(16);
        if start > 32 {
            append_arg_file_arg_list(message, "last args", arg_file.args.iter().skip(start));
        }
    }
}

fn append_arg_file_arg_list<'a>(
    message: &mut String,
    label: &str,
    mut args: impl Iterator<Item = &'a String>,
) {
    let Some(first_arg) = args.next() else {
        return;
    };

    let _ = write!(message, "\n{label}:");
    let _ = write!(message, "\n  {first_arg}");
    for arg in args {
        let _ = write!(message, "\n  {arg}");
    }
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
            let mut result = ActionResult {
                exit_code: output.status.code().unwrap_or(-1),
                stdout: None,
                stderr: Some(cache.put_blob(&output.stderr).await?),
                outputs: BTreeMap::new(),
            };
            if result.exit_code == 0 {
                result.outputs = capture_uncached_outputs(outputs, workspace, cache).await?;
            }
            Ok(result)
        }
        Action::WriteFile { .. }
        | Action::CopyPath { .. }
        | Action::PreparePath { .. }
        | Action::WriteTreeDigest { .. } => {
            once_core::run_uncached(action, workspace, cache, false)
                .await
                .map_err(Into::into)
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
        let metadata = match tokio::fs::symlink_metadata(&absolute).await {
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
        if metadata.file_type().is_symlink() {
            let target = tokio::fs::read_link(&absolute).await.with_context(|| {
                format!("reading declared output symlink `{}`", output.as_str())
            })?;
            let manifest = format!("once.symlink_output.v1\n{}\n", target.to_string_lossy());
            captured.insert(
                output.as_str().to_string(),
                cache.put_blob(manifest.as_bytes()).await?,
            );
            continue;
        }
        if metadata.is_dir() {
            let manifest = tokio::task::spawn_blocking({
                let absolute = absolute.clone();
                move || directory_manifest_bytes(&absolute)
            })
            .await
            .context("joining declared directory output capture")??;
            captured.insert(
                output.as_str().to_string(),
                cache.put_blob(&manifest).await?,
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

fn directory_manifest_bytes(root: &Path) -> Result<Vec<u8>> {
    let mut entries = Vec::new();
    collect_directory_manifest(root, root, &mut entries)?;
    let mut manifest = b"once.directory_output.v1\n".to_vec();
    for entry in entries {
        manifest.extend_from_slice(entry.as_bytes());
        manifest.push(b'\n');
    }
    Ok(manifest)
}

fn collect_directory_manifest(root: &Path, dir: &Path, entries: &mut Vec<String>) -> Result<()> {
    let mut children = std::fs::read_dir(dir)
        .with_context(|| format!("reading declared directory output `{}`", dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("reading declared directory output `{}`", dir.display()))?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let path = child.path();
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path.as_path())
            .to_string_lossy()
            .replace('\\', "/");
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("reading declared output metadata `{}`", path.display()))?;
        if metadata.is_dir() {
            entries.push(format!("dir\t{relative}"));
            collect_directory_manifest(root, &path, entries)?;
        } else if metadata.is_file() {
            let bytes = std::fs::read(&path)
                .with_context(|| format!("reading declared output file `{}`", path.display()))?;
            entries.push(format!("file\t{relative}\t{}", Digest::of_bytes(&bytes)));
        } else if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(&path)
                .with_context(|| format!("reading declared output symlink `{}`", path.display()))?;
            entries.push(format!(
                "symlink\t{relative}\t{}",
                Digest::of_bytes(target.to_string_lossy().as_bytes())
            ));
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
    declared: &DeclaredAction,
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
        declared,
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
    match &declared.operation {
        None => Ok(Action::RunCommand {
            argv: declared.argv.clone(),
            env: declared.env.clone(),
            cwd: None,
            input_digest: Some(input_digest),
            outputs,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            timeout_ms: None,
            remote: None,
        }),
        Some(operation) => operation_to_action(operation.clone(), input_digest),
    }
}

fn materialize_declared_arg_files(workspace: &Path, arg_files: &[DeclaredArgFile]) -> Result<()> {
    for arg_file in arg_files {
        let path = workspace_path(&arg_file.path, "arg_files path")?;
        let absolute = path.resolve(workspace);
        if let Some(parent) = absolute.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("creating parent directory for arg file `{}`", path.as_str())
            })?;
        }
        let content = declared_arg_file_content(arg_file)?;
        std::fs::write(&absolute, content)
            .with_context(|| format!("writing arg file `{}`", path.as_str()))?;
    }
    Ok(())
}

fn declared_arg_file_content(arg_file: &DeclaredArgFile) -> Result<Vec<u8>> {
    match arg_file.format {
        DeclaredArgFileFormat::LineDelimited => declared_arg_file_lines(arg_file, |arg| {
            validate_arg_file_line(arg_file, arg)?;
            Ok(arg.to_string())
        }),
        DeclaredArgFileFormat::RustcResponse => {
            declared_arg_file_lines(arg_file, |arg| rustc_response_arg(arg_file, arg))
        }
    }
}

fn declared_arg_file_lines(
    arg_file: &DeclaredArgFile,
    format: impl Fn(&str) -> Result<String>,
) -> Result<Vec<u8>> {
    let mut content = Vec::new();
    for arg in &arg_file.args {
        let line = format(arg)?;
        content.extend_from_slice(line.as_bytes());
        content.push(b'\n');
    }
    Ok(content)
}

fn rustc_response_arg(arg_file: &DeclaredArgFile, arg: &str) -> Result<String> {
    validate_arg_file_line(arg_file, arg)?;
    Ok(arg.to_string())
}

fn validate_arg_file_line(arg_file: &DeclaredArgFile, arg: &str) -> Result<()> {
    if arg.contains('\n') || arg.contains('\r') {
        anyhow::bail!(
            "{} arg file `{}` contains an argument with a newline",
            declared_arg_file_format_name(arg_file.format),
            arg_file.path
        );
    }
    Ok(())
}

fn declared_arg_file_format_name(format: DeclaredArgFileFormat) -> &'static str {
    match format {
        DeclaredArgFileFormat::LineDelimited => "line-delimited",
        DeclaredArgFileFormat::RustcResponse => "rustc-response",
    }
}

fn operation_to_action(operation: DeclaredActionOperation, input_digest: Digest) -> Result<Action> {
    Ok(match operation {
        DeclaredActionOperation::WriteFile { path, bytes } => Action::WriteFile {
            path: workspace_path(&path, "write_path path")?,
            bytes,
            input_digest: Some(input_digest),
        },
        DeclaredActionOperation::CopyPath {
            sources,
            destination,
            mode,
        } => Action::CopyPath {
            sources: sources
                .iter()
                .map(|source| workspace_path(source, "copy_path source"))
                .collect::<Result<Vec<_>>>()?,
            destination: workspace_path(&destination, "copy_path destination")?,
            mode: match mode {
                DeclaredCopyPathMode::File => CopyPathMode::File,
                DeclaredCopyPathMode::Tree => CopyPathMode::Tree,
            },
            input_digest: Some(input_digest),
        },
        DeclaredActionOperation::PreparePath { path, mode } => Action::PreparePath {
            path: workspace_path(&path, "prepare_path path")?,
            mode: match mode {
                DeclaredPreparePathMode::Remove => PreparePathMode::Remove,
                DeclaredPreparePathMode::Directory => PreparePathMode::Directory,
            },
            input_digest: Some(input_digest),
        },
        DeclaredActionOperation::WriteTreeDigest {
            root,
            output,
            include_suffixes,
        } => Action::WriteTreeDigest {
            root: workspace_path(&root, "write_tree_digest root")?,
            output: workspace_path(&output, "write_tree_digest output")?,
            include_suffixes,
            input_digest: Some(input_digest),
        },
    })
}

fn workspace_path(path: &str, context: &str) -> Result<WorkspacePath> {
    WorkspacePath::try_from(path).with_context(|| format!("invalid {context} `{path}`"))
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
    if let Some(operation) = &declared.operation {
        let encoded =
            serde_json::to_vec(operation).context("serializing declared action operation")?;
        builder.push_bytes(&encoded);
    }
    for arg in &declared.argv {
        builder.push_bytes(arg.as_bytes());
    }
    let encoded_arg_files =
        serde_json::to_vec(&declared.arg_files).context("serializing declared action arg files")?;
    builder.push_bytes(&encoded_arg_files);
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
            operation: None,
            argv: vec!["tool".to_string(), "--version".to_string()],
            arg_files: Vec::new(),
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

        let action = declared_to_action(workspace.path(), &declared, module_digest(), &[]).unwrap();

        assert!(workspace.path().join(".once/out/x").is_dir());
        assert!(workspace.path().join(".once/out/x/sub").is_dir());
        let Action::RunCommand { argv, .. } = action else {
            panic!("command declaration should lower to RunCommand");
        };
        assert_eq!(argv, vec!["tool".to_string(), "--version".to_string()]);
    }

    #[test]
    fn materialize_declared_arg_files_writes_line_delimited_args() {
        let workspace = tempfile::tempdir().unwrap();
        let arg_files = vec![DeclaredArgFile {
            path: ".once/out/rust/rustc-features.rsp".to_string(),
            format: DeclaredArgFileFormat::LineDelimited,
            args: vec!["--cfg".to_string(), "feature=\"alloc\"".to_string()],
        }];

        materialize_declared_arg_files(workspace.path(), &arg_files).unwrap();

        assert_eq!(
            std::fs::read_to_string(workspace.path().join(".once/out/rust/rustc-features.rsp"))
                .unwrap(),
            "--cfg\nfeature=\"alloc\"\n"
        );
    }

    #[test]
    fn materialize_declared_arg_files_writes_rustc_response_args() {
        let workspace = tempfile::tempdir().unwrap();
        let arg_files = vec![DeclaredArgFile {
            path: ".once/out/rust/rustc-features.rsp".to_string(),
            format: DeclaredArgFileFormat::RustcResponse,
            args: vec!["--cfg".to_string(), "feature=\"alloc\"".to_string()],
        }];

        materialize_declared_arg_files(workspace.path(), &arg_files).unwrap();

        assert_eq!(
            std::fs::read_to_string(workspace.path().join(".once/out/rust/rustc-features.rsp"))
                .unwrap(),
            "--cfg\nfeature=\"alloc\"\n"
        );
    }

    #[test]
    fn rustc_response_args_keep_arguments_verbatim() {
        let arg_file = DeclaredArgFile {
            path: ".once/out/rust/rustc-features.rsp".to_string(),
            format: DeclaredArgFileFormat::RustcResponse,
            args: Vec::new(),
        };
        let cases = [
            "",
            "argument with spaces",
            r"C:\Program Files\Rust\lib",
            "tab\tseparated",
            "feature='alloc'",
            "feature=\"alloc\"",
        ];

        for case in cases {
            assert_eq!(
                rustc_response_arg(&arg_file, case).unwrap(),
                case,
                "rustc response argument should stay verbatim: {case:?}"
            );
        }
    }

    #[test]
    fn materialize_declared_arg_files_rejects_newline_args() {
        let workspace = tempfile::tempdir().unwrap();
        let arg_files = vec![DeclaredArgFile {
            path: ".once/out/rust/rustc-features.rsp".to_string(),
            format: DeclaredArgFileFormat::LineDelimited,
            args: vec!["feature=\"alloc\"\n--cfg".to_string()],
        }];

        let err = materialize_declared_arg_files(workspace.path(), &arg_files)
            .unwrap_err()
            .to_string();

        assert!(err.contains("contains an argument with a newline"), "{err}");
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
            declared_action_failure_message(&cache, "target:action", 2, "target", 7, &[], &result)
                .await;

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

        let message =
            declared_action_failure_message(&cache, "id", 0, "target", 1, &[], &result).await;

        assert!(message.contains("last 16384 bytes of stdout:\n"));
        assert!(!message.contains("drop-me"));
        assert!(message.ends_with(&"x".repeat(FAILURE_OUTPUT_LIMIT)));
    }

    #[tokio::test]
    async fn declared_action_failure_message_appends_arg_file_context() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let result = ActionResult {
            exit_code: 1,
            stdout: None,
            stderr: None,
            outputs: BTreeMap::new(),
        };
        let mut args = vec![
            "--crate-name".to_string(),
            "app".to_string(),
            "--extern=dep=.once/out/crates/dep/dep/libdep.rlib".to_string(),
            "-L".to_string(),
            "dependency=.once/out/crates/dep/dep".to_string(),
        ];
        args.extend((0..60).map(|index| format!("arg-{index}")));
        args.push("crates/app/src/lib.rs".to_string());
        let arg_files = vec![DeclaredArgFile {
            path: ".once/tmp/analysis/crates/app/app/rustc.rsp".to_string(),
            format: DeclaredArgFileFormat::RustcResponse,
            args,
        }];

        let message =
            declared_action_failure_message(&cache, "id", 0, "target", 1, &arg_files, &result)
                .await;

        assert!(message.contains("arg files:"));
        assert!(message.contains(".once/tmp/analysis/crates/app/app/rustc.rsp [rustc-response]"));
        assert!(message.contains("first args:\n  --crate-name"));
        assert!(message.contains("last args:"));
        assert!(message.contains("crates/app/src/lib.rs"));
        assert!(!message.contains("extern args:"));
        assert!(!message.contains("dependency search dirs:"));
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
    async fn uncached_action_discards_stdout_without_buffering_it() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let action = Action::RunCommand {
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf noisy-stdout".to_string(),
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

        let result = run_uncached_action(&action, workspace.path(), &cache)
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, None);
    }

    #[tokio::test]
    async fn capture_uncached_outputs_records_directory_tree_manifest() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        std::fs::create_dir_all(workspace.path().join(".once/out/tree/sub")).unwrap();
        std::fs::write(workspace.path().join(".once/out/tree/a.txt"), b"a").unwrap();
        std::fs::write(workspace.path().join(".once/out/tree/sub/b.txt"), b"b").unwrap();

        let outputs = capture_uncached_outputs(
            &[WorkspacePath::try_from(".once/out/tree").unwrap()],
            workspace.path(),
            &cache,
        )
        .await
        .unwrap();
        let digest = outputs.get(".once/out/tree").unwrap();
        let manifest = cache.get_blob(digest).await.unwrap();
        let manifest = String::from_utf8(manifest).unwrap();

        assert!(manifest.starts_with("once.directory_output.v1\n"));
        assert!(manifest.contains("dir\tsub\n"));
        assert!(manifest.contains("file\ta.txt\t"));
        assert!(manifest.contains("file\tsub/b.txt\t"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn capture_uncached_outputs_records_top_level_symlink_without_following_it() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        std::fs::create_dir_all(workspace.path().join(".once/out")).unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();
        std::os::unix::fs::symlink(outside.path(), workspace.path().join(".once/out/link"))
            .unwrap();

        let outputs = capture_uncached_outputs(
            &[WorkspacePath::try_from(".once/out/link").unwrap()],
            workspace.path(),
            &cache,
        )
        .await
        .unwrap();
        let digest = outputs.get(".once/out/link").unwrap();
        let manifest = cache.get_blob(digest).await.unwrap();
        let manifest = String::from_utf8(manifest).unwrap();

        assert!(manifest.starts_with("once.symlink_output.v1\n"));
        assert!(manifest.contains(&outside.path().to_string_lossy().to_string()));
        assert!(!manifest.contains("secret.txt"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn single_action_outcome_preserves_streams_for_capability_evidence() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let target = GraphTarget {
            label: once_frontend::TargetLabel {
                package: "tools".to_string(),
                name: "single".to_string(),
                id: "tools/single".to_string(),
            },
            kind: "demo_kind".to_string(),
            deps: Vec::new(),
            srcs: Vec::new(),
            attrs: BTreeMap::new(),
            capabilities: Vec::new(),
            providers: Vec::new(),
            diagnostics: Vec::new(),
        };
        let analysis = AnalysisResult {
            actions: vec![DeclaredAction {
                operation: None,
                argv: vec![
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "printf visible-stdout; printf visible-stderr >&2; printf ok > .once/out/one.txt"
                        .to_string(),
                ],
                arg_files: Vec::new(),
                inputs: Vec::new(),
                outputs: vec![".once/out/one.txt".to_string()],
                env: BTreeMap::new(),
                cacheable: true,
                toolchain_identity: None,
                identifier: Some("one".to_string()),
            }],
            provider: serde_json::json!({}),
            declared_outputs: Vec::new(),
        };

        let outcome = run_declared_actions(
            workspace.path(),
            &cache,
            module_digest(),
            &target,
            "build",
            analysis,
            &[],
        )
        .await
        .unwrap();

        let stdout = cache
            .get_blob(&outcome.result.stdout.unwrap())
            .await
            .unwrap();
        let stderr = cache
            .get_blob(&outcome.result.stderr.unwrap())
            .await
            .unwrap();
        assert_eq!(stdout, b"visible-stdout");
        assert_eq!(stderr, b"visible-stderr");
        assert!(outcome.result.outputs.contains_key(".once/out/one.txt"));
        let records = once_core::EvidenceStore::open_workspace(workspace.path())
            .load()
            .await
            .unwrap();
        assert_eq!(records.len(), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn declared_actions_record_success_evidence_per_action() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let target = GraphTarget {
            label: once_frontend::TargetLabel {
                package: "tools".to_string(),
                name: "demo".to_string(),
                id: "tools/demo".to_string(),
            },
            kind: "demo_kind".to_string(),
            deps: Vec::new(),
            srcs: Vec::new(),
            attrs: BTreeMap::new(),
            capabilities: Vec::new(),
            providers: Vec::new(),
            diagnostics: Vec::new(),
        };
        let action = |name: &str| DeclaredAction {
            operation: None,
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                format!("printf {name} > .once/out/{name}.txt"),
            ],
            arg_files: Vec::new(),
            inputs: Vec::new(),
            outputs: vec![format!(".once/out/{name}.txt")],
            env: BTreeMap::new(),
            cacheable: false,
            toolchain_identity: None,
            identifier: Some(name.to_string()),
        };
        let analysis = AnalysisResult {
            actions: vec![action("one"), action("two")],
            provider: serde_json::json!({}),
            declared_outputs: Vec::new(),
        };

        let outcome = run_declared_actions(
            workspace.path(),
            &cache,
            module_digest(),
            &target,
            "build",
            analysis,
            &[],
        )
        .await
        .unwrap();

        assert_eq!(outcome.result.stdout, None);
        assert_eq!(outcome.result.stderr, None);
        assert_eq!(outcome.result.outputs.len(), 2);
        let records = once_core::EvidenceStore::open_workspace(workspace.path())
            .load()
            .await
            .unwrap();
        assert_eq!(records.len(), 2);
        assert!(records
            .iter()
            .all(|record| record.subject.matches("tools/demo:build")));
        assert!(records
            .iter()
            .any(|record| record.outputs.contains_key(".once/out/one.txt")));
        assert!(records
            .iter()
            .any(|record| record.outputs.contains_key(".once/out/two.txt")));
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
            operation: None,
            argv: vec!["tool".to_string()],
            arg_files: Vec::new(),
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
    fn input_digest_changes_with_declared_arg_files() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("input.txt"), b"content").unwrap();
        let declared = DeclaredAction {
            operation: None,
            argv: vec!["tool".to_string(), "@.once/out/args.rsp".to_string()],
            arg_files: vec![DeclaredArgFile {
                path: ".once/out/args.rsp".to_string(),
                format: DeclaredArgFileFormat::LineDelimited,
                args: vec!["--cfg".to_string(), "feature=\"alloc\"".to_string()],
            }],
            inputs: vec!["input.txt".to_string()],
            outputs: vec![".once/out/A.a".to_string()],
            env: BTreeMap::new(),
            cacheable: true,
            toolchain_identity: None,
            identifier: None,
        };
        let one = compose_input_digest(workspace.path(), &declared, module_digest(), &[]).unwrap();
        let declared2 = DeclaredAction {
            arg_files: vec![DeclaredArgFile {
                path: ".once/out/args.rsp".to_string(),
                format: DeclaredArgFileFormat::LineDelimited,
                args: vec!["--cfg".to_string(), "feature=\"std\"".to_string()],
            }],
            ..declared
        };
        let two = compose_input_digest(workspace.path(), &declared2, module_digest(), &[]).unwrap();

        assert_ne!(one, two);
    }

    #[test]
    fn input_digest_changes_with_module_source_digest() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("input.txt"), b"content").unwrap();
        let declared = DeclaredAction {
            operation: None,
            argv: vec!["tool".to_string()],
            arg_files: Vec::new(),
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
            operation: None,
            argv: vec!["tool".to_string()],
            arg_files: Vec::new(),
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

    #[test]
    fn target_input_digest_handles_empty_single_and_multiple_inputs() {
        let first = Digest::of_bytes(b"first-input");
        let second = Digest::of_bytes(b"second-input");

        assert_eq!(compose_target_input_digest(&[]), None);
        assert_eq!(compose_target_input_digest(&[first]), Some(first));

        let original = compose_target_input_digest(&[first, second]).unwrap();
        let same = compose_target_input_digest(&[first, second]).unwrap();
        let reordered = compose_target_input_digest(&[second, first]).unwrap();

        assert_eq!(original, same);
        assert_ne!(original, reordered);
    }
}
