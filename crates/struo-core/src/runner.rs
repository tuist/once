//! Cache-aware action execution.
//!
//! This module owns the execution lifecycle around an [`Action`]: check
//! the action cache, acquire resource permits for misses, run locally or
//! remotely, restore declared outputs on hits, and write fresh results
//! back to the configured cache.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use struo_cas::{ActionResult, CacheProvider, Cas, Digest};
use tracing::{debug, instrument, warn};

use crate::stream::ActionOutputObserver;
use crate::{
    execute, outputs, Action, Executor, ExecutorResources, ResourceLimits, ResourcePool, Result,
};

static DEFAULT_LIMITS: LazyLock<ResourceLimits> = LazyLock::new(ResourceLimits::default);

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
/// A `Runner` admits local compute and remote client work through shared
/// [`ExecutorResources`]. Local processes and micro-VMs use the same host
/// budget. Hosted requests use a separate concurrency quota and reserve host
/// client memory, not their guest CPU or memory estimates. Clones share budgets.
#[derive(Clone)]
pub struct Runner {
    cache: CacheProvider,
    workspace_root: PathBuf,
    opts: RunOpts,
    pub(crate) resources: ExecutorResources,
}

impl Runner {
    /// Prepare a command path without capturing unrelated directory contents.
    pub async fn prepare_path_admitted(
        path: &crate::WorkspacePath,
        mode: crate::PreparePathMode,
        workspace_root: &Path,
        lease: crate::ExecutionPermit,
    ) -> Result<()> {
        execute::prepare_path(path, mode, workspace_root, Some(&lease)).await
    }

