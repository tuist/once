use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use once_cas::{ActionResult, CacheProvider};
use tokio::process::Command;
use tracing::debug;

use crate::stream::{self, Destination};
use crate::{Error, Result, WorkspacePath};

pub(crate) async fn execute_command(
    argv: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&WorkspacePath>,
    timeout_ms: Option<u64>,
    workspace_root: &Path,
    cache: &CacheProvider,
) -> Result<ActionResult> {
    let (program, rest) = argv.split_first().ok_or(Error::EmptyArgv)?;
    tracing::Span::current().record("program", tracing::field::display(program));

    let mut command = Command::new(program);
    command.args(rest);
    command.env_clear();
    for (k, v) in env {
        command.env(k, v);
    }
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
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
    let stdout_pipe = child.stdout.take().expect("stdout was piped");
    let stderr_pipe = child.stderr.take().expect("stderr was piped");

    let work = async {
        let (stdout, stderr) =
            tokio::try_join!(cache.put_stream(stdout_pipe), cache.put_stream(stderr_pipe))?;
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
            stdout: Some(stdout),
            stderr: Some(stderr),
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
) -> Result<ActionResult> {
    Box::pin(execute_child_streaming(
        argv,
        env,
        cwd,
        timeout_ms,
        workspace_root,
        cache,
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
    stream_to_parent: bool,
    inherit_parent_env: bool,
) -> Result<ActionResult> {
    let (program, rest) = argv.split_first().ok_or(Error::EmptyArgv)?;
    tracing::Span::current().record("program", tracing::field::display(program));

    let mut command = Command::new(program);
    command.args(rest);
    if !inherit_parent_env {
        command.env_clear();
    }
    for (k, v) in env {
        command.env(k, v);
    }
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
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
    let stdout_pipe = child.stdout.take().expect("stdout was piped");
    let stderr_pipe = child.stderr.take().expect("stderr was piped");

    let work = Box::pin(async {
        let (stdout, stderr) = tokio::try_join!(
            stream::to_cache(stdout_pipe, Destination::Stdout, cache, stream_to_parent),
            stream::to_cache(stderr_pipe, Destination::Stderr, cache, stream_to_parent)
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
            stdout: Some(stdout),
            stderr: Some(stderr),
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
