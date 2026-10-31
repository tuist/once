//! Put a deferred output back on disk when the cache no longer holds it.
//!
//! A cache hit leaves its outputs in the cache and answers "can this be
//! reused" when it is probed. The bytes still have to be there when a later
//! action, a dependent target, or the end of the build restores them, and
//! eviction or a later action replacing the only on-disk copy can take them
//! away in between. Each action that ran or hit in this invocation is
//! remembered as the producer of its outputs, so a restore the cache confirms
//! is impossible runs that producer again instead of failing the build.
//!
//! Confirmed absence or a classified cache outage can take that path, but
//! only a recorded producer can reconstruct the exact expected bytes. An
//! outage never proves absence. Corruption, refusals, and local errors propagate.
//!
//! A repair is owned by the registry, not by whoever asked for it first: it
//! runs as its own task, every caller observes the same terminal answer, and a
//! caller that goes away leaves it running. A repair that ends without an
//! answer, runs out of time, or would wait on itself through a chain of other
//! repairs fails, and is never attempted again in the same invocation, so a
//! partially executed producer is not silently run a second time. The chain
//! of repairs one restore may need and the invocation's total re-executions
//! are bounded too.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures::future::{BoxFuture, FutureExt as _, Shared};
use once_cas::{ActionResult, CacheProvider, Digest};
use once_core::{Action, ResourcePool, ResourceRequest, RunEvent, RunEventBus, TargetResult};

use super::legacy_outputs::{self, FingerprintKind};
use super::{cacheable_action_lock, restore_outputs, SourceDigestCache};

/// The attempt a repair reports: the producer's first attempt was the run or
/// cache hit that recorded it.
const REPAIR_ATTEMPT: u32 = 2;

/// How much repairing one invocation may do.
#[derive(Clone, Copy, Debug)]
pub(in crate::commands::graph::analysis) struct RepairLimits {
    /// Producers one restore may run again in a chain, counting the
    /// producers of a re-executed producer's own missing inputs.
    pub(in crate::commands::graph::analysis) depth: usize,
    /// Re-executions the invocation may spend in total.
    pub(in crate::commands::graph::analysis) executions: usize,
    /// How long one repair may take before it fails, when bounded. A producer
    /// may legitimately take as long as it took the first time, so this is
    /// unbounded unless a caller asks otherwise.
    pub(in crate::commands::graph::analysis) deadline: Option<Duration>,
}

impl Default for RepairLimits {
    fn default() -> Self {
        Self {
            depth: 8,
            executions: 64,
            deadline: None,
        }
    }
}

/// The digest a cache operation confirmed is absent from every tier, if that
/// is what `error` reports.
pub(in crate::commands::graph::analysis) fn confirmed_missing_blob(
    error: &anyhow::Error,
) -> Option<Digest> {
    error.chain().find_map(|cause| {
        if let Some(once_core::Error::Cas(once_cas::Error::BlobNotFound(digest))) =
            cause.downcast_ref::<once_core::Error>()
        {
            return Some(*digest);
        }
        match cause.downcast_ref::<once_cas::Error>() {
            Some(once_cas::Error::BlobNotFound(digest)) => Some(*digest),
            _ => None,
        }
    })
}

fn can_recompute_output(error: &anyhow::Error) -> bool {
    confirmed_missing_blob(error).is_some()
        || error.chain().any(|cause| {
            if let Some(once_core::Error::Cas(error)) = cause.downcast_ref::<once_core::Error>() {
                return error.is_unavailable();
            }
            cause
                .downcast_ref::<once_cas::Error>()
                .is_some_and(once_cas::Error::is_unavailable)
        })
}

/// How one action produced its outputs, retained so it can run again and be
/// reported as the same action when it does.
pub(in crate::commands::graph::analysis) struct Producer {
    pub(super) target_id: String,
    pub(super) capability: String,
    pub(super) index: usize,
    pub(super) identifier: String,
    pub(super) report: ProducerReport,
    pub(super) action: Action,
    /// Command setup run before the command, absent for built-in operations.
    pub(super) setup: Option<CommandSetup>,
    /// Outputs of other actions that must be on disk before it runs.
    pub(super) inputs: BTreeMap<String, Digest>,
    /// What it produced the time it was recorded.
    pub(super) outputs: BTreeMap<String, Digest>,
}

