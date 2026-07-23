use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use once_cas::{ActionResult, CacheProvider, Digest};
use tokio::io::AsyncRead;
use tokio::process::Command;
use tracing::debug;

use crate::stream::{self, Destination};
use crate::{resolve_execution_argv, resolve_execution_env, Error, Result, WorkspacePath};

/// Optional per-stream file redirection for a command. When a stream is
/// redirected, the child writes directly to the workspace-relative file
/// (an ordinary declared output) instead of the stream being captured
/// into the CAS. When both point at the same path the two streams share
/// one file handle, reproducing shell `2>&1`.
#[derive(Clone, Copy, Default)]
pub(crate) struct Redirect<'a> {
    pub stdout: Option<&'a WorkspacePath>,
    pub stderr: Option<&'a WorkspacePath>,
}

fn open_redirect_file(path: &WorkspacePath, workspace_root: &Path) -> Result<std::fs::File> {
    let absolute = path.resolve(workspace_root);
    if let Some(parent) = absolute.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::FileAction {
            action: "create_parent_dir",
            path: path.as_str().to_string(),
            source,
        })?;
    }
    std::fs::File::create(&absolute).map_err(|source| Error::FileAction {
        action: "redirect_output",
        path: path.as_str().to_string(),
        source,
    })
}

/// Point the command's stdout/stderr at redirect files where requested,
/// leaving unredirected streams piped so they can be captured into the
/// CAS. Streams sharing a destination path share a single file handle.
fn apply_redirect(command: &mut Command, redirect: Redirect, workspace_root: &Path) -> Result<()> {
    let stdout_file = match redirect.stdout {
        Some(path) => Some(open_redirect_file(path, workspace_root)?),
        None => None,
    };
    let stderr_file = match redirect.stderr {
        Some(path) => Some(if redirect.stdout == Some(path) {
            stdout_file
                .as_ref()
                .expect("stdout redirect open when stderr merges into it")
                .try_clone()
                .map_err(|source| Error::FileAction {
                    action: "redirect_output",
                    path: path.as_str().to_string(),
                    source,
                })?
        } else {
            open_redirect_file(path, workspace_root)?
        }),
        None => None,
    };
    command.stdout(stdout_file.map_or_else(Stdio::piped, Stdio::from));
    command.stderr(stderr_file.map_or_else(Stdio::piped, Stdio::from));
    Ok(())
}

/// Capture a piped stream into the CAS, or resolve to `None` when the
/// stream was redirected to a file (and therefore not piped).
async fn capture_stream<R: AsyncRead + Unpin>(
    cache: &CacheProvider,
    pipe: Option<R>,
) -> Result<Option<Digest>> {
    match pipe {
        Some(pipe) => Ok(Some(cache.put_stream(pipe).await?)),
        None => Ok(None),
    }
}

/// Streaming counterpart of [`capture_stream`]: tee a piped stream to the
/// parent while capturing it, or resolve to `None` when redirected.
async fn capture_stream_streaming<R: AsyncRead + Unpin>(
    pipe: Option<R>,
    destination: Destination,
    cache: &CacheProvider,
    stream_to_parent: bool,
) -> Result<Option<Digest>> {
    match pipe {
        Some(pipe) => Ok(Some(
            stream::to_cache(pipe, destination, cache, stream_to_parent).await?,
        )),
        None => Ok(None),
    }
}

