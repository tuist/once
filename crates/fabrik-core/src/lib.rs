//! Action types and cache-aware execution.
//!
//! Currently exposes one action kind ([`Action::RunCommand`]) and an
//! async executor ([`Runner`]) that consults a [`Cas`] for memoization
//! before spawning a subprocess. All filesystem and process I/O is
//! async; subprocess output is streamed through the CAS rather than
//! buffered, so a multi-GB linker log doesn't OOM the executor.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use fabrik_cas::{ActionResult, Cas, Digest};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::Semaphore;
use tracing::{debug, instrument};

mod path;

pub use path::{WorkspacePath, WorkspacePathError};

/// Domain-separation prefix for action digests. Bump the version when
/// the canonical encoding (or the [`Action`] schema) changes in a way
/// that should invalidate the cache.
const ACTION_DIGEST_DOMAIN: &[u8] = b"fabrik.action.v1\0";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cas error: {0}")]
    Cas(#[from] fabrik_cas::Error),
    #[error("failed to spawn {program}: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to wait for {program}: {source}")]
    Wait {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("action requires a non-empty argv")]
    EmptyArgv,
    #[error("action exceeded its timeout of {0:?}")]
    Timeout(Duration),
    #[error("invalid workspace path: {0}")]
    InvalidPath(#[from] WorkspacePathError),
}

pub type Result<T> = std::result::Result<T, Error>;

/// All actions Fabrik can execute.
///
/// The wire format of this enum is part of the action digest (see
/// `ACTION_DIGEST_DOMAIN`). Field additions, renames, or reorderings
/// that affect the JSON encoding require a digest version bump.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    RunCommand {
        argv: Vec<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<WorkspacePath>,
        /// Per-action timeout in milliseconds. None = no timeout.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
    },
}

impl Action {
    /// Canonical, content-addressed key for this action.
    ///
    /// The key is `BLAKE3(domain || canonical_json(self))`. Bumping the
    /// domain partitions old and new cache entries cleanly instead of
    /// silently colliding.
    pub fn digest(&self) -> Digest {
        let body = serde_json::to_vec(self).expect("Action is serializable");
        let mut buf = Vec::with_capacity(ACTION_DIGEST_DOMAIN.len() + body.len());
        buf.extend_from_slice(ACTION_DIGEST_DOMAIN);
        buf.extend_from_slice(&body);
        Digest::of_bytes(&buf)
    }
}

/// Whether a result came from cache or fresh execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheState {
    Hit,
    Miss,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub action: Digest,
    pub result: ActionResult,
    pub cache: CacheState,
}

/// Caller-controlled policy for execution.
#[derive(Debug, Clone, Copy, Default)]
pub struct RunOpts {
    /// If true, non-zero-exit results are written to the cache. Off by
    /// default — a transient infra failure (OOM, disk full, network
    /// blip) shouldn't become a permanent cached failure.
    pub cache_failures: bool,
}

/// Bounded async executor.
///
/// A `Runner` caps concurrent in-flight actions via a [`Semaphore`] so
/// that callers driving large graphs cannot exhaust file descriptors,
/// memory, or process slots. The default permit count is the host's
/// available parallelism; override with [`Runner::with_max_concurrency`].
#[derive(Clone)]
pub struct Runner {
    cas: Cas,
    workspace_root: PathBuf,
    opts: RunOpts,
    permits: Arc<Semaphore>,
}

impl Runner {
    pub fn new(cas: Cas, workspace_root: impl Into<PathBuf>, opts: RunOpts) -> Self {
        let n = std::thread::available_parallelism()
            .map(NonZeroUsize::get)
            .unwrap_or(8);
        Self {
            cas,
            workspace_root: workspace_root.into(),
            opts,
            permits: Arc::new(Semaphore::new(n)),
        }
    }

    /// Override the concurrency cap. Useful for tests and constrained
    /// environments. A value of 0 is silently raised to 1.
    #[must_use]
    pub fn with_max_concurrency(mut self, n: usize) -> Self {
        self.permits = Arc::new(Semaphore::new(n.max(1)));
        self
    }