/// What the producer's own events carried, so its repair reports the same
/// action rather than unattributed work.
#[derive(Default)]
pub(in crate::commands::graph::analysis) struct ProducerReport {
    pub(super) reported: bool,
    pub(super) identifier: Option<String>,
    pub(super) display_name: Option<String>,
    pub(super) source_files: Vec<String>,
    pub(super) presentation: Option<once_core::ActionPresentation>,
    pub(super) history: Option<once_core::ActionHistoryKey>,
}

pub(in crate::commands::graph::analysis) struct CommandSetup {
    pub(super) arg_files: Vec<once_frontend::analysis::DeclaredArgFile>,
    pub(super) clean_paths: Vec<String>,
    pub(super) create_dirs: Vec<String>,
}

#[derive(Clone)]
struct FingerprintEvidence {
    kind: FingerprintKind,
    producer: String,
}

type RepairOutcome = std::result::Result<(), String>;

/// Where repairs report themselves.
struct Reporting {
    bus: RunEventBus,
    workers: crate::bus_events::WorkerSlots,
}

/// Producers of the outputs this invocation has seen, shared by every target.
#[derive(Clone)]
pub(in crate::commands::graph::analysis) struct Producers {
    inner: Arc<ProducersInner>,
}

struct ProducersInner {
    by_output: Mutex<HashMap<(String, Digest), Arc<Producer>>>,
    fingerprints: Mutex<HashMap<(String, Digest), FingerprintEvidence>>,
    /// One repair per producer action, kept after it finishes so its answer
    /// is the answer for the rest of the invocation.
    repairs: Mutex<HashMap<Digest, Shared<BoxFuture<'static, RepairOutcome>>>>,
    /// Which repair is waiting on which, to refuse a wait that closes a cycle.
    waits: Mutex<HashMap<Digest, HashSet<Digest>>>,
    remaining: AtomicUsize,
    limits: RepairLimits,
    reporting: OnceLock<Reporting>,
    executors: OnceLock<once_core::ExecutorResources>,
    execution: OnceLock<once_core::RemoteExecution>,
}

impl Default for Producers {
    fn default() -> Self {
        Self::with_limits(RepairLimits::default())
    }
}

impl Producers {
    pub(in crate::commands::graph::analysis) fn with_limits(limits: RepairLimits) -> Self {
        Self {
            inner: Arc::new(ProducersInner {
                by_output: Mutex::new(HashMap::new()),
                fingerprints: Mutex::new(HashMap::new()),
                repairs: Mutex::new(HashMap::new()),
                waits: Mutex::new(HashMap::new()),
                remaining: AtomicUsize::new(limits.executions),
                limits,
                reporting: OnceLock::new(),
                executors: OnceLock::new(),
                execution: OnceLock::new(),
            }),
        }
    }

    pub(in crate::commands::graph::analysis) fn set_execution(
        &self,
        execution: once_core::RemoteExecution,
    ) {
        self.inner
            .execution
            .set(execution)
            .expect("execution is configured before graph work");
    }

    pub(in crate::commands::graph::analysis) fn execution(
        &self,
    ) -> Option<&once_core::RemoteExecution> {
        self.inner.execution.get()
    }

    /// Report repairs on `bus`, taking lanes from `workers`. The first
    /// reporter registered for the invocation is kept.
    pub(in crate::commands::graph::analysis) fn report_to(
        &self,
        bus: Option<&RunEventBus>,
        workers: &crate::bus_events::WorkerSlots,
    ) {
        if let Some(bus) = bus {
            let _ = self.inner.reporting.set(Reporting {
                bus: bus.clone(),
                workers: workers.clone(),
            });
        }
    }

    pub(in crate::commands::graph::analysis) fn record(&self, producer: Producer) {
        if producer.outputs.is_empty() {
            return;
        }
        let producer = Arc::new(producer);
        let mut by_output = lock(&self.inner.by_output);
        for (path, digest) in &producer.outputs {
            by_output.insert((path.clone(), *digest), Arc::clone(&producer));
        }
    }