pub(crate) async fn execute_command(
    argv: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&WorkspacePath>,
    timeout_ms: Option<u64>,
    workspace_root: &Path,
    cache: &CacheProvider,
    redirect: Redirect<'_>,
) -> Result<ActionResult> {
    let argv = resolve_execution_argv(argv, workspace_root);
    let env = resolve_execution_env(env, workspace_root);
    let (program, rest) = argv.split_first().ok_or(Error::EmptyArgv)?;
    tracing::Span::current().record("program", tracing::field::display(program));

    let mut command = Command::new(program);
    command.args(rest);
    command.env_clear();
    for (k, v) in &env {
        command.env(k, v);
    }
    command.stdin(Stdio::null());
    apply_redirect(&mut command, redirect, workspace_root)?;
    let command_cwd = cwd.map_or_else(
        || workspace_root.to_path_buf(),
        |c| c.resolve(workspace_root),
    );
    command.current_dir(&command_cwd);
    command.kill_on_drop(true);
    debug!(
        program = %program,
        arg_count = rest.len(),
        env_count = env.len(),
        cwd = %command_cwd.display(),
        timeout_ms,
        "spawning local command"
    );

    let mut child = command.spawn().map_err(|source| Error::Spawn {
        program: program.clone(),
        source,
    })?;
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let work = async {
        let (stdout, stderr) = tokio::try_join!(
            capture_stream(cache, stdout_pipe),
            capture_stream(cache, stderr_pipe)
        )?;
        let status = child.wait().await.map_err(|source| Error::Wait {
            program: program.clone(),
            source,
        })?;
        let exit_code = status.code().unwrap_or(-1);
        debug!(
            program = %program,
            exit_code,
            "local command finished"
        );
        Ok::<_, Error>(ActionResult {
            exit_code,
            stdout,
            stderr,
            outputs: BTreeMap::new(),
        })
    };

    Box::pin(with_timeout(program, timeout_ms, work)).await
}

pub(crate) async fn execute_command_streaming(
    argv: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&WorkspacePath>,
    timeout_ms: Option<u64>,
    workspace_root: &Path,
    cache: &CacheProvider,
    redirect: Redirect<'_>,
) -> Result<ActionResult> {
    Box::pin(execute_child_streaming(
        argv,
        env,
        cwd,
        timeout_ms,
        workspace_root,
        cache,
        redirect,
        true,
        false,
    ))
    .await
}

#[allow(clippy::too_many_arguments)]
async fn execute_child_streaming(
    argv: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&WorkspacePath>,
    timeout_ms: Option<u64>,
    workspace_root: &Path,
    cache: &CacheProvider,
    redirect: Redirect<'_>,
    stream_to_parent: bool,
    inherit_parent_env: bool,
) -> Result<ActionResult> {
    let argv = resolve_execution_argv(argv, workspace_root);
    let env = resolve_execution_env(env, workspace_root);
    let (program, rest) = argv.split_first().ok_or(Error::EmptyArgv)?;
    tracing::Span::current().record("program", tracing::field::display(program));

    let mut command = Command::new(program);
    command.args(rest);
    if !inherit_parent_env {
        command.env_clear();
    }
    for (k, v) in &env {
        command.env(k, v);
    }
    command.stdin(Stdio::null());
    apply_redirect(&mut command, redirect, workspace_root)?;
    let command_cwd = cwd.map_or_else(
        || workspace_root.to_path_buf(),
        |c| c.resolve(workspace_root),
    );
    command.current_dir(&command_cwd);
    command.kill_on_drop(true);
    debug!(
        program = %program,
        arg_count = rest.len(),
        env_count = env.len(),
        cwd = %command_cwd.display(),
        timeout_ms,
        stream_to_parent,
        inherit_parent_env,
        "spawning local command"
    );

    let mut child = command.spawn().map_err(|source| Error::Spawn {
        program: program.clone(),
        source,
    })?;
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let work = Box::pin(async {
        let (stdout, stderr) = tokio::try_join!(
            capture_stream_streaming(stdout_pipe, Destination::Stdout, cache, stream_to_parent),
            capture_stream_streaming(stderr_pipe, Destination::Stderr, cache, stream_to_parent)
        )?;
        let status = child.wait().await.map_err(|source| Error::Wait {
            program: program.clone(),
            source,
        })?;
        let exit_code = status.code().unwrap_or(-1);
        debug!(
            program = %program,
            exit_code,
            "local command finished"
        );
        Ok::<_, Error>(ActionResult {
            exit_code,
            stdout,
            stderr,
            outputs: BTreeMap::new(),
        })
    });

    Box::pin(with_timeout(program, timeout_ms, work)).await
}

async fn with_timeout<T>(
    program: &str,
    timeout_ms: Option<u64>,
    work: impl Future<Output = Result<T>>,
) -> Result<T> {
    let Some(ms) = timeout_ms else {
        return work.await;
    };
    let dur = Duration::from_millis(ms);
    if let Ok(res) = tokio::time::timeout(dur, work).await {
        res
    } else {
        debug!(
            program = %program,
            timeout_ms = ms,
            "local command timed out"
        );
        Err(Error::Timeout(dur))
    }
}
