//! Always-on producers for [`RunEventBus`] events.
//!
//! Historically only the `--ui` HTTP dashboard published lifecycle
//! events onto the bus, so nothing else (a terminal renderer, the RFC
//! 0008 ingest, an in-process sound module that wants to subscribe)
//! could observe them. This module provides small helpers that a
//! command's hot path can call unconditionally to broadcast the same
//! events the dashboard used to emit, plus a lightweight
//! [`ActionOutputObserver`] that only pushes `LogChunk` events onto
//! the bus (with no channel or per-run UI store).
//!
//! The UI dashboard still exists; when it is enabled, its
//! [`Publisher`](crate::commands::ui::Publisher) additionally updates a
//! rendering store from these events, but the bus emit itself moves
//! here so it fires whether or not the dashboard is on.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use once_core::{
    ActionOutputObserver, ActionOutputStream, LogStream, Phase, RunEvent, RunEventBus, TargetResult,
};

/// Publish `RunStarted` and `TargetQueued`. Idempotent from the caller's
/// perspective; the bus itself is a broadcast channel that silently
/// succeeds when nothing is subscribed.
/// Spawn a background task that samples host CPU, memory, and network
/// once per second and publishes each sample on the run event bus.
/// Returns a handle whose drop stops the sampler and joins the task.
///
/// Sampling uses `sysinfo` at whole-host granularity. CPU is averaged
/// across cores (so full load on a 4-core machine reads as 100%),
/// memory is `used_memory`, and network is the total bytes-per-second
/// delta since the previous sample summed across all interfaces.
pub fn spawn_system_sampler(bus: &once_core::RunEventBus) -> SystemSamplerHandle {
    let bus = bus.clone();
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();

    let handle = tokio::spawn(async move {
        let mut system = sysinfo::System::new_with_specifics(
            sysinfo::RefreshKind::nothing()
                .with_cpu(sysinfo::CpuRefreshKind::nothing().with_cpu_usage())
                .with_memory(sysinfo::MemoryRefreshKind::nothing().with_ram()),
        );
        let mut networks = sysinfo::Networks::new_with_refreshed_list();

        // sysinfo's CPU usage needs two refreshes to compute a delta.
        system.refresh_cpu_usage();
        tokio::time::sleep(std::time::Duration::from_millis(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.as_millis() as u64 + 50)).await;

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    system.refresh_cpu_usage();
                    system.refresh_memory();
                    networks.refresh(true);

                    let cpu_percent = {
                        let cpus = system.cpus();
                        if cpus.is_empty() {
                            0.0
                        } else {
                            cpus.iter().map(|cpu| cpu.cpu_usage()).sum::<f32>() / cpus.len() as f32
                        }
                    };
                    let memory_bytes = system.used_memory();
                    let (network_in, network_out) = networks
                        .iter()
                        .fold((0u64, 0u64), |(inc, out), (_name, data)| {
                            (inc + data.received(), out + data.transmitted())
                        });

                    bus.publish(RunEvent::SystemSampled {
                        at_epoch_ms: now_epoch_ms(),
                        cpu_percent,
                        memory_bytes,
                        network_in_bytes_per_second: network_in,
                        network_out_bytes_per_second: network_out,
                    });
                }
                _ = &mut shutdown_rx => break,
            }
        }
    });

    SystemSamplerHandle {
        shutdown: Some(shutdown_tx),
        handle: Some(handle),
    }
}

/// Handle to a spawned system sampler task. Dropping it (or calling
/// `stop`) signals the sampler to exit and joins the task.
pub struct SystemSamplerHandle {
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl SystemSamplerHandle {
    /// Signal the sampler to exit and wait for it to drain.
    pub async fn stop(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }
}

impl Drop for SystemSamplerHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

/// A bounded pool of executor slots that carry stable `worker_id`s
/// across the whole run. Each slot is a peer of Bazel's per-thread
/// Chrome-Trace `tid`: a durable worker identity that many actions
/// rotate through. Sized to match the runner's action-parallelism
/// ceiling so the pool never over- or under-provisions relative to
/// what's actually allowed to run concurrently.
///
/// Also the natural seam for remote execution later: a remote worker
/// is just another slot backed by a remote executor. Local slots
/// carry `worker_kind: "local"` today; remote slots will carry
/// `"remote"` once that lands.
#[derive(Clone)]
pub struct WorkerSlots {
    sender: tokio::sync::mpsc::UnboundedSender<String>,
    receiver: std::sync::Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<String>>>,
}

impl WorkerSlots {
    /// Build a pool of `size` slots. `size` is clamped to at least one
    /// so `acquire` always eventually returns even under degenerate
    /// configuration.
    pub fn new(size: usize) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let n = size.max(1);
        for i in 0..n {
            let _ = sender.send(format!("worker-{i}"));
        }
        Self {
            sender,
            receiver: std::sync::Arc::new(tokio::sync::Mutex::new(receiver)),
        }
    }