    pub(in crate::commands::graph::analysis) fn log_paths(&self) -> Vec<String> {
        lock(&self.inner.by_output)
            .values()
            .flat_map(|producer| match &producer.action {
                Action::RunCommand {
                    stdout_path,
                    stderr_path,
                    ..
                } => stdout_path
                    .iter()
                    .chain(stderr_path)
                    .map(|path| path.as_str().to_string())
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(in crate::commands::graph::analysis) fn record_fingerprint(
        &self,
        path: &str,
        digest: Digest,
        kind: FingerprintKind,
        producer: &str,
    ) {
        // Later raw files can equal manifest bytes while earlier readers still need the tree.
        lock(&self.inner.fingerprints)
            .entry((path.to_string(), digest))
            .or_insert_with(|| FingerprintEvidence {
                kind,
                producer: producer.to_string(),
            });
    }

    pub(super) async fn verify_fingerprints(
        &self,
        workspace: &Path,
        result: &ActionResult,
        lease: &once_core::ExecutionPermit,
    ) -> Result<Vec<String>> {
        let fingerprints = {
            let known = lock(&self.inner.fingerprints);
            result
                .outputs
                .iter()
                .filter_map(|(path, digest)| {
                    known
                        .get(&(path.clone(), *digest))
                        .map(|evidence| (path.clone(), *digest, evidence.clone()))
                })
                .collect::<Vec<_>>()
        };
        let mut verified = Vec::with_capacity(fingerprints.len());
        for (path, digest, evidence) in fingerprints {
            legacy_outputs::verify(workspace, &path, digest, evidence.kind, lease)
                .await
                .with_context(|| {
                    format!(
                        "verifying output `{path}` of uncached action `{}`",
                        evidence.producer
                    )
                })?;
            verified.push(path);
        }
        Ok(verified)
    }

    pub(in crate::commands::graph::analysis) fn has_fingerprint(
        &self,
        path: &str,
        digest: Digest,
    ) -> bool {
        lock(&self.inner.fingerprints).contains_key(&(path.to_string(), digest))
    }

    pub(in crate::commands::graph) fn executors(
        &self,
        host: &Arc<ResourcePool>,
    ) -> Result<&once_core::ExecutorResources> {
        let executors = self
            .inner
            .executors
            .get_or_init(|| once_core::ExecutorResources::new(Arc::clone(host)));
        anyhow::ensure!(
            Arc::ptr_eq(executors.host(), host),
            "graph admission pool changed after work started"
        );
        Ok(executors)
    }

    fn producer_of(&self, path: &str, digest: Digest) -> Option<Arc<Producer>> {
        lock(&self.inner.by_output)
            .get(&(path.to_string(), digest))
            .cloned()
    }

    fn spend_repair(&self) -> bool {
        self.inner
            .remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
    }

    /// Record that the repair of `waiter` waits on the repair of `awaited`,
    /// refusing the wait when `awaited` already waits on `waiter`.
    fn begin_wait(&self, waiter: Digest, awaited: Digest) -> Result<WaitEdge> {
        let mut waits = lock(&self.inner.waits);
        if waiter == awaited || reaches(&waits, awaited, waiter) {
            anyhow::bail!(
                "recovering action {waiter} needs action {awaited} to run again, which itself \
                 needs {waiter}; the recorded producers form a cycle"
            );
        }
        waits.entry(waiter).or_default().insert(awaited);
        Ok(WaitEdge {
            producers: self.clone(),
            waiter,
            awaited,
        })
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn reaches(waits: &HashMap<Digest, HashSet<Digest>>, from: Digest, to: Digest) -> bool {
    let mut stack = vec![from];
    let mut seen = HashSet::new();
    while let Some(next) = stack.pop() {
        if next == to {
            return true;
        }
        if seen.insert(next) {
            stack.extend(waits.get(&next).into_iter().flatten().copied());
        }
    }
    false
}

struct WaitEdge {
    producers: Producers,
    waiter: Digest,
    awaited: Digest,
}

impl Drop for WaitEdge {
    fn drop(&mut self) {
        let mut waits = lock(&self.producers.inner.waits);
        if let Some(awaited) = waits.get_mut(&self.waiter) {
            awaited.remove(&self.awaited);
            if awaited.is_empty() {
                waits.remove(&self.waiter);
            }
        }
    }
}

/// What restoring with repair needs to run a producer again.
#[derive(Clone, Copy)]
pub(in crate::commands::graph::analysis) struct Repair<'a> {
    pub(in crate::commands::graph::analysis) workspace: &'a Path,
    pub(in crate::commands::graph::analysis) cache: &'a CacheProvider,
    pub(in crate::commands::graph::analysis) source_digest_cache: Option<&'a SourceDigestCache>,
    pub(in crate::commands::graph::analysis) resources: &'a Arc<ResourcePool>,
    pub(in crate::commands::graph::analysis) producers: &'a Producers,
}

/// The same, owned, for a repair task that outlives the restore that
/// started it.
struct RepairEnv {
    workspace: PathBuf,
    cache: CacheProvider,
    source_digest_cache: Option<SourceDigestCache>,
    resources: Arc<ResourcePool>,
    producers: Producers,
}

impl RepairEnv {
    fn repair(&self) -> Repair<'_> {
        Repair {
            workspace: &self.workspace,
            cache: &self.cache,
            source_digest_cache: self.source_digest_cache.as_ref(),
            resources: &self.resources,
            producers: &self.producers,
        }
    }
}

impl Repair<'_> {
    /// Restore `result`, running the producer of any output whose bytes the
    /// cache confirms are gone.
    ///
    /// Must not be called while holding an admission permit: a producer that
    /// runs again needs one of its own.
    pub(in crate::commands::graph::analysis) async fn restore(
        &self,
        result: &ActionResult,
    ) -> Result<()> {
        self.restore_at(result, 0, None).await
    }

    fn owned(&self) -> RepairEnv {
        RepairEnv {
            workspace: self.workspace.to_path_buf(),
            cache: self.cache.clone(),
            source_digest_cache: self.source_digest_cache.cloned(),
            resources: Arc::clone(self.resources),
            producers: self.producers.clone(),
        }
    }

    async fn restore_attempt(&self, result: &ActionResult) -> Result<()> {
        let lease = self
            .producers
            .executors(self.resources)?
            .acquire_materialization()
            .await;
        let fingerprints = self
            .producers
            .verify_fingerprints(self.workspace, result, &lease)
            .await?;
        if fingerprints.is_empty() {
            return restore_outputs(
                self.workspace,
                self.cache,
                result,
                self.source_digest_cache,
                lease,
            )
            .await;
        }
        let mut restorable = result.clone();
        for path in fingerprints {
            restorable.outputs.remove(&path);
        }
        if restorable.outputs.is_empty() {
            return Ok(());
        }
        restore_outputs(
            self.workspace,
            self.cache,
            &restorable,
            self.source_digest_cache,
            lease,
        )
        .await
    }

    /// `waiter` is the repair this restore runs inside, if any.
    fn restore_at<'s>(
        &'s self,
        result: &'s ActionResult,
        depth: usize,
        waiter: Option<Digest>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 's>> {
        Box::pin(async move {
            let error = match self.restore_attempt(result).await {
                Ok(()) => return Ok(()),
                Err(error) => error,
            };
            if !can_recompute_output(&error) {
                return Err(error);
            }
            for (path, digest) in &result.outputs {
                let single = single_output(path, *digest);
                let error = match self.restore_attempt(&single).await {
                    Ok(()) => continue,
                    Err(error) => error,
                };
                if !can_recompute_output(&error) {
                    return Err(error);
                }
                let missing = confirmed_missing_blob(&error);
                let Some(producer) = self.producers.producer_of(path, *digest) else {
                    return Err(error.context(format!(
                        "output `{path}` could not be restored and no action in this build \
                         recorded producing it; run the build again to produce it"
                    )));
                };
                tracing::warn!(
                    output = %path,
                    blob = %digest,
                    confirmed_missing = missing.is_some(),
                    error = %error,
                    target = %producer.target_id,
                    action_index = producer.index,
                    identifier = %producer.identifier,
                    depth,
                    "deferred output could not be restored; running its recorded producer again"
                );
                self.repair(&producer, depth, waiter)
                    .await
                    .with_context(|| {
                        format!(
                            "running action {} for {} ({}) again to recover `{path}`",
                            producer.index, producer.target_id, producer.identifier
                        )
                    })?;
                self.restore_attempt(&single)
                    .await
                    .with_context(|| format!("restoring `{path}` after running its producer"))?;
            }
            Ok(())
        })
    }

    async fn repair(
        &self,
        producer: &Arc<Producer>,
        depth: usize,
        waiter: Option<Digest>,
    ) -> Result<()> {
        let limits = self.producers.inner.limits;
        if depth >= limits.depth {
            anyhow::bail!(
                "recovering missing outputs needed more than {} producers in a chain",
                limits.depth
            );
        }
        let digest = producer.action.digest();
        let _edge = waiter
            .map(|waiter| self.producers.begin_wait(waiter, digest))
            .transpose()?;
        let repair = lock(&self.producers.inner.repairs)
            .entry(digest)
            .or_insert_with(|| self.spawn_repair(Arc::clone(producer), depth))
            .clone();
        repair.await.map_err(anyhow::Error::msg)
    }

    fn spawn_repair(
        &self,
        producer: Arc<Producer>,
        depth: usize,
    ) -> Shared<BoxFuture<'static, RepairOutcome>> {
        let env = self.owned();
        let digest = producer.action.digest();
        let task = tokio::spawn(async move {
            let repair = env.repair();
            if !repair.producers.spend_repair() {
                return Err(format!(
                    "this build already ran {} actions again to recover missing cache content; \
                     the cache is losing content faster than it can be repaired",
                    repair.producers.inner.limits.executions
                ));
            }
            let rerun = repair.rerun(&producer, depth);
            let outcome = match repair.producers.inner.limits.deadline {
                Some(deadline) => match tokio::time::timeout(deadline, rerun).await {
                    Ok(outcome) => outcome,
                    Err(_) => Err(anyhow::anyhow!(
                        "running the producer again did not finish within {deadline:?}"
                    )),
                },
                None => rerun.await,
            };
            outcome.map_err(|error| format!("{error:#}"))
        });
        async move {
            task.await.unwrap_or_else(|error| {
                Err(format!(
                    "running action {digest} again ended without an answer ({error}); it may \
                     have partially run, so it is not attempted again"
                ))
            })
        }
        .boxed()
        .shared()
    }