    /// Execute a caller-admitted command with explicit workspace setup.
    /// Graph callers must include setup in their own command/cache identity.
    /// Docker applies it after staging inputs; native unsandboxed execution
    /// applies it to the host workspace. Other adapters fail closed.
    /// Docker restores `post_setup_inputs` after cleanup, for response files
    /// whose paths are inside reset scratch directories.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_uncached_admitted_with_setup(
        action: &Action,
        workspace_root: &Path,
        cache: &CacheProvider,
        stream_to_parent: bool,
        observer: Option<&dyn ActionOutputObserver>,
        lease: crate::ExecutionPermit,
        clean_paths: &[crate::WorkspacePath],
        create_dirs: &[crate::WorkspacePath],
        post_setup_inputs: &[crate::WorkspacePath],
    ) -> Result<ActionResult> {
        let executor = Executor::resolve(action)?;
        if !matches!(action, Action::RunCommand { .. }) {
            return Err(executor.unsupported("command filesystem setup on a non-command action"));
        }
        if lease.executor() != Some(executor) {
            return Err(
                executor.unsupported("execution permit does not match the resolved executor")
            );
        }
        if !matches!(executor, Executor::Docker | Executor::Tuist) && !post_setup_inputs.is_empty()
        {
            return Err(executor.unsupported("restoring inputs after command filesystem setup"));
        }
        if matches!(action, Action::RunCommand { sandbox, .. } if executor == Executor::Local && *sandbox != crate::SandboxMode::Off)
            && (!clean_paths.is_empty() || !create_dirs.is_empty())
        {
            return Err(executor.unsupported("explicit filesystem setup in native input sandboxes"));
        }
        execute::run_admitted_with_setup(
            action,
            workspace_root,
            cache,
            stream_to_parent,
            false,
            observer,
            Some(lease),
            crate::remote::CommandFilesystemSetup {
                clean_paths,
                create_dirs,
                post_setup_inputs,
            },
        )
        .await
    }

    pub fn new(cas: Cas, workspace_root: impl Into<PathBuf>, opts: RunOpts) -> Self {
        Self::with_cache(CacheProvider::Local(cas), workspace_root, opts)
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
            resources: ExecutorResources::new(Arc::new(ResourcePool::new(DEFAULT_LIMITS.clone()))),
        }
    }

    /// Override host CPU slots, preserving explicit remote admission settings.
    /// A value of 0 is silently raised to 1.
    #[must_use]
    pub fn with_max_concurrency(mut self, n: usize) -> Self {
        let mut limits = self.resources.host().limits();
        limits.cpu_slots = n.max(1);
        self.resources = self
            .resources
            .with_host(Arc::new(ResourcePool::new(limits)));
        self
    }

    /// Replace the host budget, preserving explicit remote admission settings.
    #[must_use]
    pub fn with_resource_limits(mut self, limits: ResourceLimits) -> Self {
        self.resources = self
            .resources
            .with_host(Arc::new(ResourcePool::new(limits)));
        self
    }

    /// Supply shared domains, including a custom remote concurrency quota.
    /// Configure budgets before cloning runners or admitting work.
    #[must_use]
    pub fn with_executor_resources(mut self, resources: ExecutorResources) -> Self {
        self.resources = resources;
        self
    }

    pub(super) fn max_parallel_plan_tasks(&self) -> usize {
        self.resources
            .host()
            .max_parallel_actions()
            .saturating_add(self.resources.remote_concurrency())
    }

    pub fn cache(&self) -> &CacheProvider {
        &self.cache
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub async fn run(&self, action: &Action) -> Result<Outcome> {
        self.run_inner(action, false).await
    }

    pub async fn run_streaming(&self, action: &Action) -> Result<Outcome> {
        self.run_inner(action, true).await
    }

    /// Execute without reading or writing the action cache, with shared admission.
    pub async fn run_fresh(&self, action: &Action, stream_to_parent: bool) -> Result<ActionResult> {
        let executor = Executor::resolve(action)?;
        let permit = self
            .resources
            .acquire(executor, action.resource_request())
            .await;
        Box::pin(execute::run_admitted(
            action,
            &self.workspace_root,
            &self.cache,
            stream_to_parent,
            false,
            None,
            Some(permit.clone()),
        ))
        .await
    }

    async fn run_inner(&self, action: &Action, stream_to_parent: bool) -> Result<Outcome> {
        let executor = Executor::resolve(action)?;
        let key = action.digest();
        if let Some(hit) = lookup_cached(
            action,
            &self.cache,
            &self.workspace_root,
            &key,
            Some(&self.resources),
        )
        .await?
        {
            return Ok(hit);
        }
        let permit = self
            .resources
            .acquire(executor, action.resource_request())
            .await;
        if let Some(hit) =
            lookup_cached(action, &self.cache, &self.workspace_root, &key, None).await?
        {
            return Ok(hit);
        }
        let result = Box::pin(execute::run_admitted(
            action,
            &self.workspace_root,
            &self.cache,
            stream_to_parent,
            false,
            None,
            Some(permit.clone()),
        ))
        .await?;
        store_result(action, &self.cache, self.opts, key, result).await
    }
}

/// Convenience: run a single action without constructing a [`Runner`].
/// This creates per-call budgets. Concurrent schedulers must reuse a [`Runner`]
/// or supply shared [`ExecutorResources`] for aggregate admission.
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
    Runner::with_cache(cache.clone(), workspace_root, opts)
        .run(action)
        .await
}

pub async fn run_with_cache_streaming(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    opts: RunOpts,
) -> Result<Outcome> {
    Runner::with_cache(cache.clone(), workspace_root, opts)
        .run_streaming(action)
        .await
}

/// Execute graph work with caller-owned admission and optional output observation.
/// Blocking filesystem work and cleanup retain clones of the supplied lease.
/// Remote commands currently reject live output observers.
pub async fn run_uncached_admitted(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
    observer: Option<&dyn ActionOutputObserver>,
    lease: crate::ExecutionPermit,
) -> Result<ActionResult> {
    let executor = Executor::resolve(action)?;
    if matches!(action, Action::RunCommand { .. }) && lease.executor() != Some(executor) {
        return Err(executor.unsupported("execution permit does not match the resolved executor"));
    }
    execute::run_admitted(
        action,
        workspace_root,
        cache,
        stream_to_parent,
        false,
        observer,
        Some(lease),
    )
    .await
}

