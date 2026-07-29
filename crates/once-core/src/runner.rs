//! Cache-aware action execution.
//!
//! This module owns the execution lifecycle around an [`Action`]: check
//! the action cache, acquire resource permits for misses, run locally or
//! remotely, restore declared outputs on hits, and write fresh results
//! back to the configured cache.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use once_cas::{ActionResult, CacheProvider, Cas, Digest};
use tracing::{debug, instrument, warn};

use crate::{execute, outputs, Action, ResourceLimits, ResourcePool, Result};

/// Whether a result came from cache or fresh execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheState {
    Hit,
    Miss,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    /// Digest of the action that produced or looked up this result.
    pub action: Digest,
    /// Process result and declared output digests.
    pub result: ActionResult,
    /// Whether the result came from cache or fresh execution.
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
        let mut limits = self.resources.limits();
        limits.cpu_slots = n.max(1);
        self.resources = Arc::new(ResourcePool::new(limits));
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
        if let Some(hit) = lookup_cached(action, &self.cache, &self.workspace_root, &key).await? {
            return Ok(hit);
        }
        let _permit = self
            .resources
            .acquire(action.resource_request().clone())
            .await;
        if let Some(hit) = lookup_cached(action, &self.cache, &self.workspace_root, &key).await? {
            return Ok(hit);
        }
        Box::pin(produce(
            action,
            &self.workspace_root,
            &self.cache,
            self.opts,
            key,
        ))
        .await
    }

    pub async fn run_streaming(&self, action: &Action) -> Result<Outcome> {
        let key = action.digest();
        if let Some(hit) = lookup_cached(action, &self.cache, &self.workspace_root, &key).await? {
            return Ok(hit);
        }
        let _permit = self
            .resources
            .acquire(action.resource_request().clone())
            .await;
        if let Some(hit) = lookup_cached(action, &self.cache, &self.workspace_root, &key).await? {
            return Ok(hit);
        }
        let result = Box::pin(execute::run(
            action,
            &self.workspace_root,
            &self.cache,
            true,
            false,
        ))
        .await?;
        let uses_cache = action_uses_cache(action);
        let cacheable = uses_cache && (result.exit_code == 0 || self.opts.cache_failures);
        if cacheable {
            self.cache.put_action_result(&key, &result).await?;
        } else if !uses_cache {
            debug!("skipping action cache for non-cacheable action");
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
    Box::pin(run_with_cache(
        action,
        workspace_root,
        &CacheProvider::Local(cas.clone()),
        opts,
    ))
    .await
}

pub async fn run_with_cache(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    opts: RunOpts,
) -> Result<Outcome> {
    let key = action.digest();
    if let Some(hit) = lookup_cached(action, cache, workspace_root, &key).await? {
        return Ok(hit);
    }
    Box::pin(produce(action, workspace_root, cache, opts, key)).await
}

pub async fn run_with_cache_streaming(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    opts: RunOpts,
) -> Result<Outcome> {
    let key = action.digest();
    if let Some(hit) = lookup_cached(action, cache, workspace_root, &key).await? {
        return Ok(hit);
    }
    let result = Box::pin(execute::run(action, workspace_root, cache, true, false)).await?;
    let uses_cache = action_uses_cache(action);
    let cacheable = uses_cache && (result.exit_code == 0 || opts.cache_failures);
    if cacheable {
        cache.put_action_result(&key, &result).await?;
    } else if !uses_cache {
        debug!("skipping action cache for non-cacheable action");
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

/// Execute one action without reading or writing the action cache.
/// Callers that need cache-aware execution should use [`Runner`] or
/// [`run_with_cache`] instead.
pub async fn run_uncached(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
) -> Result<ActionResult> {
    Box::pin(execute::run(
        action,
        workspace_root,
        cache,
        stream_to_parent,
        false,
    ))
    .await
}

pub async fn run_uncached_contract(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
) -> Result<ActionResult> {
    Box::pin(execute::run(
        action,
        workspace_root,
        cache,
        stream_to_parent,
        true,
    ))
    .await
}

pub async fn materialize_outputs(
    result: &ActionResult,
    workspace_root: &Path,
    cache: &CacheProvider,
) -> Result<()> {
    outputs::restore(result, workspace_root, cache).await
}

#[instrument(skip(cache), fields(action_digest = %key))]
async fn lookup_cached(
    action: &Action,
    cache: &CacheProvider,
    workspace_root: &Path,
    key: &Digest,
) -> Result<Option<Outcome>> {
    if !action_uses_cache(action) {
        return Ok(None);
    }
    if let Some(result) = cache.get_action_result(key).await? {
        // A partially-evicted or partially-synced entry can have its
        // action result present while a blob it points at is gone. The
        // captured stdout/stderr blobs are not restored to the workspace
        // here, but downstream consumers read them straight from the CAS,
        // so a "hit" that references a missing stream blob would crash
        // them. Verify those blobs exist before reporting a hit, and if
        // either is gone treat the entry as absent and re-execute.
        if !stream_blobs_present(&result, cache).await? {
            warn!("cached action is missing a captured stdout/stderr blob; re-executing");
            return Ok(None);
        }
        match outputs::restore(&result, workspace_root, cache).await {
            Ok(()) => {
                debug!("cache hit");
                return Ok(Some(Outcome {
                    action: *key,
                    result,
                    cache: CacheState::Hit,
                }));
            }
            // An action result survived but an output blob it references
            // is gone (evicted by `once cache gc`, or never synced from a
            // remote tier). Treat the entry as absent and let the caller
            // re-execute rather than failing the build. `restore` stages
            // every output blob before materializing any of them, so a
            // missing blob leaves no partial output on disk.
            Err(err) if is_incomplete_cache_entry(&err) => {
                warn!(error = %err, "cached action outputs are unrestorable; re-executing");
                return Ok(None);
            }
            Err(err) => return Err(err),
        }
    }
    Ok(None)
}

/// True when every captured stdout/stderr blob an action result
/// references is still present in the cache. A `false` here means the
/// entry is incomplete and the action should be re-executed rather than
/// returned as a hit a consumer cannot fully read.
async fn stream_blobs_present(result: &ActionResult, cache: &CacheProvider) -> Result<bool> {
    for digest in [result.stdout, result.stderr].into_iter().flatten() {
        if !cache.has_blob(&digest).await? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// True when a restore failed only because the cache is missing a blob
/// the action result points at - a recoverable "the cache lost data"
/// condition, distinct from a real I/O or corruption error the caller
/// should see.
fn is_incomplete_cache_entry(err: &crate::Error) -> bool {
    matches!(err, crate::Error::Cas(once_cas::Error::BlobNotFound(_)))
}

#[instrument(skip(action, cache), fields(action_digest = %key))]
async fn produce(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    opts: RunOpts,
    key: Digest,
) -> Result<Outcome> {
    let result = Box::pin(execute::run(action, workspace_root, cache, false, false)).await?;
    let uses_cache = action_uses_cache(action);
    let cacheable = uses_cache && (result.exit_code == 0 || opts.cache_failures);
    if cacheable {
        cache.put_action_result(&key, &result).await?;
    } else if !uses_cache {
        debug!("skipping action cache for non-cacheable action");
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

fn action_uses_cache(action: &Action) -> bool {
    !matches!(action, Action::LinkPath { .. })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::time::Duration;

    use crate::action::ACTION_DIGEST_DOMAIN;
    use crate::{
        ArchiveEntry, ArchiveEntryKind, ArchiveFormat, CopyPathMode, Error, OutputSymlinkMode,
        PreparePathMode, RemoteExecution, ResourceRequest, SandboxMode, WorkspacePath,
    };
    use once_cas::{CacheProvider, Cas, Digest};
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
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
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
        assert_eq!(
            cas.get_blob(&first.result.stdout.unwrap()).await.unwrap(),
            b"hello"
        );

        let second = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(second.cache, CacheState::Hit);
        assert_eq!(second.result, first.result);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn input_sandbox_exposes_declared_inputs_and_captures_outputs() {
        let (tmp, cas) = fresh_cas();
        std::fs::write(tmp.path().join("declared.txt"), "declared").unwrap();
        std::fs::write(tmp.path().join("undeclared.txt"), "undeclared").unwrap();
        let output = WorkspacePath::try_from("out/result.txt").unwrap();
        let action = Action::RunCommand {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "test -f declared.txt && test ! -e undeclared.txt && mkdir -p out && cat declared.txt > out/result.txt"
                    .into(),
            ],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: Some(Digest::of_bytes(b"sandbox-inputs")),
            inputs: vec![WorkspacePath::try_from("declared.txt").unwrap()],
            outputs: vec![output.clone()],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::Inputs,
            timeout_ms: None,
            success_exit_codes: vec![0],
            remote: None,
        };

        let first = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(first.cache, CacheState::Miss);
        assert_eq!(first.result.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(output.resolve(tmp.path())).unwrap(),
            "declared"
        );
        assert!(first.result.outputs.contains_key("out/result.txt"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn copied_input_sandbox_materializes_private_inputs() {
        let (tmp, cas) = fresh_cas();
        std::fs::write(tmp.path().join("declared.txt"), "declared").unwrap();
        let output = WorkspacePath::try_from("out/result.txt").unwrap();
        let action = Action::RunCommand {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "test ! -L declared.txt && printf changed > declared.txt && cat declared.txt > out/result.txt"
                    .into(),
            ],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: Some(Digest::of_bytes(b"copied-sandbox-inputs")),
            inputs: vec![WorkspacePath::try_from("declared.txt").unwrap()],
            outputs: vec![output.clone()],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::CopiedInputs,
            timeout_ms: None,
            success_exit_codes: vec![0],
            remote: None,
        };

        let first = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();

        assert_eq!(first.cache, CacheState::Miss);
        assert_eq!(first.result.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("declared.txt")).unwrap(),
            "declared"
        );
        assert_eq!(
            std::fs::read_to_string(output.resolve(tmp.path())).unwrap(),
            "changed"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn input_sandbox_creates_declared_cwd() {
        let (tmp, cas) = fresh_cas();
        let output = WorkspacePath::try_from("pkg/result.txt").unwrap();
        let action = Action::RunCommand {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf ok > result.txt".into(),
            ],
            env: BTreeMap::new(),
            cwd: Some(WorkspacePath::try_from("pkg").unwrap()),
            input_digest: Some(Digest::of_bytes(b"sandbox-cwd")),
            inputs: vec![],
            outputs: vec![output.clone()],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::Inputs,
            timeout_ms: None,
            success_exit_codes: vec![0],
            remote: None,
        };

        let first = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(first.cache, CacheState::Miss);
        assert_eq!(first.result.exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(output.resolve(tmp.path())).unwrap(),
            "ok"
        );
        assert!(first.result.outputs.contains_key("pkg/result.txt"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdout_stderr_redirect_to_a_shared_file_is_captured_and_restored() {
        let (tmp, cas) = fresh_cas();
        let log = WorkspacePath::try_from(".once/out/run/log.txt").unwrap();
        let action = Action::RunCommand {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf out; printf err >&2".into(),
            ],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: vec![log.clone()],
            // Both streams share the log path, reproducing `> log 2>&1`.
            stdout_path: Some(Box::new(log.clone())),
            stderr_path: Some(Box::new(log.clone())),
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
            remote: None,
        };

        let first = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(first.cache, CacheState::Miss);
        assert_eq!(first.result.exit_code, 0);
        // Redirected streams are not captured as stream blobs; they live
        // in the declared output file instead.
        assert_eq!(first.result.stdout, None);
        assert_eq!(first.result.stderr, None);
        assert!(first.result.outputs.contains_key(".once/out/run/log.txt"));
        let on_disk = std::fs::read_to_string(tmp.path().join(".once/out/run/log.txt")).unwrap();
        assert!(on_disk.contains("out"), "log missing stdout: {on_disk:?}");
        assert!(on_disk.contains("err"), "log missing stderr: {on_disk:?}");

        // A cache hit restores the redirected file from the CAS.
        std::fs::remove_file(tmp.path().join(".once/out/run/log.txt")).unwrap();
        let second = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(second.cache, CacheState::Hit);
        let restored = std::fs::read_to_string(tmp.path().join(".once/out/run/log.txt")).unwrap();
        assert!(restored.contains("out"));
        assert!(restored.contains("err"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdout_redirect_leaves_stderr_stream_captured() {
        let (tmp, cas) = fresh_cas();
        let out = WorkspacePath::try_from(".once/out/only-stdout.txt").unwrap();
        let action = Action::RunCommand {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf captured-stdout; printf streamed-stderr >&2".into(),
            ],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: vec![out.clone()],
            stdout_path: Some(Box::new(out.clone())),
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
            remote: None,
        };

        let outcome = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(outcome.result.exit_code, 0);
        // stdout went to the file; stderr stays a captured stream blob.
        assert_eq!(outcome.result.stdout, None);
        let stderr = cas.get_blob(&outcome.result.stderr.unwrap()).await.unwrap();
        assert_eq!(stderr, b"streamed-stderr");
        let on_disk =
            std::fs::read_to_string(tmp.path().join(".once/out/only-stdout.txt")).unwrap();
        assert_eq!(on_disk, "captured-stdout");
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
    async fn write_file_action_restores_from_cache() {
        let (tmp, cas) = fresh_cas();
        let action = Action::WriteFile {
            path: WorkspacePath::try_from("out/generated.txt").unwrap(),
            bytes: b"generated".to_vec(),
            input_digest: Some(Digest::of_bytes(b"write-file")),
        };

        let first = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(first.cache, CacheState::Miss);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("out/generated.txt")).unwrap(),
            "generated"
        );
        std::fs::remove_file(tmp.path().join("out/generated.txt")).unwrap();

        let second = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(second.cache, CacheState::Hit);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("out/generated.txt")).unwrap(),
            "generated"
        );
    }

    #[tokio::test]
    async fn write_archive_action_restores_archive_and_digest_from_cache() {
        let (tmp, cas) = fresh_cas();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src/hello"), "hello").unwrap();
        let action = Action::WriteArchive {
            entries: vec![ArchiveEntry {
                kind: ArchiveEntryKind::File,
                source: Some(WorkspacePath::try_from("src/hello").unwrap()),
                path: "usr/local/bin/hello".to_string(),
                mode: 0o755,
                directory_mode: 0o755,
                owner_id: 0,
                group_id: 0,
                mtime: 0,
            }],
            output: WorkspacePath::try_from("out/layer.tar").unwrap(),
            sha256_output: Some(WorkspacePath::try_from("out/layer.tar.sha256").unwrap()),
            format: ArchiveFormat::Tar,
            input_digest: Some(Digest::of_bytes(b"write-archive")),
        };

        let first = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(first.cache, CacheState::Miss);
        let archive = std::fs::read(tmp.path().join("out/layer.tar")).unwrap();
        let digest = std::fs::read_to_string(tmp.path().join("out/layer.tar.sha256")).unwrap();
        assert!(!archive.is_empty());
        assert_eq!(digest.trim().len(), 64);
        std::fs::remove_file(tmp.path().join("out/layer.tar")).unwrap();
        std::fs::remove_file(tmp.path().join("out/layer.tar.sha256")).unwrap();

        let second = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(second.cache, CacheState::Hit);
        assert_eq!(
            std::fs::read(tmp.path().join("out/layer.tar")).unwrap(),
            archive
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("out/layer.tar.sha256")).unwrap(),
            digest
        );
    }

    #[tokio::test]
    async fn cache_hit_with_evicted_output_blob_falls_back_to_execution() {
        let (tmp, cas) = fresh_cas();
        let action = Action::WriteFile {
            path: WorkspacePath::try_from("out/generated.txt").unwrap(),
            bytes: b"generated".to_vec(),
            input_digest: Some(Digest::of_bytes(b"evicted-blob")),
        };

        let first = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(first.cache, CacheState::Miss);

        // Simulate `once cache gc` reclaiming the blob tier while the
        // action result survives, plus the materialized output being
        // removed from the workspace.
        std::fs::remove_dir_all(tmp.path().join("cas")).unwrap();
        std::fs::remove_file(tmp.path().join("out/generated.txt")).unwrap();
        assert!(
            cas.get_action_result(&action.digest())
                .await
                .unwrap()
                .is_some(),
            "the stale action result must still be present"
        );

        // The stale result points at a now-missing blob. The lookup must
        // degrade to a miss and re-execute rather than erroring.
        let second = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(second.cache, CacheState::Miss);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("out/generated.txt")).unwrap(),
            "generated"
        );

        // And the cache is self-healed: a third run hits cleanly.
        let third = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(third.cache, CacheState::Hit);
    }

    #[tokio::test]
    async fn cache_hit_with_evicted_stdout_blob_falls_back_to_execution() {
        let (tmp, cas) = fresh_cas();
        let action = echo_action("streamed-hello");

        let first = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(first.cache, CacheState::Miss);
        assert!(first.result.stdout.is_some());

        // Evict the blob tier (as `once cache gc` cascading, or a remote
        // tier that never synced the stream blobs) while the action
        // result survives and still references the stdout/stderr blobs.
        std::fs::remove_dir_all(tmp.path().join("cas")).unwrap();
        assert!(cas
            .get_action_result(&action.digest())
            .await
            .unwrap()
            .is_some());

        // The referenced stdout blob is gone, so the lookup must decline
        // the hit and re-execute rather than return a result whose stdout
        // a consumer cannot read.
        let second = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(second.cache, CacheState::Miss);
        assert_eq!(
            cas.get_blob(&second.result.stdout.unwrap()).await.unwrap(),
            b"streamed-hello"
        );

        let third = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(third.cache, CacheState::Hit);
    }

    #[tokio::test]
    async fn copy_file_action_materializes_destination() {
        let (tmp, cas) = fresh_cas();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src/input.txt"), "input").unwrap();
        let action = Action::CopyPath {
            sources: vec![WorkspacePath::try_from("src/input.txt").unwrap()],
            destination: WorkspacePath::try_from("out/copied.txt").unwrap(),
            mode: CopyPathMode::File,
            input_digest: Some(Digest::of_bytes(b"copy-file")),
        };

        let outcome = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();

        assert_eq!(outcome.cache, CacheState::Miss);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("out/copied.txt")).unwrap(),
            "input"
        );
        assert!(outcome.result.outputs.contains_key("out/copied.txt"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn copy_file_action_materializes_a_directory_symlink_value() {
        let (tmp, cas) = fresh_cas();
        std::fs::create_dir_all(tmp.path().join("shared/nested")).unwrap();
        std::fs::write(tmp.path().join("shared/nested/value.txt"), "value").unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::os::unix::fs::symlink("../shared", tmp.path().join("src/resources")).unwrap();
        let action = Action::CopyPath {
            sources: vec![WorkspacePath::try_from("src/resources").unwrap()],
            destination: WorkspacePath::try_from("out/resources").unwrap(),
            mode: CopyPathMode::File,
            input_digest: Some(Digest::of_bytes(b"copy-directory-symlink")),
        };

        let outcome = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();

        assert_eq!(outcome.cache, CacheState::Miss);
        assert!(!std::fs::symlink_metadata(tmp.path().join("out/resources"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("out/resources/nested/value.txt")).unwrap(),
            "value"
        );
    }

    #[tokio::test]
    async fn materialize_host_file_verifies_content_and_replays_from_cache() {
        let (tmp, cas) = fresh_cas();
        let host = TempDir::new().unwrap();
        let source = host.path().join("toolchain.bin");
        std::fs::write(&source, b"toolchain").unwrap();
        let action = Action::MaterializeHostFile {
            source: source.clone(),
            source_sha256: "0db3de82a739e43a2b560d166d037c3c0061601bb194866eb79b2c87045d00f2"
                .to_string(),
            destination: WorkspacePath::try_from("out/toolchain.bin").unwrap(),
            input_digest: Some(Digest::of_bytes(b"host-toolchain")),
        };

        let first = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(first.cache, CacheState::Miss);
        assert_eq!(
            std::fs::read(tmp.path().join("out/toolchain.bin")).unwrap(),
            b"toolchain"
        );

        std::fs::remove_file(source).unwrap();
        std::fs::remove_file(tmp.path().join("out/toolchain.bin")).unwrap();
        let second = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(second.cache, CacheState::Hit);
        assert_eq!(
            std::fs::read(tmp.path().join("out/toolchain.bin")).unwrap(),
            b"toolchain"
        );
    }

    #[tokio::test]
    async fn materialize_host_file_rejects_content_changed_after_analysis() {
        let (tmp, cas) = fresh_cas();
        let source = tmp.path().join("toolchain.bin");
        std::fs::write(&source, b"changed").unwrap();
        let action = Action::MaterializeHostFile {
            source,
            source_sha256: "0db3de82a739e43a2b560d166d037c3c0061601bb194866eb79b2c87045d00f2"
                .to_string(),
            destination: WorkspacePath::try_from("out/toolchain.bin").unwrap(),
            input_digest: Some(Digest::of_bytes(b"changed-host-toolchain")),
        };

        let error = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap_err();
        assert!(matches!(error, Error::HostFileDigestMismatch { .. }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn materialize_host_tree_preserves_modes_and_symlinks_and_replays_from_cache() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let (tmp, cas) = fresh_cas();
        let host = TempDir::new().unwrap();
        let source = host.path().join("crate");
        std::fs::create_dir_all(source.join("src")).unwrap();
        std::fs::write(source.join("src/lib.rs"), "pub fn value() {}\n").unwrap();
        let executable = source.join("build-helper");
        std::fs::write(&executable, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        symlink("src/lib.rs", source.join("linked.rs")).unwrap();
        let source_sha256 = crate::execute::host_tree_sha256_hex(&source).unwrap();
        let action = Action::MaterializeHostTree {
            source: source.clone(),
            source_sha256,
            destination: WorkspacePath::try_from("out/crate").unwrap(),
            input_digest: Some(Digest::of_bytes(b"host-tree")),
        };

        let first = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(first.cache, CacheState::Miss);
        assert_eq!(
            std::fs::metadata(tmp.path().join("out/crate/build-helper"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert!(
            std::fs::symlink_metadata(tmp.path().join("out/crate/linked.rs"))
                .unwrap()
                .file_type()
                .is_symlink()
        );

        std::fs::remove_dir_all(source).unwrap();
        std::fs::remove_dir_all(tmp.path().join("out/crate")).unwrap();
        let second = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(second.cache, CacheState::Hit);
        assert_eq!(
            std::fs::read_link(tmp.path().join("out/crate/linked.rs")).unwrap(),
            std::path::PathBuf::from("src/lib.rs")
        );
    }

    #[tokio::test]
    async fn materialize_host_tree_rejects_content_changed_after_analysis() {
        let (tmp, cas) = fresh_cas();
        let source = tmp.path().join("crate");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("source.rs"), "before\n").unwrap();
        let source_sha256 = crate::execute::host_tree_sha256_hex(&source).unwrap();
        std::fs::write(source.join("source.rs"), "after\n").unwrap();
        let action = Action::MaterializeHostTree {
            source,
            source_sha256,
            destination: WorkspacePath::try_from("out/crate").unwrap(),
            input_digest: Some(Digest::of_bytes(b"changed-host-tree")),
        };

        let error = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap_err();
        assert!(matches!(error, Error::HostFileDigestMismatch { .. }));
    }

    #[tokio::test]
    async fn copy_tree_action_replaces_destination_and_restores_from_cache() {
        let (tmp, cas) = fresh_cas();
        std::fs::create_dir_all(tmp.path().join("src/a")).unwrap();
        std::fs::create_dir_all(tmp.path().join("src/b")).unwrap();
        std::fs::write(tmp.path().join("src/a/one.txt"), "one").unwrap();
        std::fs::write(tmp.path().join("src/b/two.txt"), "two").unwrap();
        std::fs::create_dir_all(tmp.path().join("out/tree")).unwrap();
        std::fs::write(tmp.path().join("out/tree/stale.txt"), "stale").unwrap();
        let action = Action::CopyPath {
            sources: vec![
                WorkspacePath::try_from("src/a").unwrap(),
                WorkspacePath::try_from("src/b").unwrap(),
            ],
            destination: WorkspacePath::try_from("out/tree").unwrap(),
            mode: CopyPathMode::Tree,
            input_digest: Some(Digest::of_bytes(b"copy-tree")),
        };

        let first = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(first.cache, CacheState::Miss);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("out/tree/one.txt")).unwrap(),
            "one"
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("out/tree/two.txt")).unwrap(),
            "two"
        );
        assert!(!tmp.path().join("out/tree/stale.txt").exists());
        std::fs::remove_dir_all(tmp.path().join("out/tree")).unwrap();

        let second = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(second.cache, CacheState::Hit);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("out/tree/one.txt")).unwrap(),
            "one"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn copy_tree_action_preserves_framework_symlinks() {
        let (tmp, cas) = fresh_cas();
        let framework = tmp.path().join("src/Shared.framework");
        std::fs::create_dir_all(framework.join("Versions/A/Headers")).unwrap();
        std::fs::create_dir_all(framework.join("Versions/A/Modules")).unwrap();
        std::fs::write(framework.join("Versions/A/Shared"), "binary").unwrap();
        std::fs::write(framework.join("Versions/A/Headers/Shared.h"), "header").unwrap();
        std::os::unix::fs::symlink("A", framework.join("Versions/Current")).unwrap();
        std::os::unix::fs::symlink("Versions/Current/Headers", framework.join("Headers")).unwrap();
        std::os::unix::fs::symlink("Versions/Current/Modules", framework.join("Modules")).unwrap();
        std::os::unix::fs::symlink("Versions/Current/Shared", framework.join("Shared")).unwrap();
        let action = Action::CopyPath {
            sources: vec![WorkspacePath::try_from("src/Shared.framework").unwrap()],
            destination: WorkspacePath::try_from("out/Frameworks/Shared.framework").unwrap(),
            mode: CopyPathMode::Tree,
            input_digest: Some(Digest::of_bytes(b"copy-framework")),
        };

        let first = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(first.cache, CacheState::Miss);
        let copied = tmp.path().join("out/Frameworks/Shared.framework");
        assert!(std::fs::symlink_metadata(copied.join("Headers"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_link(copied.join("Headers")).unwrap(),
            std::path::PathBuf::from("Versions/Current/Headers")
        );
        assert!(std::fs::symlink_metadata(copied.join("Shared"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(copied.join("Headers/Shared.h")).unwrap(),
            "header"
        );

        std::fs::remove_dir_all(tmp.path().join("out/Frameworks")).unwrap();
        let second = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(second.cache, CacheState::Hit);
        assert!(std::fs::symlink_metadata(copied.join("Modules"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_link(copied.join("Shared")).unwrap(),
            std::path::PathBuf::from("Versions/Current/Shared")
        );
    }

    #[tokio::test]
    async fn write_tree_digest_action_filters_by_suffix() {
        let (tmp, cas) = fresh_cas();
        std::fs::create_dir_all(tmp.path().join("tree/sub")).unwrap();
        std::fs::write(tmp.path().join("tree/sub/a.java"), "java").unwrap();
        std::fs::write(tmp.path().join("tree/sub/b.txt"), "text").unwrap();
        let action = Action::WriteTreeDigest {
            root: WorkspacePath::try_from("tree").unwrap(),
            output: WorkspacePath::try_from("out/tree.sha256").unwrap(),
            include_suffixes: vec![".java".to_string()],
            input_digest: Some(Digest::of_bytes(b"tree-digest")),
        };

        let outcome = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();

        assert_eq!(outcome.cache, CacheState::Miss);
        let digest_file = std::fs::read_to_string(tmp.path().join("out/tree.sha256")).unwrap();
        assert!(digest_file.contains("sub/a.java"), "{digest_file}");
        assert!(!digest_file.contains("sub/b.txt"), "{digest_file}");
        assert!(outcome.result.outputs.contains_key("out/tree.sha256"));
    }

    #[tokio::test]
    async fn remove_path_and_ensure_dir_run_uncached() {
        let (tmp, cas) = fresh_cas();
        let cache = CacheProvider::Local(cas);
        std::fs::create_dir_all(tmp.path().join("out/stale")).unwrap();
        std::fs::write(tmp.path().join("out/stale/file.txt"), "stale").unwrap();
        let remove = Action::PreparePath {
            path: WorkspacePath::try_from("out/stale").unwrap(),
            mode: PreparePathMode::Remove,
            input_digest: Some(Digest::of_bytes(b"remove")),
        };
        let ensure = Action::PreparePath {
            path: WorkspacePath::try_from("out/stale").unwrap(),
            mode: PreparePathMode::Directory,
            input_digest: Some(Digest::of_bytes(b"ensure")),
        };

        run_uncached(&remove, tmp.path(), &cache, false)
            .await
            .unwrap();
        assert!(!tmp.path().join("out/stale").exists());
        run_uncached(&ensure, tmp.path(), &cache, false)
            .await
            .unwrap();
        assert!(tmp.path().join("out/stale").is_dir());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn link_path_replaces_the_destination_with_a_workspace_symlink() {
        let (tmp, cas) = fresh_cas();
        let cache = CacheProvider::Local(cas);
        std::fs::create_dir_all(tmp.path().join("deps/node_modules/pkg")).unwrap();
        std::fs::write(tmp.path().join("deps/node_modules/pkg/package.json"), "{}").unwrap();
        std::fs::create_dir_all(tmp.path().join("app/node_modules")).unwrap();
        std::fs::write(tmp.path().join("app/node_modules/stale.txt"), "stale").unwrap();
        let action = Action::LinkPath {
            source: WorkspacePath::try_from("deps/node_modules").unwrap(),
            destination: WorkspacePath::try_from("app/node_modules").unwrap(),
            input_digest: None,
        };

        run_uncached(&action, tmp.path(), &cache, false)
            .await
            .unwrap();

        let destination = tmp.path().join("app/node_modules");
        assert!(std::fs::symlink_metadata(&destination)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_link(&destination).unwrap(),
            std::path::Path::new("../deps/node_modules")
        );
        assert_eq!(
            std::fs::read_to_string(destination.join("pkg/package.json")).unwrap(),
            "{}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn link_path_keeps_directory_input_digests_stable_across_workspace_roots() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        let mut digests = Vec::new();
        for workspace in [first.path(), second.path()] {
            let cache = CacheProvider::Local(Cas::open(workspace.join(".cache")));
            std::fs::create_dir_all(workspace.join("deps/node_modules/pkg")).unwrap();
            std::fs::write(workspace.join("deps/node_modules/pkg/package.json"), "{}").unwrap();
            let action = Action::LinkPath {
                source: WorkspacePath::try_from("deps/node_modules").unwrap(),
                destination: WorkspacePath::try_from("app/node_modules").unwrap(),
                input_digest: None,
            };
            run_uncached(&action, workspace, &cache, false)
                .await
                .unwrap();
            let mut digest = crate::input_digest::InputDigestBuilder::new(b"link-path-test");
            digest.push_source(workspace, "app").unwrap();
            digests.push(digest.finish());
        }
        assert_eq!(digests[0], digests[1]);
    }

    #[tokio::test]
    async fn link_path_rejects_a_missing_source_without_removing_the_destination() {
        let (tmp, cas) = fresh_cas();
        let cache = CacheProvider::Local(cas);
        std::fs::create_dir_all(tmp.path().join("app/node_modules")).unwrap();
        std::fs::write(tmp.path().join("app/node_modules/keep.txt"), "keep").unwrap();
        let action = Action::LinkPath {
            source: WorkspacePath::try_from("deps/node_modules").unwrap(),
            destination: WorkspacePath::try_from("app/node_modules").unwrap(),
            input_digest: None,
        };

        let error = run_uncached(&action, tmp.path(), &cache, false)
            .await
            .unwrap_err();

        assert!(matches!(error, Error::FileAction { .. }));
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("app/node_modules/keep.txt")).unwrap(),
            "keep"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn link_path_rejects_a_destination_that_would_remove_the_source() {
        let (tmp, cas) = fresh_cas();
        let cache = CacheProvider::Local(cas);
        std::fs::create_dir_all(tmp.path().join("deps/node_modules/pkg")).unwrap();
        std::fs::write(tmp.path().join("deps/node_modules/pkg/package.json"), "{}").unwrap();
        let action = Action::LinkPath {
            source: WorkspacePath::try_from("deps/node_modules").unwrap(),
            destination: WorkspacePath::try_from("deps").unwrap(),
            input_digest: None,
        };

        let error = run_uncached(&action, tmp.path(), &cache, false)
            .await
            .unwrap_err();

        assert!(matches!(error, Error::InvalidLinkPath { .. }));
        assert!(tmp
            .path()
            .join("deps/node_modules/pkg/package.json")
            .is_file());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn link_path_rejects_logically_equal_normalized_paths() {
        let (tmp, cas) = fresh_cas();
        let cache = CacheProvider::Local(cas);
        std::fs::create_dir_all(tmp.path().join("deps/node_modules")).unwrap();
        let action = Action::LinkPath {
            source: WorkspacePath::try_from("deps/./node_modules").unwrap(),
            destination: WorkspacePath::try_from("deps/node_modules").unwrap(),
            input_digest: None,
        };

        let error = run_uncached(&action, tmp.path(), &cache, false)
            .await
            .unwrap_err();

        assert!(matches!(error, Error::InvalidLinkPath { .. }));
        assert!(tmp.path().join("deps/node_modules").is_dir());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runner_recreates_workspace_links_instead_of_caching_them() {
        let workspace = TempDir::new().unwrap();
        let cache_root = TempDir::new().unwrap();
        std::fs::create_dir_all(workspace.path().join("deps/node_modules/pkg")).unwrap();
        std::fs::write(
            workspace.path().join("deps/node_modules/pkg/package.json"),
            "{}",
        )
        .unwrap();
        let action = Action::LinkPath {
            source: WorkspacePath::try_from("deps/node_modules").unwrap(),
            destination: WorkspacePath::try_from("app/node_modules").unwrap(),
            input_digest: None,
        };
        let runner = Runner::new(
            Cas::open(cache_root.path()),
            workspace.path(),
            RunOpts::default(),
        );

        let first = runner.run(&action).await.unwrap();
        std::fs::remove_file(workspace.path().join("app/node_modules")).unwrap();
        let second = runner.run(&action).await.unwrap();

        assert_eq!(first.cache, CacheState::Miss);
        assert_eq!(second.cache, CacheState::Miss);
        assert!(
            std::fs::symlink_metadata(workspace.path().join("app/node_modules"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
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
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
            remote: None,
        };
        let b = Action::RunCommand {
            argv,
            env: env_b,
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
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
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
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
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
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
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: Some(100),
            success_exit_codes: vec![0],
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
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
            remote: None,
        };
        let outcome = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(outcome.result.exit_code, 0);
        assert_eq!(
            cas.get_blob(&outcome.result.stdout.unwrap()).await.unwrap(),
            b"present"
        );
    }

    #[tokio::test]
    async fn captures_binary_stdout_with_null_bytes() {
        let (tmp, cas) = fresh_cas();
        let action = Action::RunCommand {
            argv: vec!["/bin/sh".into(), "-c".into(), r"printf 'abc\000def'".into()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: Some(5_000),
            success_exit_codes: vec![0],
            remote: None,
        };
        let outcome = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(outcome.result.exit_code, 0);
        let stdout = cas.get_blob(&outcome.result.stdout.unwrap()).await.unwrap();
        assert_eq!(stdout, b"abc\x00def");
    }

    #[tokio::test]
    async fn streams_large_output_without_buffering_in_memory() {
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
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: Some(10_000),
            success_exit_codes: vec![0],
            remote: None,
        };
        let outcome = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(outcome.result.exit_code, 0);
        let stdout = cas.get_blob(&outcome.result.stdout.unwrap()).await.unwrap();
        assert_eq!(stdout.len(), 4 * 1024 * 1024);
    }

    #[tokio::test]
    async fn streaming_run_writes_and_reuses_cache_entries() {
        let (tmp, cas) = fresh_cas();
        let cache = CacheProvider::Local(cas.clone());
        let action = Action::RunCommand {
            argv: vec!["/bin/sh".into(), "-c".into(), "true".into()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
            remote: None,
        };

        let first = run_with_cache_streaming(&action, tmp.path(), &cache, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(first.cache, CacheState::Miss);
        assert_eq!(
            cas.get_blob(&first.result.stdout.unwrap()).await.unwrap(),
            b""
        );

        let second = run_with_cache_streaming(&action, tmp.path(), &cache, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(second.cache, CacheState::Hit);
    }

    #[tokio::test]
    async fn remote_actions_delegate_to_remote_provider_dispatch() {
        let (tmp, cas) = fresh_cas();
        let action = Action::RunCommand {
            argv: vec!["true".into()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
            remote: Some(Box::new(RemoteExecution::provider("unknown-remote"))),
        };

        let error = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            Error::UnsupportedRemoteProvider { ref provider } if provider == "unknown-remote"
        ));
    }

    #[tokio::test]
    async fn runner_caps_concurrency() {
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
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: Some(5_000),
            success_exit_codes: vec![0],
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
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::new(2, 0),
            sandbox: SandboxMode::default(),
            timeout_ms: Some(5_000),
            success_exit_codes: vec![0],
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
        let action = Action::RunCommand {
            argv: vec!["true".into()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
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
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: t,
            success_exit_codes: vec![0],
            remote: None,
        };
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
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
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
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
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
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
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
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
            remote: None,
        };
        let err = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Spawn { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn child_stdin_is_closed() {
        let (tmp, cas) = fresh_cas();
        let action = Action::RunCommand {
            argv: vec!["/bin/sh".into(), "-c".into(), "cat".into()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: Some(2_000),
            success_exit_codes: vec![0],
            remote: None,
        };
        let outcome = run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();
        assert_eq!(outcome.result.exit_code, 0);
        assert!(cas
            .get_blob(&outcome.result.stdout.unwrap())
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn runner_clones_share_the_same_permit_pool() {
        let (tmp, cas) = fresh_cas();
        let runner =
            Runner::new(cas, tmp.path().to_path_buf(), RunOpts::default()).with_max_concurrency(1);
        let runner2 = runner.clone();
        let action_a = Action::RunCommand {
            argv: vec!["/bin/sh".into(), "-c".into(), "sleep 0.2; printf a".into()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: Some(5_000),
            success_exit_codes: vec![0],
            remote: None,
        };
        let action_b = Action::RunCommand {
            argv: vec!["/bin/sh".into(), "-c".into(), "sleep 0.2; printf b".into()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: Some(5_000),
            success_exit_codes: vec![0],
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
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: Some(5_000),
            success_exit_codes: vec![0],
            remote: None,
        };
        let outcome = runner.run(&action).await.unwrap();
        assert_eq!(outcome.result.exit_code, 0);
        let stdout = runner
            .cache()
            .get_blob(&outcome.result.stdout.unwrap())
            .await
            .unwrap();
        assert_eq!(stdout, b"ok");
    }

    #[tokio::test]
    async fn cache_hits_do_not_queue_on_the_permit_pool() {
        let (tmp, cas) = fresh_cas();
        let action = echo_action("warm");
        run(&action, tmp.path(), &cas, RunOpts::default())
            .await
            .unwrap();

        let runner =
            Runner::new(cas, tmp.path().to_path_buf(), RunOpts::default()).with_max_concurrency(1);

        // Exhaust the single permit and hold it for the whole test. If cache
        // hits queued on the pool they would block forever here, so the
        // timeout below fires; correct behavior short-circuits before the
        // permit is ever requested and completes immediately.
        let _held = runner.resources.acquire(ResourceRequest::default()).await;

        let mut handles = Vec::new();
        for _ in 0..32 {
            let runner = runner.clone();
            let action = action.clone();
            handles.push(tokio::spawn(
                async move { runner.run(&action).await.unwrap() },
            ));
        }
        for h in handles {
            let outcome = tokio::time::timeout(Duration::from_secs(5), h)
                .await
                .expect("cache hit blocked on the permit pool")
                .unwrap();
            assert_eq!(outcome.cache, CacheState::Hit);
        }
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
            inputs: vec![],
            outputs: vec![WorkspacePath::try_from("Demo.app").unwrap()],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: Some(5_000),
            success_exit_codes: vec![0],
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
        let (tmp, cas) = fresh_cas();
        let action = Action::RunCommand {
            argv: vec!["/bin/sh".into(), "-c".into(), "sleep 5".into()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: vec![],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: Some(50),
            success_exit_codes: vec![0],
            remote: None,
        };
        let _ = run(&action, tmp.path(), &cas, RunOpts::default()).await;
        assert!(cas
            .get_action_result(&action.digest())
            .await
            .unwrap()
            .is_none());
    }
}