    async fn rerun(&self, producer: &Producer, depth: usize) -> Result<()> {
        let digest = producer.action.digest();
        if !producer.inputs.is_empty() {
            self.restore_at(
                &ActionResult {
                    exit_code: 0,
                    stdout: None,
                    stderr: None,
                    outputs: producer.inputs.clone(),
                },
                depth + 1,
                Some(digest),
            )
            .await
            .context("restoring the producer's inputs")?;
        }
        let reporting = self.producers.inner.reporting.get();
        let result = with_action_lock_and_permit(
            self.workspace,
            digest,
            self.producers.executors(self.resources)?,
            once_core::Executor::resolve(&producer.action)?,
            producer.action.resource_request().clone(),
            |_contended, lease| async move {
                let attempt = match reporting {
                    Some(reporting) => Some(AttemptReport::start(reporting, producer).await),
                    None => None,
                };
                let outcome = async {
                    let setup = producer.setup.as_ref();
                    if let Some(setup) = setup {
                        if once_core::Executor::resolve(&producer.action)?
                            == once_core::Executor::Local
                        {
                            super::prepare_command_setup_admitted(
                                self.workspace,
                                self.cache,
                                &setup.clean_paths,
                                &setup.create_dirs,
                                &lease,
                            )
                            .await?;
                        }
                        super::preparation::write_args(self.workspace, &setup.arg_files, &lease)
                            .await?;
                    }
                    let result = super::provider::execute(
                        &producer.action,
                        self.workspace,
                        self.cache,
                        None,
                        lease,
                        setup.map_or(&[], |setup| setup.clean_paths.as_slice()),
                        setup.map_or(&[], |setup| setup.create_dirs.as_slice()),
                        setup.map_or(&[], |setup| setup.arg_files.as_slice()),
                    )
                    .await?;
                    if result.exit_code == 0 {
                        self.cache.put_action_result(&digest, &result).await?;
                    }
                    Ok::<_, anyhow::Error>(result)
                }
                .await;
                if let Some(attempt) = attempt {
                    attempt.finish(
                        producer,
                        digest,
                        outcome.as_ref().map_or(1, |result| result.exit_code),
                    );
                }
                outcome
            },
        )
        .await?;
        if result.exit_code != 0 {
            anyhow::bail!("the producer exited with status {}", result.exit_code);
        }
        if let Some(cache) = self.source_digest_cache {
            cache.record_outputs(&result, self.workspace);
        }
        let changed = producer
            .outputs
            .iter()
            .filter(|(path, digest)| result.outputs.get(*path) != Some(digest))
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>();
        if !changed.is_empty() {
            anyhow::bail!(
                "the producer wrote different bytes than its cached result for {}; \
                 consumers were keyed on the cached bytes, so run the build again",
                changed.join(", ")
            );
        }
        Ok(())
    }
}

