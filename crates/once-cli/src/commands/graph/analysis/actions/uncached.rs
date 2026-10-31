use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use once_cas::{ActionResult, CacheProvider, Digest};
use once_core::{
    resolve_execution_argv, resolve_execution_env, Action, SandboxMode, WorkspacePath,
};
use tokio::process::Command;

use super::{capture_step, legacy_outputs, render_execution_error, uncached_process, Producers};

pub(super) async fn execute_declared(
    context: &super::DeclaredActionContext<'_>,
    action: &Action,
    declared: &once_frontend::analysis::DeclaredAction,
    lease: &once_core::ExecutionPermit,
) -> Result<ActionResult> {
    if once_core::Executor::resolve(action)? == once_core::Executor::Local {
        return once_core::with_execution_permit(
            lease.clone(),
            Box::pin(run_uncached_action_admitted(
                action,
                context.workspace,
                context.cache,
                declared.inherit_parent_env,
                Some(lease.clone()),
                Some((context.producers, context.identifier)),
            )),
        )
        .await;
    }
    let result = super::provider::execute(
        action,
        context.workspace,
        context.cache,
        None,
        lease.clone(),
        &declared.clean_paths,
        &declared.create_dirs,
        &declared.arg_files,
    )
    .await?;
    super::provider::replay_output(
        &result,
        context.cache,
        context.output_observer,
        context.workspace,
        lease,
    )
    .await?;
    Ok(result)
}

#[cfg(test)]
pub(super) async fn run_uncached_action(
    action: &Action,
    workspace: &Path,
    cache: &CacheProvider,
    inherit_parent_env: bool,
) -> Result<ActionResult> {
    run_uncached_action_admitted(action, workspace, cache, inherit_parent_env, None, None).await
}

pub(super) async fn run_uncached_action_admitted(
    action: &Action,
    workspace: &Path,
    cache: &CacheProvider,
    inherit_parent_env: bool,
    lease: Option<once_core::ExecutionPermit>,
    producers: Option<(&Producers, &str)>,
) -> Result<ActionResult> {
    match action {
        Action::RunCommand {
            argv,
            env,
            cwd,
            timeout_ms,
            outputs,
            stdout_path,
            stderr_path,
            sandbox,
            network,
            ..
        } => {
            // Network denial is enforced only by the core runner, so a command
            // that asks for it must not take the direct spawn below.
            if *sandbox != SandboxMode::Off
                || network.is_denied()
                || once_core::Executor::resolve(action)? != once_core::Executor::Local
            {
                if inherit_parent_env {
                    anyhow::bail!(if network.is_denied() {
                        "inherit_parent_env cannot be combined with network = \"deny\"; remove one of them"
                    } else {
                        "inherit_parent_env is available only when the action sandbox is off"
                    });
                }
                return run_core_admitted(action, workspace, cache, lease).await;
            }
            let argv = resolve_execution_argv(argv, workspace);
            let env = resolve_execution_env(env, workspace);
            let (program, rest) = argv
                .split_first()
                .ok_or_else(|| anyhow::anyhow!("declared action has empty argv"))?;
            let mut command = Command::new(program);
            command.args(rest);
            if !inherit_parent_env {
                command.env_clear();
            }
            for (key, value) in &env {
                command.env(key, value);
            }
            command.stdin(Stdio::null());
            command.current_dir(
                cwd.as_ref()
                    .map_or_else(|| workspace.to_path_buf(), |path| path.resolve(workspace)),
            );
            command.kill_on_drop(true);

            let mut result = if stdout_path.is_some() || stderr_path.is_some() {
                run_uncached_redirected(
                    command,
                    stdout_path.as_deref(),
                    stderr_path.as_deref(),
                    *timeout_ms,
                    workspace,
                    cache,
                )
                .await?
            } else {
                run_uncached_discarding_stdout(command, *timeout_ms, cache).await?
            };
            if let Err(error) = action.normalize_result(&mut result) {
                return Err(render_execution_error(error, cache).await);
            }
            if result.exit_code == 0 {
                result.outputs = capture_uncached_outputs_admitted(
                    outputs,
                    workspace,
                    cache,
                    lease.as_ref(),
                    producers,
                )
                .await?;
            }
            Ok(result)
        }
        Action::WriteFile { .. }
        | Action::CopyPath { .. }
        | Action::LinkPath { .. }
        | Action::MaterializeHostFile { .. }
        | Action::MaterializeHostTree { .. }
        | Action::PreparePath { .. }
        | Action::WriteTreeDigest { .. }
        | Action::WriteArchive { .. }
        | Action::DownloadAndExtract { .. } => {
            run_core_admitted(action, workspace, cache, lease).await
        }
    }
}

async fn run_core_admitted(
    action: &Action,
    workspace: &Path,
    cache: &CacheProvider,
    lease: Option<once_core::ExecutionPermit>,
) -> Result<ActionResult> {
    let result = match lease {
        Some(lease) => {
            once_core::run_uncached_admitted(action, workspace, cache, false, None, lease).await
        }
        None => once_core::run_uncached(action, workspace, cache, false).await,
    };
    match result {
        Ok(result) => Ok(result),
        Err(error) => Err(render_execution_error(error, cache).await),
    }
}