    /// Await a free slot and take ownership of it for the duration of
    /// the returned guard. Slot ids are returned to the pool when the
    /// guard is dropped, so the next waiter picks them up in order.
    pub async fn acquire(&self) -> WorkerGuard {
        let id = {
            let mut rx = self.receiver.lock().await;
            rx.recv()
                .await
                .expect("worker slot channel is never closed by the pool")
        };
        WorkerGuard {
            id,
            release: self.sender.clone(),
        }
    }
}

/// RAII guard for a slot claimed from [`WorkerSlots`]. Read the id with
/// [`WorkerGuard::worker_id`]; the slot returns to the pool on drop.
pub struct WorkerGuard {
    id: String,
    release: tokio::sync::mpsc::UnboundedSender<String>,
}

impl WorkerGuard {
    pub fn worker_id(&self) -> &str {
        &self.id
    }
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        let _ = self.release.send(std::mem::take(&mut self.id));
    }
}

/// Emit a completed span for one of the coarse target-level phases
/// (analysis, materialize_outputs, ...) that consume most of the wall
/// clock on cache-hit builds but never show up as declared actions.
/// Rides the existing `ActionCompleted` shape so it lands on the same
/// worker lane in the flame graph; the server projector treats a
/// capability starting with `_phase` as a synthetic span that
/// doesn't roll up into the run's action counters.
pub fn target_phase_completed(
    bus: &RunEventBus,
    target_id: &str,
    phase: &str,
    worker_id: &str,
    start_at_epoch_ms: i64,
    duration_ms: u64,
) {
    bus.publish(RunEvent::ActionCompleted {
        at_epoch_ms: now_epoch_ms(),
        start_at_epoch_ms,
        worker_id: worker_id.to_string(),
        target_id: target_id.to_string(),
        capability: "_phase".to_string(),
        action_index: 0,
        identifier: Some(phase.to_string()),
        result: TargetResult::Succeeded,
        was_cached: false,
        duration_ms: i64::try_from(duration_ms).unwrap_or(i64::MAX),
        exit_code: 0,
        prepare_ms: 0,
        execute_ms: 0,
        cache_key: String::new(),
    });
}

/// Emit one content-blob transfer against the cache. Kind is
/// "download" (blob pulled into the workspace from the cache) or
/// "upload" (blob pushed from the workspace back into the cache).
/// The server projects these into `once_cache_events` so the Cache
/// tab's Content Objects view lists real digests + sizes and the
/// summary widgets can total `content_downloaded` / `content_uploaded`.
pub fn cache_content_transferred(
    bus: &RunEventBus,
    kind: &str,
    target_id: &str,
    content_hash: &str,
    size_bytes: u64,
    duration_ms: u64,
) {
    bus.publish(RunEvent::CacheContentTransferred {
        at_epoch_ms: now_epoch_ms(),
        kind: kind.to_string(),
        target_id: target_id.to_string(),
        content_hash: content_hash.to_string(),
        size_bytes: i64::try_from(size_bytes).unwrap_or(i64::MAX),
        duration_ms: i64::try_from(duration_ms).unwrap_or(i64::MAX),
    });
}

pub fn run_started(bus: &RunEventBus, target_id: &str, at_epoch_ms: i64) {
    bus.publish(RunEvent::RunStarted { at_epoch_ms });
    bus.publish(RunEvent::TargetQueued {
        at_epoch_ms,
        target_id: target_id.to_string(),
    });
}

pub fn target_cache_checking(bus: &RunEventBus, target_id: &str) {
    bus.publish(RunEvent::TargetPhase {
        at_epoch_ms: now_epoch_ms(),
        target_id: target_id.to_string(),
        phase: Phase::CacheChecking,
    });
}

pub fn target_preparing(bus: &RunEventBus, target_id: &str) {
    bus.publish(RunEvent::TargetPhase {
        at_epoch_ms: now_epoch_ms(),
        target_id: target_id.to_string(),
        phase: Phase::Preparing,
    });
}