/// Execute one action without reading or writing the action cache.
/// This is a caller-admitted primitive for local graph work. Use
/// [`Runner::run_fresh`] for hosted work so admission follows sandbox cleanup.
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

/// Execute one action without the action cache and receive its output as it is
/// produced. The observer is best suited to interfaces that report a running
/// action, while the result continues to retain the complete streams in the
/// configured cache.
pub async fn run_uncached_observed(
    action: &Action,
    workspace_root: &Path,
    cache: &CacheProvider,
    stream_to_parent: bool,
    observer: &dyn ActionOutputObserver,
) -> Result<ActionResult> {
    Box::pin(execute::run_observed(
        action,
        workspace_root,
        cache,
        stream_to_parent,
        false,
        Some(observer),
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

/// Restore outputs with caller-owned memory admission retained by physical work.
pub async fn materialize_outputs_admitted(
    result: &ActionResult,
    workspace_root: &Path,
    cache: &CacheProvider,
    lease: crate::ExecutionPermit,
) -> Result<()> {
    outputs::restore_admitted(result, workspace_root, cache, Some(lease)).await
}

pub async fn materialize_outputs(
    result: &ActionResult,
    workspace_root: &Path,
    cache: &CacheProvider,
) -> Result<()> {
    outputs::restore(result, workspace_root, cache).await
}

#[instrument(skip(action, cache, resources), fields(action_digest = %key))]
async fn lookup_cached(
    action: &Action,
    cache: &CacheProvider,
    workspace_root: &Path,
    key: &Digest,
    resources: Option<&ExecutorResources>,
) -> Result<Option<Outcome>> {
    if !action_uses_cache(action) {
        return Ok(None);
    }
    match lookup_cached_available(cache, workspace_root, key, resources).await {
        Err(crate::Error::Cas(error)) if error.is_unavailable() => {
            warn!(action_digest = %key, error = %error, "cache unavailable; executing the action instead of reusing it");
            Ok(None)
        }
        result => result,
    }
}

async fn lookup_cached_available(
    cache: &CacheProvider,
    workspace_root: &Path,
    key: &Digest,
    resources: Option<&ExecutorResources>,
) -> Result<Option<Outcome>> {
    if let Some(hit) = cache.lookup_action_result(key).await? {
        let result = hit.result;
        // A partially-evicted or partially-synced entry can have its
        // action result present while a blob it points at is gone. The
        // captured stdout/stderr blobs are not restored to the workspace
        // here, but downstream consumers read them straight from the CAS,
        // so a "hit" that references a missing stream blob would crash
        // them. Verify those blobs exist before reporting a hit, and if
        // either is gone treat the entry as absent and re-execute.
        if !hit.blobs_available && !stream_blobs_present(&result, cache).await? {
            warn!("cached action is missing a captured stdout/stderr blob; re-executing");
            return Ok(None);
        }
        let permit = match resources {
            Some(resources) if !result.outputs.is_empty() => {
                Some(resources.acquire_materialization().await)
            }
            _ => None,
        };
        match outputs::restore_admitted(&result, workspace_root, cache, permit).await {
            Ok(()) => {
                debug!("cache hit");
                return Ok(Some(Outcome {
                    action: *key,
                    result,
                    cache: CacheState::Hit,
                }));
            }
            // An action result survived but an output blob it references
            // is gone (evicted by `struo cache gc`, or never synced from a
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
    matches!(err, crate::Error::Cas(struo_cas::Error::BlobNotFound(_)))
}

#[instrument(skip(action, cache), fields(action_digest = %key))]
async fn store_result(
    action: &Action,
    cache: &CacheProvider,
    opts: RunOpts,
    key: Digest,
    result: ActionResult,
) -> Result<Outcome> {
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
mod tests;