/// The events of one repair attempt, reported as another attempt of the
/// producer's own action with its original identity, presentation, history,
/// and source files.
struct AttemptReport<'a> {
    bus: &'a RunEventBus,
    worker: crate::bus_events::WorkerGuard,
    started_at_ms: i64,
    started: Instant,
}

impl<'a> AttemptReport<'a> {
    async fn start(reporting: &'a Reporting, producer: &Producer) -> Self {
        let worker = reporting.workers.acquire().await;
        let started_at_ms = crate::bus_events::now_ms();
        if producer.report.reported {
            reporting.bus.publish(RunEvent::ActionAttemptStarted {
                at_epoch_ms: started_at_ms,
                target_id: producer.target_id.clone(),
                capability: producer.capability.clone(),
                action_index: action_index(producer),
                attempt: REPAIR_ATTEMPT,
                worker_id: worker.worker_id().to_string(),
            });
        }
        Self {
            bus: &reporting.bus,
            worker,
            started_at_ms,
            started: Instant::now(),
        }
    }

    fn finish(self, producer: &Producer, digest: Digest, exit_code: i32) {
        // Once's own file plumbing is reported only when it fails, as on its
        // first attempt.
        if !producer.report.reported && exit_code == 0 {
            return;
        }
        let duration_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.bus.publish(RunEvent::ActionAttemptCompleted {
            at_epoch_ms: crate::bus_events::now_ms(),
            target_id: producer.target_id.clone(),
            capability: producer.capability.clone(),
            action_index: action_index(producer),
            attempt: REPAIR_ATTEMPT,
            result: if exit_code == 0 {
                TargetResult::Succeeded
            } else {
                TargetResult::Failed
            },
            exit_code,
            duration_ms: i64::try_from(duration_ms).unwrap_or(i64::MAX),
            was_cached: false,
        });
        let report = &producer.report;
        crate::bus_events::action_completed(
            self.bus,
            &producer.target_id,
            &producer.capability,
            action_index(producer),
            report.identifier.as_deref(),
            report.display_name.as_deref(),
            &report.source_files,
            report.presentation.as_ref(),
            report.history.as_ref(),
            duration_ms,
            false,
            exit_code,
            self.started_at_ms,
            self.worker.worker_id(),
            0,
            i64::try_from(duration_ms).unwrap_or(i64::MAX),
            &digest.to_string(),
            REPAIR_ATTEMPT,
        );
    }
}