/// Fire `TargetStarted` followed by `TargetPhase(Executing)` for a
/// target that has begun running its action.
pub fn target_executing(bus: &RunEventBus, target_id: &str) {
    let at = now_epoch_ms();
    bus.publish(RunEvent::TargetStarted {
        at_epoch_ms: at,
        target_id: target_id.to_string(),
    });
    bus.publish(RunEvent::TargetPhase {
        at_epoch_ms: at,
        target_id: target_id.to_string(),
        phase: Phase::Executing,
    });
}

pub fn target_capturing(bus: &RunEventBus, target_id: &str) {
    bus.publish(RunEvent::TargetPhase {
        at_epoch_ms: now_epoch_ms(),
        target_id: target_id.to_string(),
        phase: Phase::Capturing,
    });
}

pub fn target_publishing(bus: &RunEventBus, target_id: &str) {
    bus.publish(RunEvent::TargetPhase {
        at_epoch_ms: now_epoch_ms(),
        target_id: target_id.to_string(),
        phase: Phase::Publishing,
    });
}

/// Publish `RunCompleted` for the whole run. Used at the outer run
/// boundary when the scheduler already published a per-target
/// `TargetCompleted` for the top-level target and only the terminal
/// run event is missing.
pub fn run_completed(bus: &RunEventBus, exit_code: i32) {
    bus.publish(RunEvent::RunCompleted {
        at_epoch_ms: now_epoch_ms(),
        exit_status: exit_code,
    });
}

/// Publish `TargetCompleted` for one target with no `RunCompleted`.
/// Used by the graph scheduler which fires many of these as
/// dependencies finish; the outer run publishes its own `RunCompleted`
/// via [`target_finished`] or [`run_completed`].
pub fn target_completed(
    bus: &RunEventBus,
    target_id: &str,
    duration_ms: u64,
    was_cached: bool,
    exit_code: i32,
) {
    let result = if exit_code == 0 {
        TargetResult::Succeeded
    } else {
        TargetResult::Failed
    };
    bus.publish(RunEvent::TargetCompleted {
        at_epoch_ms: now_epoch_ms(),
        target_id: target_id.to_string(),
        result,
        was_cached,
        duration_ms: i64::try_from(duration_ms).unwrap_or(i64::MAX),
    });
}

/// Publish `TargetCompleted` for a target that finished (any outcome)
/// followed by `RunCompleted` for the whole run. The cache label is
/// interpreted case-insensitively: `hit` maps to `was_cached=true`.
pub fn target_finished(
    bus: &RunEventBus,
    target_id: &str,
    duration_ms: u64,
    cache: &str,
    exit_code: i32,
) {
    let result = if exit_code == 0 {
        TargetResult::Succeeded
    } else {
        TargetResult::Failed
    };
    bus.publish(RunEvent::TargetCompleted {
        at_epoch_ms: now_epoch_ms(),
        target_id: target_id.to_string(),
        result,
        was_cached: cache.eq_ignore_ascii_case("hit"),
        duration_ms: i64::try_from(duration_ms).unwrap_or(i64::MAX),
    });
    bus.publish(RunEvent::RunCompleted {
        at_epoch_ms: now_epoch_ms(),
        exit_status: exit_code,
    });
}

/// Publish `ActionCompleted` for one declared action inside a target.
///
/// Called as each declared action finishes, including cache restoration.
/// Reused targets replay the identities of their retained actions as hits.
#[allow(clippy::too_many_arguments)]
pub fn action_completed(
    bus: &RunEventBus,
    target_id: &str,
    capability: &str,
    action_index: u32,
    identifier: Option<&str>,
    duration_ms: u64,
    was_cached: bool,
    exit_code: i32,
    // Captured by the caller at the point the action actually
    // starts (right before dispatching to the executor). Passing 0
    // means "unknown"; the server projector then derives the start
    // from `envelope.epoch_ms - duration_ms`, which is inaccurate
    // when the reporter drains events long after emission.
    start_at_epoch_ms: i64,
    // The stable id of the executor slot that ran this action, from a
    // [`WorkerSlots`] guard. Bazel's Chrome-Trace uses `tid` the same
    // way, so the server can lay one lane per worker and stack the
    // actions each worker ran end-to-end down its column.
    worker_id: &str,
    // Wall time spent on setup (arg files, input fingerprinting,
    // action digest) before the cache probe. Zero when unknown
    // (cached replay).
    prepare_ms: i64,
    // Wall time spent inside cache probe + (on a miss) command
    // execution and output upload. Zero when unknown (cached replay).
    execute_ms: i64,
    // Hex digest of the action's cache lookup key (Bazel calls this
    // `action_digest`). Empty when unknown (a failure that couldn't
    // even compute one).
    cache_key: &str,
) {
    let result = if exit_code == 0 {
        TargetResult::Succeeded
    } else {
        TargetResult::Failed
    };
    let finished_ms = now_epoch_ms();
    bus.publish(RunEvent::ActionCompleted {
        at_epoch_ms: finished_ms,
        start_at_epoch_ms,
        worker_id: worker_id.to_string(),
        target_id: target_id.to_string(),
        capability: capability.to_string(),
        action_index,
        identifier: identifier.map(str::to_string),
        result,
        was_cached,
        duration_ms: i64::try_from(duration_ms).unwrap_or(i64::MAX),
        exit_code,
        prepare_ms,
        execute_ms,
        cache_key: cache_key.to_string(),
    });
}