/// Run an uncacheable command that writes no stream to a file. Stdout is
/// discarded rather than buffered; only stderr is retained for failure
/// diagnostics.
async fn run_uncached_discarding_stdout(
    mut command: Command,
    timeout_ms: Option<u64>,
    cache: &CacheProvider,
) -> Result<ActionResult> {
    command.stdout(Stdio::null());
    command.stderr(Stdio::piped());
    let mut group =
        uncached_process::ProcessGroup::spawn(&mut command, "spawning declared action")?;
    let stderr_pipe = group
        .take_stderr()
        .context("capturing declared action stderr")?;
    let wait = group.wait(Some(stderr_pipe), cache);
    let (status, stderr) = match timeout_ms {
        Some(ms) => tokio::time::timeout(Duration::from_millis(ms), wait)
            .await
            .with_context(|| format!("declared action timed out after {ms}ms"))??,
        None => wait.await?,
    };
    Ok(ActionResult {
        exit_code: status.code().unwrap_or(-1),
        stdout: None,
        stderr: Some(stderr.context("capturing declared action stderr")?),
        outputs: BTreeMap::new(),
    })
}

/// Run an uncacheable command whose stdout and/or stderr are redirected
/// into declared output files. Redirected streams go straight to disk and
/// are captured later as ordinary outputs; a non-redirected stderr is
/// still piped and retained for failure diagnostics.
async fn run_uncached_redirected(
    mut command: Command,
    stdout_path: Option<&WorkspacePath>,
    stderr_path: Option<&WorkspacePath>,
    timeout_ms: Option<u64>,
    workspace: &Path,
    cache: &CacheProvider,
) -> Result<ActionResult> {
    let stdout_file = match stdout_path {
        Some(path) => Some(open_redirect_file(path, workspace)?),
        None => None,
    };
    let stderr_file = match stderr_path {
        Some(path) => Some(if stdout_path == Some(path) {
            stdout_file
                .as_ref()
                .expect("stdout redirect open when stderr merges into it")
                .try_clone()
                .with_context(|| format!("cloning redirect handle for `{}`", path.as_str()))?
        } else {
            open_redirect_file(path, workspace)?
        }),
        None => None,
    };
    // A redirected stdout goes to its file; otherwise it is discarded.
    command.stdout(stdout_file.map_or_else(Stdio::null, Stdio::from));
    let capture_stderr = stderr_file.is_none();
    command.stderr(stderr_file.map_or_else(Stdio::piped, Stdio::from));

    let mut group =
        uncached_process::ProcessGroup::spawn(&mut command, "spawning redirected declared action")?;
    let stderr_pipe = group.take_stderr();
    let wait = group.wait(stderr_pipe, cache);
    let (status, stderr_blob) = match timeout_ms {
        Some(ms) => tokio::time::timeout(Duration::from_millis(ms), wait)
            .await
            .with_context(|| format!("declared action timed out after {ms}ms"))??,
        None => wait.await?,
    };
    debug_assert!(capture_stderr == stderr_blob.is_some());
    Ok(ActionResult {
        exit_code: status.code().unwrap_or(-1),
        stdout: None,
        stderr: stderr_blob,
        outputs: BTreeMap::new(),
    })
}

fn open_redirect_file(path: &WorkspacePath, workspace: &Path) -> Result<std::fs::File> {
    let absolute = path.resolve(workspace);
    if let Some(parent) = absolute.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("creating parent directory for redirect `{}`", path.as_str())
        })?;
    }
    std::fs::File::create(&absolute)
        .with_context(|| format!("creating redirect output `{}`", path.as_str()))
}

#[cfg(test)]
pub(super) async fn capture_uncached_outputs(
    outputs: &[WorkspacePath],
    workspace: &Path,
    cache: &CacheProvider,
) -> Result<BTreeMap<String, Digest>> {
    capture_uncached_outputs_admitted(outputs, workspace, cache, None, None).await
}

pub(super) async fn capture_uncached_outputs_admitted(
    outputs: &[WorkspacePath],
    workspace: &Path,
    cache: &CacheProvider,
    lease: Option<&once_core::ExecutionPermit>,
    producers: Option<(&Producers, &str)>,
) -> Result<BTreeMap<String, Digest>> {
    let mut captured = BTreeMap::new();
    for output in outputs {
        let absolute = output.resolve(workspace);
        let metadata = match capture_step(lease, {
            let absolute = absolute.clone();
            move || std::fs::symlink_metadata(absolute)
        })
        .await?
        {
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
        if metadata.file_type().is_symlink() || metadata.is_dir() {
            let fingerprint = capture_step(lease, {
                let absolute = absolute.clone();
                move || legacy_outputs::fingerprint(&absolute)
            })
            .await??
            .with_context(|| {
                format!(
                    "declared output `{}` changed type during capture",
                    output.as_str()
                )
            })?;
            let digest = cache.put_blob(&fingerprint.bytes).await?;
            if let Some((producers, identifier)) = producers {
                producers.record_fingerprint(output.as_str(), digest, fingerprint.kind, identifier);
            }
            captured.insert(output.as_str().to_string(), digest);
            continue;
        }
        let file = capture_step(lease, move || std::fs::File::open(absolute))
            .await?
            .with_context(|| format!("opening declared action output `{}`", output.as_str()))?;
        captured.insert(
            output.as_str().to_string(),
            cache.put_stream(tokio::fs::File::from_std(file)).await?,
        );
    }
    Ok(captured)
}
