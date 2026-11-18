//! Native execution of command actions on the host.
//!
//! Every action runs as the leader of its own process group so a timeout or a
//! cancelled run stops everything the action started, and output pipes that
//! outlive the leader are drained for a bounded time only.

mod capture;
mod isolation;
pub(crate) mod process_group;
mod supervise;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;

use struo_cas::{ActionResult, CacheProvider};
use tokio::process::Command;
use tracing::debug;

use self::process_group::GroupChild;
use crate::stream::ActionOutputObserver;
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
#[derive(Clone, Copy)]
enum Capture<'a> {
    /// Drain each pipe into the CAS and nowhere else.
    Cached,
    /// Mirror each pipe to this process's own stdout/stderr while
    /// capturing it into the CAS.
    Streaming,
    /// Report each chunk to a caller while retaining it in the CAS.
    Observed {
        stream_to_parent: bool,
        observer: &'a dyn ActionOutputObserver,
    },
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

pub(crate) async fn execute_command_observed(
    invocation: Invocation<'_>,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
    observer: &dyn ActionOutputObserver,
) -> Result<ActionResult> {
    Box::pin(spawn_and_capture(
        invocation,
        workspace_root,
        cache,
        Capture::Observed {
            stream_to_parent,
            observer,
        },
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
    capture: Capture<'_>,
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

    let mut command = isolation::command(network, program, rest)?;
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
    debug!(
        program = %program,
        arg_count = rest.len(),
        env_count = env.len(),
        cwd = %command_cwd.display(),
        timeout_ms,
        network_denied = network.is_denied(),
        "spawning local command"
    );

    let mut group = GroupChild::spawn(&mut command).map_err(|source| Error::Spawn {
        program: program.clone(),
        source,
    })?;
    let stdout_pipe = group.take_stdout();
    let stderr_pipe = group.take_stderr();
    let pipes = capture::capture_pipes(capture, cache, stdout_pipe, stderr_pipe);
    let (status, (stdout, stderr)) =
        Box::pin(supervise::supervise(group, program, timeout_ms, pipes)).await?;
    let exit_code = status.code().unwrap_or(-1);
    debug!(
        program = %program,
        exit_code,
        "local command finished"
    );
    Ok(ActionResult {
        exit_code,
        stdout,
        stderr,
        outputs: BTreeMap::new(),
    })
}
