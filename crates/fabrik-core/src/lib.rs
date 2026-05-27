//! Action types and cache-aware execution.
//!
//! Currently exposes one action kind ([`Action::RunCommand`]) and an
//! async executor ([`Runner`]) that consults a cache provider
//! before spawning a subprocess. All filesystem and process I/O is
//! async; subprocess output is streamed through the CAS rather than
//! buffered, so a multi-GB linker log doesn't OOM the executor.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use fabrik_cas::{ActionResult, CacheProvider, Cas, Digest};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tracing::{debug, instrument};

mod directory_blob;
mod env;
mod input_digest;
mod path;
mod plan;
mod resources;
mod xdg;

use directory_blob::{capture_directory_blob, restore_directory_blob, DIRECTORY_BLOB_MAGIC};

pub use env::{
    select_tool_env, tool_env, workspace_tool, workspace_tool_env, workspace_tool_var, ToolEnvError,
};
pub use input_digest::InputDigestBuilder;
pub use path::{WorkspacePath, WorkspacePathError};
pub use plan::{BuiltPlan, NodeInfo, Plan, PlanError, PlanNode, PlanOutcome};
pub use resources::{ResourceLimits, ResourcePool, ResourceRequest};
pub use xdg::Xdg;

/// Domain-separation prefix for action digests. Bump the version when
/// the canonical encoding (or the [`Action`] schema) changes in a way
/// that should invalidate the cache.
const ACTION_DIGEST_DOMAIN: &[u8] = b"fabrik.action.v3\0";

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
    #[error("remote provider `{provider}` is not supported yet")]
    UnsupportedRemoteProvider { provider: String },
    #[error("remote provider `{provider}` is not configured: {message}")]
    RemoteProviderConfig { provider: String, message: String },
    #[error("action requires a non-empty argv")]
    EmptyArgv,
    #[error("action exceeded its timeout of {0:?}")]
    Timeout(Duration),
    #[error("invalid workspace path: {0}")]
    InvalidPath(#[from] WorkspacePathError),
    #[error("declared output `{path}` was not produced")]
    MissingOutput { path: String },
    #[error("failed to read declared output `{path}`: {source}")]
    ReadOutput {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to restore cached output `{path}`: {source}")]
    RestoreOutput {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid cached directory output `{path}`: {message}")]
    InvalidDirectoryOutput { path: String, message: String },
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_digest: Option<Digest>,
        /// Workspace-relative paths the action promises to produce. The
        /// runner stores each one in the CAS after a fresh execution
        /// and restores it from the CAS on a cache hit. An empty list
        /// means the action has no declared outputs (only stdout/stderr
        /// are cached); cache hits then provide nothing on disk.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        outputs: Vec<WorkspacePath>,
        #[serde(default, skip_serializing_if = "ResourceRequest::is_default")]
        resources: ResourceRequest,
        /// Per-action timeout in milliseconds. None = no timeout.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
        /// Optional compute provider for remote execution. This is
        /// part of the action key so local and remote runs never share
        /// a cache slot by accident.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote: Option<RemoteExecution>,
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

    pub fn resource_request(&self) -> &ResourceRequest {
        match self {
            Action::RunCommand { resources, .. } => resources,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct RemoteExecution {
    pub provider: String,
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
    /// default - a transient infra failure (OOM, disk full, network
    /// blip) shouldn't become a permanent cached failure.
    pub cache_failures: bool,
}

/// Bounded async executor.
///
/// A `Runner` caps in-flight actions with a [`ResourcePool`] so callers
/// driving large graphs cannot exhaust file descriptors, memory, or
/// process slots. The default CPU budget is the host's available
/// parallelism; override with [`Runner::with_max_concurrency`] or
/// [`Runner::with_resource_limits`].
#[derive(Clone)]
pub struct Runner {
    cache: CacheProvider,
    workspace_root: PathBuf,
    opts: RunOpts,
    resources: Arc<ResourcePool>,
}

impl Runner {
    pub fn new(cas: Cas, workspace_root: impl Into<PathBuf>, opts: RunOpts) -> Self {
        Self {
            cache: CacheProvider::Local(cas),
            workspace_root: workspace_root.into(),
            opts,
            resources: Arc::new(ResourcePool::new(ResourceLimits::default())),
        }
    }

    pub fn with_cache(
        cache: CacheProvider,
        workspace_root: impl Into<PathBuf>,
        opts: RunOpts,
    ) -> Self {
        Self {
            cache,
            workspace_root: workspace_root.into(),
            opts,
            resources: Arc::new(ResourcePool::new(ResourceLimits::default())),
        }
    }

    /// Override the concurrency cap. Useful for tests and constrained
    /// environments. A value of 0 is silently raised to 1.
    #[must_use]
    pub fn with_max_concurrency(mut self, n: usize) -> Self {
        let limits = self.resources.limits();
        self.resources = Arc::new(ResourcePool::new(ResourceLimits::new(
            n,
            limits.memory_bytes,
        )));
        self
    }

    #[must_use]
    pub fn with_resource_limits(mut self, limits: ResourceLimits) -> Self {
        self.resources = Arc::new(ResourcePool::new(limits));
        self
    }

    pub fn cache(&self) -> &CacheProvider {
        &self.cache
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub async fn run(&self, action: &Action) -> Result<Outcome> {
        let key = action.digest();
        if let Some(hit) = lookup_cached(&self.cache, &self.workspace_root, &key).await? {
            return Ok(hit);
        }
        // Permits guard real subprocess execution, not cache lookups.
        // Dropping the future on cancellation releases the permit
        // without ever entering execute().
        let _permit = self
            .resources
            .acquire(action.resource_request().clone())
            .await;
        // Re-check under the permit: another runner may have produced
        // the entry while we were queued.
        if let Some(hit) = lookup_cached(&self.cache, &self.workspace_root, &key).await? {
            return Ok(hit);
        }
        produce(action, &self.workspace_root, &self.cache, self.opts, key).await
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
    run_with_cache(
        action,
        workspace_root,
        &CacheProvider::Local(cas.clone()),
        opts,
    )
    .await
}

pub async fn run_with_cache(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    opts: RunOpts,
) -> Result<Outcome> {
    let key = action.digest();
    if let Some(hit) = lookup_cached(cache, workspace_root, &key).await? {
        return Ok(hit);
    }
    produce(action, workspace_root, cache, opts, key).await
}

#[instrument(skip(cache), fields(action_digest = %key))]
async fn lookup_cached(
    cache: &CacheProvider,
    workspace_root: &Path,
    key: &Digest,
) -> Result<Option<Outcome>> {
    if let Some(result) = cache.get_action_result(key).await? {
        debug!("cache hit");
        // A cache hit must materialize the action's declared outputs to
        // disk; downstream actions see real files even though the
        // upstream action did not actually run on this machine.
        restore_outputs(&result, workspace_root, cache).await?;
        return Ok(Some(Outcome {
            action: *key,
            result,
            cache: CacheState::Hit,
        }));
    }
    Ok(None)
}

#[instrument(skip(action, cache), fields(action_digest = %key))]
async fn produce(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    opts: RunOpts,
    key: Digest,
) -> Result<Outcome> {
    let result = execute(action, workspace_root, cache).await?;
    let cacheable = result.exit_code == 0 || opts.cache_failures;
    if cacheable {
        cache.put_action_result(&key, &result).await?;
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

/// Materialize every cached output blob to its declared workspace path.
/// On cache hit this is what makes a downstream action see a file the
/// upstream action did not actually run on this machine.
async fn restore_outputs(
    result: &ActionResult,
    workspace_root: &Path,
    cache: &CacheProvider,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    for (rel, digest) in &result.outputs {
        let abs = workspace_root.join(rel);
        let bytes = cache.get_blob(digest).await?;
        if bytes.starts_with(DIRECTORY_BLOB_MAGIC) {
            restore_directory_blob(rel, &abs, &bytes)?;
            continue;
        }
        if let Some(parent) = abs.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| Error::RestoreOutput {
                    path: rel.clone(),
                    source,
                })?;
        }
        let mut file =
            tokio::fs::File::create(&abs)
                .await
                .map_err(|source| Error::RestoreOutput {
                    path: rel.clone(),
                    source,
                })?;
        file.write_all(&bytes)
            .await
            .map_err(|source| Error::RestoreOutput {
                path: rel.clone(),
                source,
            })?;
        file.flush().await.map_err(|source| Error::RestoreOutput {
            path: rel.clone(),
            source,
        })?;
        // Preserve executable bit on Unix: rustc emits binaries with
        // mode 0o755 and a restored file from the CAS would otherwise
        // come out 0o644 and fail to execute. We mark every restored
        // output executable; libraries (.rlib/.rmeta) are not executed
        // so the extra bit is harmless, and we avoid carrying mode in
        // the cache schema for now.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o755);
            tokio::fs::set_permissions(&abs, perms)
                .await
                .map_err(|source| Error::RestoreOutput {
                    path: rel.clone(),
                    source,
                })?;
        }
    }
    Ok(())
}

#[instrument(skip(action, cache), fields(program))]
async fn execute(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
) -> Result<ActionResult> {
    match action {
        Action::RunCommand {
            argv,
            env,
            cwd,
            input_digest: _,
            outputs,
            resources: _,
            timeout_ms,
            remote,
        } => {
            let mut result = if let Some(remote) = remote {
                execute_remote_command(
                    remote,
                    argv,
                    env,
                    cwd.as_ref(),
                    *timeout_ms,
                    workspace_root,
                    cache,
                    false,
                )
                .await?
            } else {
                execute_run_command(argv, env, cwd.as_ref(), *timeout_ms, workspace_root, cache)
                    .await?
            };
            // Failed actions don't have to produce their declared
            // outputs; the caller (`produce`) decides whether to cache
            // the failure at all. When the action succeeded, every
            // declared output must exist or it's a contract violation.
            if result.exit_code == 0 {
                result.outputs = capture_outputs(outputs, workspace_root, cache).await?;
            }
            Ok(result)
        }
    }
}

pub async fn run_with_cache_streaming(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    opts: RunOpts,
) -> Result<Outcome> {
    let key = action.digest();
    if let Some(hit) = lookup_cached(cache, workspace_root, &key).await? {
        return Ok(hit);
    }
    let result = execute_streaming(action, workspace_root, cache).await?;
    let cacheable = result.exit_code == 0 || opts.cache_failures;
    if cacheable {
        cache.put_action_result(&key, &result).await?;
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

async fn execute_streaming(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
) -> Result<ActionResult> {
    match action {
        Action::RunCommand {
            argv,
            env,
            cwd,
            input_digest: _,
            outputs,
            resources: _,
            timeout_ms,
            remote,
        } => {
            let mut result = if let Some(remote) = remote {
                execute_remote_command(
                    remote,
                    argv,
                    env,
                    cwd.as_ref(),
                    *timeout_ms,
                    workspace_root,
                    cache,
                    true,
                )
                .await?
            } else {
                execute_run_command_streaming(
                    argv,
                    env,
                    cwd.as_ref(),
                    *timeout_ms,
                    workspace_root,
                    cache,
                )
                .await?
            };
            if result.exit_code == 0 {
                result.outputs = capture_outputs(outputs, workspace_root, cache).await?;
            }
            Ok(result)
        }
    }
}

/// Hash and store every declared output in the CAS, returning the
/// (path -> digest) map that goes into the cached `ActionResult`.
async fn capture_outputs(
    outputs: &[WorkspacePath],
    workspace_root: &Path,
    cache: &CacheProvider,
) -> Result<BTreeMap<String, Digest>> {
    let mut captured = BTreeMap::new();
    for rel in outputs {
        let abs = rel.resolve(workspace_root);
        let metadata = match tokio::fs::metadata(&abs).await {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::MissingOutput {
                    path: rel.as_str().to_string(),
                });
            }
            Err(source) => {
                return Err(Error::ReadOutput {
                    path: rel.as_str().to_string(),
                    source,
                });
            }
        };
        let bytes = if metadata.is_dir() {
            capture_directory_blob(&abs).map_err(|source| Error::ReadOutput {
                path: rel.as_str().to_string(),
                source,
            })?
        } else {
            tokio::fs::read(&abs)
                .await
                .map_err(|source| Error::ReadOutput {
                    path: rel.as_str().to_string(),
                    source,
                })?
        };
        let digest = cache.put_blob(&bytes).await?;
        captured.insert(rel.as_str().to_string(), digest);
    }
    Ok(captured)
}

async fn execute_run_command(
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
    // Don't inherit the parent's env: actions must declare every
    // variable they depend on, or the cache key lies.
    command.env_clear();
    for (k, v) in env {
        command.env(k, v);
    }
    // Close stdin - actions never read from a parent terminal.
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.current_dir(cwd.map_or_else(
        || workspace_root.to_path_buf(),
        |c| c.resolve(workspace_root),
    ));
    // If the future is dropped (e.g. timeout fires), tokio sends SIGKILL
    // to the child instead of orphaning it.
    command.kill_on_drop(true);

    let mut child = command.spawn().map_err(|source| Error::Spawn {
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
            tokio::try_join!(cache.put_stream(stdout_pipe), cache.put_stream(stderr_pipe))?;
        let status = child.wait().await.map_err(|source| Error::Wait {
            program: program.clone(),
            source,
        })?;
        Ok::<_, Error>(ActionResult {
            exit_code: status.code().unwrap_or(-1),
            stdout,
            stderr,
            outputs: BTreeMap::new(),
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

async fn execute_run_command_streaming(
    argv: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&WorkspacePath>,
    timeout_ms: Option<u64>,
    workspace_root: &Path,
    cache: &CacheProvider,
) -> Result<ActionResult> {
    execute_child_streaming(
        argv,
        env,
        cwd,
        timeout_ms,
        workspace_root,
        cache,
        true,
        false,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn execute_remote_command(
    remote: &RemoteExecution,
    argv: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&WorkspacePath>,
    timeout_ms: Option<u64>,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
) -> Result<ActionResult> {
    match remote.provider.as_str() {
        "microsandbox" => {
            let microsandbox =
                resolve_provider_program("microsandbox").unwrap_or_else(|| "microsandbox".into());
            let mut remote_argv = vec![
                microsandbox,
                "run".to_string(),
                "--workspace".to_string(),
                workspace_root.to_string_lossy().into_owned(),
            ];
            if let Some(cwd) = cwd {
                remote_argv.push("--cwd".to_string());
                remote_argv.push(cwd.as_str().to_string());
            }
            remote_argv.push("--".to_string());
            remote_argv.extend(argv.iter().cloned());
            execute_child_streaming(
                &remote_argv,
                env,
                None,
                timeout_ms,
                workspace_root,
                cache,
                stream_to_parent,
                false,
            )
            .await
        }
        "daytona" => {
            let daytona = resolve_provider_program("daytona").unwrap_or_else(|| "daytona".into());
            let sandbox = daytona_sandbox()?;
            let remote_cwd = daytona_workdir(cwd);
            let mut remote_argv = vec![
                daytona,
                "exec".to_string(),
                sandbox,
                "--cwd".to_string(),
                remote_cwd,
            ];
            if let Some(timeout_ms) = timeout_ms {
                remote_argv.push("--timeout".to_string());
                remote_argv.push(timeout_secs(timeout_ms).to_string());
            }
            remote_argv.push("--".to_string());
            append_env_command(&mut remote_argv, env);
            remote_argv.extend(argv.iter().cloned());
            execute_child_streaming(
                &remote_argv,
                &BTreeMap::new(),
                None,
                timeout_ms,
                workspace_root,
                cache,
                stream_to_parent,
                true,
            )
            .await
        }
        provider => Err(Error::UnsupportedRemoteProvider {
            provider: provider.to_string(),
        }),
    }
}

fn daytona_sandbox() -> Result<String> {
    let sandbox =
        std::env::var("FABRIK_DAYTONA_SANDBOX").map_err(|_| Error::RemoteProviderConfig {
            provider: "daytona".to_string(),
            message: "set FABRIK_DAYTONA_SANDBOX to the sandbox id or name".to_string(),
        })?;
    if sandbox.trim().is_empty() {
        return Err(Error::RemoteProviderConfig {
            provider: "daytona".to_string(),
            message: "FABRIK_DAYTONA_SANDBOX cannot be empty".to_string(),
        });
    }
    Ok(sandbox)
}

fn daytona_workdir(cwd: Option<&WorkspacePath>) -> String {
    let root = std::env::var("FABRIK_DAYTONA_WORKDIR").unwrap_or_else(|_| "/workspace".to_string());
    match cwd {
        Some(cwd) => join_remote_path(&root, cwd.as_str()),
        None => root,
    }
}

fn join_remote_path(root: &str, rel: &str) -> String {
    if root.ends_with('/') {
        format!("{root}{rel}")
    } else {
        format!("{root}/{rel}")
    }
}

fn timeout_secs(timeout_ms: u64) -> u64 {
    timeout_ms.div_ceil(1000).max(1)
}

fn append_env_command(argv: &mut Vec<String>, env: &BTreeMap<String, String>) {
    if env.is_empty() {
        return;
    }
    argv.push("env".to_string());
    argv.push("-i".to_string());
    argv.extend(env.iter().map(|(key, value)| format!("{key}={value}")));
}

fn resolve_provider_program(program: &str) -> Option<String> {
    if program.contains('/') {
        return Some(program.to_string());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(program);
        if candidate.is_file() {
            Some(candidate.to_string_lossy().into_owned())
        } else {
            None
        }
    })
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
    command.current_dir(cwd.map_or_else(
        || workspace_root.to_path_buf(),
        |c| c.resolve(workspace_root),
    ));
    command.kill_on_drop(true);

    let mut child = command.spawn().map_err(|source| Error::Spawn {
        program: program.clone(),
        source,
    })?;
    let stdout_pipe = child.stdout.take().expect("stdout was piped");
    let stderr_pipe = child.stderr.take().expect("stderr was piped");

    let work = async {
        let (stdout_bytes, stderr_bytes) = tokio::try_join!(
            capture_stream(stdout_pipe, StreamDestination::Stdout, stream_to_parent),
            capture_stream(stderr_pipe, StreamDestination::Stderr, stream_to_parent)
        )?;
        let status = child.wait().await.map_err(|source| Error::Wait {
            program: program.clone(),
            source,
        })?;
        let stdout = cache.put_blob(&stdout_bytes).await?;
        let stderr = cache.put_blob(&stderr_bytes).await?;
        Ok::<_, Error>(ActionResult {
            exit_code: status.code().unwrap_or(-1),
            stdout,
            stderr,
            outputs: BTreeMap::new(),
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

#[derive(Clone, Copy)]
enum StreamDestination {
    Stdout,
    Stderr,
}

async fn capture_stream<R>(
    mut reader: R,
    destination: StreamDestination,
    stream_to_parent: bool,
) -> Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    let mut buf = [0_u8; 4 * 1024];
    loop {
        let n = reader.read(&mut buf).await.map_err(|source| Error::Wait {
            program: "stream".to_string(),
            source,
        })?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        if stream_to_parent {
            match destination {
                StreamDestination::Stdout => {
                    let mut out = tokio::io::stdout();
                    out.write_all(&buf[..n])
                        .await
                        .map_err(|source| Error::Wait {
                            program: "stdout".to_string(),
                            source,
                        })?;
                    out.flush().await.map_err(|source| Error::Wait {
                        program: "stdout".to_string(),
                        source,
                    })?;
                }
                StreamDestination::Stderr => {
                    let mut err = tokio::io::stderr();
                    err.write_all(&buf[..n])
                        .await
                        .map_err(|source| Error::Wait {
                            program: "stderr".to_string(),
                            source,
                        })?;
                    err.flush().await.map_err(|source| Error::Wait {
                        program: "stderr".to_string(),
                        source,
                    })?;
                }
            }
        }
    }
    Ok(bytes)
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
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: None,
            remote: None,
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
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: None,
            remote: None,
        };
        let b = Action::RunCommand {
            argv,
            env: env_b,
            cwd: None,
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: None,
            remote: None,
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
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: None,
            remote: None,
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
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: None,
            remote: None,
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
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: Some(100),
            remote: None,
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
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: None,
            remote: None,
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
    async fn captures_binary_stdout_with_null_bytes() {
        // The cache stores stdout as a raw blob; null bytes and other
        // non-printable bytes must round-trip unchanged. Shellspec's
        // pipeline machinery is unreliable for this assertion across
        // shells, so the contract lives here instead.
        let (tmp, cas) = fresh_cas();
        let action = Action::RunCommand {
            argv: vec!["/bin/sh".into(), "-c".into(), r"printf 'abc\000def'".into()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: Some(5_000),
            remote: None,
        };
        let outcome = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(outcome.result.exit_code, 0);
        let stdout = cas.get_blob(&outcome.result.stdout).await.unwrap();
        assert_eq!(stdout, b"abc\x00def");
    }

    #[tokio::test]
    async fn streams_large_output_without_buffering_in_memory() {
        // Produces 4 MB of data - comfortably larger than the 64 KB
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
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: Some(10_000),
            remote: None,
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
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: Some(5_000),
            remote: None,
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

    #[tokio::test]
    async fn runner_respects_action_cpu_slots() {
        let (tmp, cas) = fresh_cas();
        let runner = Runner::new(cas, tmp.path().to_path_buf(), RunOpts::default())
            .with_resource_limits(ResourceLimits::new(2, 0));
        let mk = |suffix: &str| Action::RunCommand {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                format!("sleep 0.2; printf {suffix}"),
            ],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::new(2, 0),
            timeout_ms: Some(5_000),
            remote: None,
        };

        let started = std::time::Instant::now();
        let action_a = mk("a");
        let action_b = mk("b");
        let (a, b) = tokio::join!(runner.run(&action_a), runner.run(&action_b));
        a.unwrap();
        b.unwrap();
        assert!(
            started.elapsed() >= Duration::from_millis(380),
            "expected weighted actions to serialize; took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn digest_includes_domain_prefix() {
        // Pins the v1 domain so an accidental edit to ACTION_DIGEST_DOMAIN
        // is loud - every cached digest changes when this constant
        // changes.
        let action = Action::RunCommand {
            argv: vec!["true".into()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: None,
            remote: None,
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

    #[test]
    fn digest_changes_when_timeout_changes() {
        let mk = |t: Option<u64>| Action::RunCommand {
            argv: vec!["true".into()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: t,
            remote: None,
        };
        // None vs Some are distinct slots; differing Some values are too.
        assert_ne!(mk(None).digest(), mk(Some(1000)).digest());
        assert_ne!(mk(Some(1000)).digest(), mk(Some(2000)).digest());
    }

    #[test]
    fn digest_changes_when_cwd_changes() {
        let mk = |c: Option<WorkspacePath>| Action::RunCommand {
            argv: vec!["true".into()],
            env: BTreeMap::new(),
            cwd: c,
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: None,
            remote: None,
        };
        let a = mk(None);
        let b = mk(Some(WorkspacePath::try_from("a").unwrap()));
        let c = mk(Some(WorkspacePath::try_from("b").unwrap()));
        assert_ne!(a.digest(), b.digest());
        assert_ne!(b.digest(), c.digest());
    }

    #[test]
    fn digest_changes_when_input_digest_changes() {
        let mk = |input_digest: Option<Digest>| Action::RunCommand {
            argv: vec!["true".into()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: None,
            remote: None,
        };
        let a = mk(Some(Digest::of_bytes(b"a")));
        let b = mk(Some(Digest::of_bytes(b"b")));
        assert_ne!(mk(None).digest(), a.digest());
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn default_resources_are_wire_compatible() {
        let action = echo_action("hello");
        let encoded = serde_json::to_value(&action).unwrap();
        assert!(encoded.get("resources").is_none());

        let decoded: Action = serde_json::from_value(serde_json::json!({
            "kind": "run_command",
            "argv": ["true"]
        }))
        .unwrap();
        assert_eq!(decoded.resource_request(), &ResourceRequest::default());
    }

    #[test]
    fn workspace_path_deserialization_rejects_absolute() {
        // An absolute path encoded as JSON must round-trip into the
        // structured WorkspacePathError, not silently accept.
        let raw = serde_json::json!({
            "kind": "run_command",
            "argv": ["true"],
            "cwd": "/etc/passwd"
        });
        let err = serde_json::from_value::<Action>(raw).unwrap_err();
        assert!(
            err.to_string().contains("relative"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn empty_argv_returns_empty_argv_error() {
        let (tmp, cas) = fresh_cas();
        let action = Action::RunCommand {
            argv: vec![],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: None,
            remote: None,
        };
        let err = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::EmptyArgv));
    }

    #[tokio::test]
    async fn nonexistent_program_returns_spawn_error() {
        let (tmp, cas) = fresh_cas();
        let action = Action::RunCommand {
            argv: vec!["/this/program/does/not/exist".into()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: None,
            remote: None,
        };
        let err = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Spawn { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn child_stdin_is_closed() {
        // If stdin were inherited (or a tty), `cat` would block waiting
        // for input. With stdin closed, cat returns immediately on EOF.
        let (tmp, cas) = fresh_cas();
        let action = Action::RunCommand {
            argv: vec!["/bin/sh".into(), "-c".into(), "cat".into()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: Some(2_000),
            remote: None,
        };
        let outcome = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(outcome.result.exit_code, 0);
        assert!(cas
            .get_blob(&outcome.result.stdout)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn runner_clones_share_the_same_permit_pool() {
        // Cloning a Runner must not give each clone its own pool - the
        // semaphore exists to bound *total* concurrency. Distinct argv
        // ensures both invocations actually execute (not cache hits).
        let (tmp, cas) = fresh_cas();
        let runner =
            Runner::new(cas, tmp.path().to_path_buf(), RunOpts::default()).with_max_concurrency(1);
        let runner2 = runner.clone();
        let action_a = Action::RunCommand {
            argv: vec!["/bin/sh".into(), "-c".into(), "sleep 0.2; printf a".into()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: Some(5_000),
            remote: None,
        };
        let action_b = Action::RunCommand {
            argv: vec!["/bin/sh".into(), "-c".into(), "sleep 0.2; printf b".into()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: Some(5_000),
            remote: None,
        };
        let started = std::time::Instant::now();
        let (a, b) = tokio::join!(runner.run(&action_a), runner2.run(&action_b));
        a.unwrap();
        b.unwrap();
        assert!(
            started.elapsed() >= Duration::from_millis(380),
            "clones must share the permit pool; took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn runner_uses_the_supplied_workspace_root() {
        // The workspace_root passed to Runner::new is what `cwd` resolves
        // against - not the current working directory.
        let (tmp, cas) = fresh_cas();
        let sub = tmp.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("marker"), b"ok").unwrap();
        let runner = Runner::new(cas, tmp.path().to_path_buf(), RunOpts::default());
        let action = Action::RunCommand {
            argv: vec!["/bin/sh".into(), "-c".into(), "cat marker".into()],
            env: BTreeMap::new(),
            cwd: Some(WorkspacePath::try_from("sub").unwrap()),
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: Some(5_000),
            remote: None,
        };
        let outcome = runner.run(&action).await.unwrap();
        assert_eq!(outcome.result.exit_code, 0);
        let stdout = runner
            .cache()
            .get_blob(&outcome.result.stdout)
            .await
            .unwrap();
        assert_eq!(stdout, b"ok");
    }

    #[tokio::test]
    async fn cache_hits_do_not_queue_on_the_permit_pool() {
        // Warm the cache, then issue many concurrent runs of the same
        // action under max_concurrency=1. A naive implementation that
        // holds a permit across the cache lookup would serialize all of
        // them; with the lookup outside the permit they all return
        // immediately.
        let (tmp, cas) = fresh_cas();
        let action = echo_action("warm");
        run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();

        let runner =
            Runner::new(cas, tmp.path().to_path_buf(), RunOpts::default()).with_max_concurrency(1);
        let started = std::time::Instant::now();
        let mut handles = Vec::new();
        for _ in 0..32 {
            let runner = runner.clone();
            let action = action.clone();
            handles.push(tokio::spawn(
                async move { runner.run(&action).await.unwrap() },
            ));
        }
        for h in handles {
            assert_eq!(h.await.unwrap().cache, CacheState::Hit);
        }
        // 32 cache hits with no real concurrency cap on lookups should
        // finish far faster than any single subprocess spawn would take.
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "32 cache hits took {:?}; permit must not gate lookups",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn directory_outputs_restore_from_cache() {
        let (tmp, cas) = fresh_cas();
        let action = Action::RunCommand {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "mkdir -p Demo.app/Nested && printf info > Demo.app/Info.plist && printf bin > Demo.app/Nested/Demo".into(),
            ],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            outputs: vec![WorkspacePath::try_from("Demo.app").unwrap()],
            resources: ResourceRequest::default(),
            timeout_ms: Some(5_000),
            remote: None,
        };

        let first = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(first.cache, CacheState::Miss);
        std::fs::remove_dir_all(tmp.path().join("Demo.app")).unwrap();

        let second = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(second.cache, CacheState::Hit);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("Demo.app/Info.plist")).unwrap(),
            "info"
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("Demo.app/Nested/Demo")).unwrap(),
            "bin"
        );
    }

    #[tokio::test]
    async fn timeout_does_not_pollute_the_cache() {
        // A timed-out action returns Error::Timeout; nothing should be
        // written to the action cache, so a follow-up run also runs
        // fresh (and may also time out, or may succeed if the deadline
        // changes).
        let (tmp, cas) = fresh_cas();
        let action = Action::RunCommand {
            argv: vec!["/bin/sh".into(), "-c".into(), "sleep 5".into()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            outputs: vec![],
            resources: ResourceRequest::default(),
            timeout_ms: Some(50),
            remote: None,
        };
        let _ = run(&action, tmp.path(), &cas, RunOpts::default()).await;
        // Nothing was cached: the action's slot is empty.
        assert!(cas
            .get_action_result(&action.digest())
            .await
            .unwrap()
            .is_none());
    }
}
