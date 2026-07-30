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
use crate::{
    resolve_execution_argv, resolve_execution_env, Error, NetworkPolicy, Result, WorkspacePath,
};

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

/// One local child process: what to run, where, and how its streams are
/// wired.
///
/// These five values travel together through every local execution path
/// and through the sandboxed path above it. Grouping them keeps the entry
/// points at three arguments instead of eight, and keeps the meaning of
/// each value at the call site attached to a field name.
#[derive(Clone, Copy)]
pub(crate) struct Invocation<'a> {
    pub argv: &'a [String],
    pub env: &'a BTreeMap<String, String>,
    pub cwd: Option<&'a WorkspacePath>,
    pub timeout_ms: Option<u64>,
    pub redirect: Redirect<'a>,
    pub network: NetworkPolicy,
}

/// How a child's stdout and stderr are consumed.
#[derive(Clone, Copy, Debug)]
enum Capture {
    /// Drain each pipe into the CAS and nowhere else.
    Cached,
    /// Mirror each pipe to this process's own stdout/stderr while
    /// capturing it into the CAS.
    Streaming,
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
) -> Result<Option<Digest>> {
    match pipe {
        Some(pipe) => Ok(Some(
            stream::to_cache(pipe, destination, cache, true).await?,
        )),
        None => Ok(None),
    }
}

pub(crate) async fn execute_command(
    invocation: Invocation<'_>,
    workspace_root: &Path,
    cache: &CacheProvider,
) -> Result<ActionResult> {
    Box::pin(spawn_and_capture(
        invocation,
        workspace_root,
        cache,
        Capture::Cached,
    ))
    .await
}

pub(crate) async fn execute_command_streaming(
    invocation: Invocation<'_>,
    workspace_root: &Path,
    cache: &CacheProvider,
) -> Result<ActionResult> {
    Box::pin(spawn_and_capture(
        invocation,
        workspace_root,
        cache,
        Capture::Streaming,
    ))
    .await
}

/// Spawn one child under the workspace and collect its result.
///
/// The cached and streaming paths differ only in how the two pipes are
/// drained, so they share this body; keeping them as separate copies let
/// the spawn setup drift between them.
async fn spawn_and_capture(
    invocation: Invocation<'_>,
    workspace_root: &Path,
    cache: &CacheProvider,
    capture: Capture,
) -> Result<ActionResult> {
    let Invocation {
        argv,
        env,
        cwd,
        timeout_ms,
        redirect,
        network,
    } = invocation;
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
    // Isolate the child from the network when the action declared `deny`.
    // On Linux a seccomp filter installed between fork and exec turns every
    // network syscall into `EACCES`. Other platforms accept the declaration
    // but cannot enforce it; warn so the gap is visible rather than silent.
    #[cfg(target_os = "linux")]
    if network.is_denied() {
        crate::network::arm(&mut command);
    }
    #[cfg(not(target_os = "linux"))]
    if network.is_denied() {
        tracing::warn!(
            program = %program,
            "network `deny` requested but not enforced on this platform; the action may still reach the network",
        );
    }
    debug!(
        program = %program,
        arg_count = rest.len(),
        env_count = env.len(),
        cwd = %command_cwd.display(),
        timeout_ms,
        ?capture,
        "spawning local command"
    );

    let mut child = command.spawn().map_err(|source| Error::Spawn {
        program: program.clone(),
        source,
    })?;
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let work = Box::pin(async {
        let (stdout, stderr) = match capture {
            Capture::Cached => tokio::try_join!(
                capture_stream(cache, stdout_pipe),
                capture_stream(cache, stderr_pipe)
            )?,
            Capture::Streaming => tokio::try_join!(
                capture_stream_streaming(stdout_pipe, Destination::Stdout, cache),
                capture_stream_streaming(stderr_pipe, Destination::Stderr, cache)
            )?,
        };
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