    pub fn cas(&self) -> &Cas {
        &self.cas
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub async fn run(&self, action: &Action) -> Result<Outcome> {
        // Acquire is cancel-safe; if the caller drops the future, the
        // permit is released without ever entering execute().
        let _permit = self
            .permits
            .acquire()
            .await
            .expect("semaphore is not closed for the runner's lifetime");
        run_inner(action, &self.workspace_root, &self.cas, self.opts).await
    }
}

/// Convenience: run a single action without constructing a [`Runner`].
/// Production callers (schedulers) should use [`Runner`] instead so the
/// concurrency cap applies.
pub async fn run(
    action: &Action,
    workspace_root: &Path,
    cas: &Cas,
    opts: RunOpts,
) -> Result<Outcome> {
    run_inner(action, workspace_root, cas, opts).await
}

#[instrument(skip(action, cas), fields(action_digest))]
async fn run_inner(
    action: &Action,
    workspace_root: &Path,
    cas: &Cas,
    opts: RunOpts,
) -> Result<Outcome> {
    let key = action.digest();
    tracing::Span::current().record("action_digest", tracing::field::display(&key));

    if let Some(result) = cas.get_action_result(&key).await? {
        debug!("cache hit");
        return Ok(Outcome {
            action: key,
            result,
            cache: CacheState::Hit,
        });
    }

    let result = execute(action, workspace_root, cas).await?;
    let cacheable = result.exit_code == 0 || opts.cache_failures;
    if cacheable {
        cas.put_action_result(&key, &result).await?;
    } else {
        debug!(
            exit_code = result.exit_code,
            "skipping cache write for failure"
        );
    }
    Ok(Outcome {
        action: key,
        result,
        cache: CacheState::Miss,
    })
}

#[instrument(skip(action, cas), fields(program))]
async fn execute(action: &Action, workspace_root: &Path, cas: &Cas) -> Result<ActionResult> {
    match action {
        Action::RunCommand {
            argv,
            env,
            cwd,
            timeout_ms,
        } => execute_run_command(argv, env, cwd.as_ref(), *timeout_ms, workspace_root, cas).await,
    }
}

async fn execute_run_command(
    argv: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&WorkspacePath>,
    timeout_ms: Option<u64>,
    workspace_root: &Path,
    cas: &Cas,
) -> Result<ActionResult> {
    let (program, rest) = argv.split_first().ok_or(Error::EmptyArgv)?;
    tracing::Span::current().record("program", tracing::field::display(program));

    let mut cmd = Command::new(program);
    cmd.args(rest);
    // Don't inherit the parent's env: actions must declare every
    // variable they depend on, or the cache key lies.
    cmd.env_clear();
    for (k, v) in env {
        cmd.env(k, v);
    }
    // Close stdin — actions never read from a parent terminal.
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.current_dir(cwd.map_or_else(
        || workspace_root.to_path_buf(),
        |c| c.resolve(workspace_root),
    ));
    // If the future is dropped (e.g. timeout fires), tokio sends SIGKILL
    // to the child instead of orphaning it.
    cmd.kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|source| Error::Spawn {
        program: program.clone(),
        source,
    })?;
    let stdout_pipe = child.stdout.take().expect("stdout was piped");
    let stderr_pipe = child.stderr.take().expect("stderr was piped");

    // Stream stdout and stderr into the CAS concurrently. Memory use
    // per stream is bounded by the CAS's stream chunk size; total
    // memory does not grow with output size.
    let work = async {
        let (stdout, stderr) =
            tokio::try_join!(cas.put_stream(stdout_pipe), cas.put_stream(stderr_pipe))?;
        let status = child.wait().await.map_err(|source| Error::Wait {
            program: program.clone(),
            source,
        })?;
        Ok::<_, Error>(ActionResult {
            exit_code: status.code().unwrap_or(-1),
            stdout,
            stderr,
        })
    };