fn action_index(producer: &Producer) -> u32 {
    u32::try_from(producer.index).unwrap_or(u32::MAX)
}

fn single_output(path: &str, digest: Digest) -> ActionResult {
    ActionResult {
        exit_code: 0,
        stdout: None,
        stderr: None,
        outputs: BTreeMap::from([(path.to_string(), digest)]),
    }
}

/// Hold an action's lock and an admission permit together, never waiting for
/// one while holding the other.
///
/// Waiting for admission while holding the lock would stall every other
/// process waiting for the same action behind this one's queue position;
/// waiting for the lock while holding a permit would pin capacity behind work
/// that may already be done. `work` learns whether another holder was seen, in
/// which case the action may have finished in the meantime.
pub(super) async fn with_action_lock_and_permit<T, F, Fut>(
    workspace: &Path,
    digest: Digest,
    resources: &once_core::ExecutorResources,
    executor: once_core::Executor,
    request: ResourceRequest,
    work: F,
) -> Result<T>
where
    F: FnOnce(bool, once_core::ExecutionPermit) -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let lock = Arc::new(cacheable_action_lock(workspace, digest)?.into_inner());
    let mut work = Some(work);
    let mut contended = false;
    loop {
        let permit = resources.acquire(executor, &request).await;
        match lock.try_lock() {
            Ok(()) => {
                let permit = permit.with_lifetime_owner(Arc::clone(&lock));
                let work = work.take().context("action work already ran")?;
                return work(contended, permit).await;
            }
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(error) => {
                return Err(anyhow::Error::msg(error.to_string()))
                    .with_context(|| format!("locking action {digest}"));
            }
        }
        drop(permit);
        contended = true;
        wait_for_action_lock(&lock, digest).await?;
    }
}

async fn wait_for_action_lock(lock: &std::fs::File, digest: Digest) -> Result<()> {
    loop {
        match lock.try_lock() {
            Ok(()) => {
                lock.unlock()
                    .with_context(|| format!("unlocking action {digest}"))?;
                return Ok(());
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => {
                return Err(anyhow::Error::msg(error.to_string()))
                    .with_context(|| format!("locking action {digest}"));
            }
        }
    }
}

#[cfg(test)]
mod tests;