/// Publish `TargetCompleted{Failed}` and `RunCompleted{1}` for a run
/// that couldn't even reach the action stage (setup failure).
pub fn target_failed(bus: &RunEventBus, target_id: &str, duration_ms: u64) {
    bus.publish(RunEvent::TargetCompleted {
        at_epoch_ms: now_epoch_ms(),
        target_id: target_id.to_string(),
        result: TargetResult::Failed,
        was_cached: false,
        duration_ms: i64::try_from(duration_ms).unwrap_or(i64::MAX),
    });
    bus.publish(RunEvent::RunCompleted {
        at_epoch_ms: now_epoch_ms(),
        exit_status: 1,
    });
}

/// A lightweight observer that only publishes `LogChunk` events onto
/// the bus. Unlike the UI dashboard's observer, it never queues text
/// into a decoder or channel; the terminal reporter and any other
/// subscriber decode as they render.
pub struct BusOutputObserver {
    bus: RunEventBus,
    target_id: String,
}

impl BusOutputObserver {
    #[must_use]
    pub fn new(bus: RunEventBus, target_id: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            bus,
            target_id: target_id.into(),
        })
    }
}

impl ActionOutputObserver for BusOutputObserver {
    fn observe(&self, stream: ActionOutputStream, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.bus.publish(RunEvent::LogChunk {
            at_epoch_ms: now_epoch_ms(),
            target_id: self.target_id.clone(),
            stream: match stream {
                ActionOutputStream::Stdout => LogStream::Stdout,
                ActionOutputStream::Stderr => LogStream::Stderr,
            },
            bytes: bytes.to_vec(),
        });
    }
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

pub fn now_ms() -> i64 {
    now_epoch_ms()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn helpers_fire_expected_events() {
        let bus = RunEventBus::new(16);
        let mut rx = bus.subscribe();
        run_started(&bus, "//foo:bar", 42);
        target_executing(&bus, "//foo:bar");
        target_finished(&bus, "//foo:bar", 10, "miss", 0);

        assert!(matches!(
            rx.recv().await.unwrap(),
            RunEvent::RunStarted { at_epoch_ms: 42 }
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            RunEvent::TargetQueued { target_id, .. } if target_id == "//foo:bar"
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            RunEvent::TargetStarted { target_id, .. } if target_id == "//foo:bar"
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            RunEvent::TargetPhase {
                phase: Phase::Executing,
                ..
            }
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            RunEvent::TargetCompleted {
                result: TargetResult::Succeeded,
                was_cached: false,
                ..
            }
        ));
        assert!(matches!(
            rx.recv().await.unwrap(),
            RunEvent::RunCompleted { exit_status: 0, .. }
        ));
    }

    #[tokio::test]
    async fn bus_output_observer_publishes_log_chunks() {
        let bus = RunEventBus::new(8);
        let mut rx = bus.subscribe();
        let observer = BusOutputObserver::new(bus.clone(), "//x:y");
        observer.observe(ActionOutputStream::Stdout, b"hello");

        match rx.recv().await.unwrap() {
            RunEvent::LogChunk {
                target_id,
                stream,
                bytes,
                ..
            } => {
                assert_eq!(target_id, "//x:y");
                assert_eq!(stream, LogStream::Stdout);
                assert_eq!(bytes, b"hello");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn empty_chunk_is_ignored() {
        let bus = RunEventBus::new(4);
        let mut rx = bus.subscribe();
        let observer = BusOutputObserver::new(bus.clone(), "//x:y");
        observer.observe(ActionOutputStream::Stdout, b"");
        // Nothing published; try_recv would immediately error.
        assert!(rx.try_recv().is_err());
    }
}