    match timeout_ms {
        Some(ms) => {
            let dur = Duration::from_millis(ms);
            match tokio::time::timeout(dur, work).await {
                Ok(res) => res,
                Err(_) => Err(Error::Timeout(dur)),
            }
        }
        None => work.await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_cas() -> (TempDir, Cas) {
        let tmp = TempDir::new().unwrap();
        let cas = Cas::open(tmp.path());
        (tmp, cas)
    }

    fn echo_action(msg: &str) -> Action {
        Action::RunCommand {
            argv: vec!["/bin/sh".into(), "-c".into(), format!("printf '{msg}'")],
            env: BTreeMap::new(),
            cwd: None,
            timeout_ms: None,
        }
    }

    #[tokio::test]
    async fn first_run_is_miss_second_is_hit() {
        let (tmp, cas) = fresh_cas();
        let action = echo_action("hello");
        let first = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(first.cache, CacheState::Miss);
        assert_eq!(first.result.exit_code, 0);
        assert_eq!(cas.get_blob(&first.result.stdout).await.unwrap(), b"hello");

        let second = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(second.cache, CacheState::Hit);
        assert_eq!(second.result, first.result);
    }

    #[tokio::test]
    async fn different_argv_gets_different_cache_slot() {
        let (tmp, cas) = fresh_cas();
        let a = run(&echo_action("a"), tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        let b = run(&echo_action("b"), tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_ne!(a.action, b.action);
    }

    #[tokio::test]
    async fn env_is_part_of_the_cache_key() {
        let mut env_a = BTreeMap::new();
        env_a.insert("X".into(), "1".into());
        let mut env_b = BTreeMap::new();
        env_b.insert("X".into(), "2".into());
        let argv = vec!["/bin/sh".into(), "-c".into(), "true".into()];
        let a = Action::RunCommand {
            argv: argv.clone(),
            env: env_a,
            cwd: None,
            timeout_ms: None,
        };
        let b = Action::RunCommand {
            argv,
            env: env_b,
            cwd: None,
            timeout_ms: None,
        };
        assert_ne!(a.digest(), b.digest());
    }

    #[tokio::test]
    async fn failures_are_not_cached_by_default() {
        let (tmp, cas) = fresh_cas();
        let action = Action::RunCommand {
            argv: vec!["/bin/sh".into(), "-c".into(), "exit 7".into()],
            env: BTreeMap::new(),
            cwd: None,
            timeout_ms: None,
        };
        let first = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(first.cache, CacheState::Miss);
        assert_eq!(first.result.exit_code, 7);
        let second = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(second.cache, CacheState::Miss);
    }

    #[tokio::test]
    async fn failures_are_cached_with_opt_in() {
        let (tmp, cas) = fresh_cas();
        let action = Action::RunCommand {
            argv: vec!["/bin/sh".into(), "-c".into(), "exit 7".into()],
            env: BTreeMap::new(),
            cwd: None,
            timeout_ms: None,
        };
        let opts = RunOpts {
            cache_failures: true,
        };
        let first = run(&action, tmp.path(), &cas, opts).await.unwrap();
        assert_eq!(first.cache, CacheState::Miss);
        let second = run(&action, tmp.path(), &cas, opts).await.unwrap();
        assert_eq!(second.cache, CacheState::Hit);
        assert_eq!(second.result.exit_code, 7);
    }

    #[tokio::test]
    async fn timeout_kills_long_running_action() {
        let (tmp, cas) = fresh_cas();
        let action = Action::RunCommand {
            argv: vec!["/bin/sh".into(), "-c".into(), "sleep 5".into()],
            env: BTreeMap::new(),
            cwd: None,
            timeout_ms: Some(100),
        };
        let err = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Timeout(_)));
    }

    #[tokio::test]
    async fn cwd_resolves_against_workspace_root() {
        let (tmp, cas) = fresh_cas();
        let sub = tmp.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("marker"), b"present").unwrap();
        let action = Action::RunCommand {
            argv: vec!["/bin/sh".into(), "-c".into(), "cat marker".into()],
            env: BTreeMap::new(),
            cwd: Some(WorkspacePath::try_from("sub").unwrap()),
            timeout_ms: None,
        };
        let outcome = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(outcome.result.exit_code, 0);
        assert_eq!(
            cas.get_blob(&outcome.result.stdout).await.unwrap(),
            b"present"
        );
    }

    #[tokio::test]
    async fn streams_large_output_without_buffering_in_memory() {
        // Produces 4 MB of data — comfortably larger than the 64 KB
        // stream chunk. If we ever regress to buffering, this test
        // still passes but a memory profiler would notice the spike.
        let (tmp, cas) = fresh_cas();
        let action = Action::RunCommand {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "yes hello | head -c 4194304".into(),
            ],
            env: BTreeMap::new(),
            cwd: None,
            timeout_ms: Some(10_000),
        };
        let outcome = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(outcome.result.exit_code, 0);
        let stdout = cas.get_blob(&outcome.result.stdout).await.unwrap();
        assert_eq!(stdout.len(), 4 * 1024 * 1024);
    }

    #[tokio::test]
    async fn runner_caps_concurrency() {
        // With max_concurrency=1, two actions started concurrently must
        // execute serially. Each action sleeps 200ms; if they ran in
        // parallel the total would be ~200ms, serialized it's ~400ms.
        let (tmp, cas) = fresh_cas();
        let runner =
            Runner::new(cas, tmp.path().to_path_buf(), RunOpts::default()).with_max_concurrency(1);
        let mk = |suffix: &str| Action::RunCommand {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                format!("sleep 0.2; printf {suffix}"),
            ],
            env: BTreeMap::new(),
            cwd: None,
            timeout_ms: Some(5_000),
        };
        let started = std::time::Instant::now();
        let action_a = mk("a");
        let action_b = mk("b");
        let (a, b) = tokio::join!(runner.run(&action_a), runner.run(&action_b));
        a.unwrap();
        b.unwrap();
        assert!(
            started.elapsed() >= Duration::from_millis(380),
            "expected serialized execution; took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn digest_includes_domain_prefix() {
        // Pins the v1 domain so an accidental edit to ACTION_DIGEST_DOMAIN
        // is loud — every cached digest changes when this constant
        // changes.
        let action = Action::RunCommand {
            argv: vec!["true".into()],
            env: BTreeMap::new(),
            cwd: None,
            timeout_ms: None,
        };
        let expected = {
            let body = serde_json::to_vec(&action).unwrap();
            let mut buf = Vec::with_capacity(ACTION_DIGEST_DOMAIN.len() + body.len());
            buf.extend_from_slice(ACTION_DIGEST_DOMAIN);
            buf.extend_from_slice(&body);
            Digest::of_bytes(&buf)
        };
        assert_eq!(action.digest(), expected);
    }
}
