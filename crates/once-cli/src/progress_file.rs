//! Machine-readable progress for whoever launched Once, such as a coding
//! agent's host following a build in a sandbox.
//!
//! When `ONCE_PROGRESS` names a file, a build or test run appends one JSON
//! object per line to it: a snapshot of the run so far, written when it
//! changed and at most once a second, plus a final one when the run ends.
//! A reader only needs the last complete line. The snapshot is
//! ecosystem-neutral and versioned by `v`:
//!
//! ```json
//! {"v":1,"command":"build","target":"//app:app","state":"running",
//!  "planned":40,"done":12,"cached":9,"failed":0,"running":["//lib:a"],
//!  "tests":{"passed":0,"failed":0,"skipped":0},"elapsed_ms":5321}
//! ```
//!
//! `planned` is zero until a scheduler planned its targets. `state` is
//! `running`, `succeeded` or `failed`. A scheduled `once test` without a
//! single target writes one snapshot for the whole schedule, where each
//! target is a test batch; its batches write none of their own. Writing never
//! fails the run: an unwritable file disables the sink with a debug log.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use once_core::{RunEvent, RunEventBus, TargetResult, TestCaseResult};
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

/// The environment variable naming the progress file.
pub const ENV: &str = "ONCE_PROGRESS";

const INTERVAL: Duration = Duration::from_secs(1);
const MAX_RUNNING: usize = 5;

/// A running progress sink. Await [`Self::finish`] after the run's
/// `RunCompleted` so the final snapshot is written.
pub struct ProgressFile {
    handle: JoinHandle<()>,
}

impl ProgressFile {
    /// Subscribes to `bus` when `ONCE_PROGRESS` is set and non-empty.
    pub fn spawn(bus: &RunEventBus, command: &str, target: &str) -> Option<Self> {
        Some(Self::spawn_at(bus, path_from_env()?, command, target))
    }

    fn spawn_at(bus: &RunEventBus, path: PathBuf, command: &str, target: &str) -> Self {
        let mut receiver = bus.subscribe();
        let mut state = State::new(command, target);
        let handle = tokio::spawn(async move {
            let mut written_at: Option<Instant> = None;
            let mut dirty = true;
            let mut tick = tokio::time::interval(INTERVAL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    event = receiver.recv() => match event {
                        Ok(event) => {
                            let finished = state.apply(event);
                            dirty = true;
                            if finished {
                                break;
                            }
                        }
                        Err(RecvError::Lagged(_)) => state.lagged = true,
                        Err(RecvError::Closed) => break,
                    },
                    _ = tick.tick() => {}
                }
                let due = written_at.is_none_or(|at| at.elapsed() >= INTERVAL);
                if dirty && due {
                    if !append_off_runtime(&path, state.snapshot()).await {
                        return;
                    }
                    written_at = Some(Instant::now());
                    dirty = false;
                }
            }
            append_off_runtime(&path, state.snapshot()).await;
        });
        Self { handle }
    }

    /// Waits for the final snapshot, which follows the run's `RunCompleted`.
    pub async fn finish(self) {
        let _ = self.handle.await;
    }
}

fn path_from_env() -> Option<PathBuf> {
    std::env::var_os(ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Appends on the blocking pool so a slow disk never stalls the event loop
/// and makes the sink miss events.
async fn append_off_runtime(path: &Path, snapshot: serde_json::Value) -> bool {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || append(&path, &snapshot))
        .await
        .unwrap_or(false)
}

/// Progress for a scheduled `once test`, written from its worker threads.
/// Each batch counts as one planned target.
pub struct ScheduleProgress {
    path: PathBuf,
    inner: Mutex<ScheduleInner>,
}

struct ScheduleInner {
    state: State,
    written_at: Option<Instant>,
    disabled: bool,
}

impl ScheduleProgress {
    /// Starts when `ONCE_PROGRESS` is set and non-empty.
    pub fn start(batches: usize) -> Option<Self> {
        Some(Self::start_at(path_from_env()?, batches))
    }

    fn start_at(path: PathBuf, batches: usize) -> Self {
        let mut state = State::new("test", "");
        state.planned = u64::try_from(batches).unwrap_or(u64::MAX);
        let progress = Self {
            path,
            inner: Mutex::new(ScheduleInner {
                state,
                written_at: None,
                disabled: false,
            }),
        };
        progress.write(false);
        progress
    }

    pub fn batch_started(&self, target: &str) {
        self.update(|state| state.start(target));
        self.write(false);
    }

    /// Records a finished batch with its normalized test summary, if any.
    pub fn batch_finished(
        &self,
        target: &str,
        succeeded: bool,
        cached: bool,
        summary: Option<&serde_json::Value>,
    ) {
        self.update(|state| {
            let result = if succeeded {
                TargetResult::Succeeded
            } else {
                TargetResult::Failed
            };
            state.complete(target, result, cached);
            if let Some(summary) = summary {
                let count = |field: &str| summary[field].as_u64().unwrap_or(0);
                state.passed_cases += count("passed");
                state.failed_cases += count("failed");
                state.skipped_cases += count("skipped");
            }
        });
        self.write(false);
    }

    pub fn finish(&self, succeeded: bool) {
        self.update(|state| {
            state.outcome = Some(succeeded);
            state.running.clear();
        });
        self.write(true);
    }

    fn update(&self, change: impl FnOnce(&mut State)) {
        if let Ok(mut inner) = self.inner.lock() {
            change(&mut inner.state);
        }
    }

    #[cfg(test)]
    fn finish_snapshot_for_test(&self) {
        self.write(true);
    }

    fn write(&self, force: bool) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let due = force || inner.written_at.is_none_or(|at| at.elapsed() >= INTERVAL);
        if inner.disabled || !due {
            return;
        }
        inner.disabled = !append(&self.path, &inner.state.snapshot());
        inner.written_at = Some(Instant::now());
    }
}

fn append(path: &Path, snapshot: &serde_json::Value) -> bool {
    let line = format!("{snapshot}\n");
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(line.as_bytes()));
    if let Err(error) = &result {
        tracing::debug!(%error, path = %path.display(), "progress file is not writable");
    }
    result.is_ok()
}

struct State {
    command: String,
    target: String,
    started: Instant,
    outcome: Option<bool>,
    planned: u64,
    done: u64,
    cached: u64,
    failed: u64,
    /// Running targets with how many runs of each are in flight; scheduled
    /// test batches can share a target.
    running: BTreeMap<String, u32>,
    /// The sink missed events, so counts can fall short of what ran.
    lagged: bool,
    passed_cases: u64,
    failed_cases: u64,
    skipped_cases: u64,
}

impl State {
    fn new(command: &str, target: &str) -> Self {
        Self {
            command: command.to_string(),
            target: target.to_string(),
            started: Instant::now(),
            outcome: None,
            planned: 0,
            done: 0,
            cached: 0,
            failed: 0,
            running: BTreeMap::new(),
            lagged: false,
            passed_cases: 0,
            failed_cases: 0,
            skipped_cases: 0,
        }
    }

    /// Returns whether the run ended.
    fn apply(&mut self, event: RunEvent) -> bool {
        match event {
            RunEvent::TargetsPlanned { count, .. } => self.planned += u64::from(count),
            RunEvent::TargetStarted { target_id, .. } => self.start(&target_id),
            RunEvent::TargetCompleted {
                target_id,
                result,
                was_cached,
                ..
            } => self.complete(&target_id, result, was_cached),
            RunEvent::TestCaseCompleted { result, .. } => match result {
                TestCaseResult::Passed => self.passed_cases += 1,
                TestCaseResult::Skipped => self.skipped_cases += 1,
                TestCaseResult::Unknown => {}
                _ => self.failed_cases += 1,
            },
            RunEvent::RunCompleted { exit_status, .. } => {
                self.outcome = Some(exit_status == 0);
                self.running.clear();
                // A successful run finished every planned target, even the
                // ones whose completion the sink missed.
                if exit_status == 0 && self.lagged {
                    self.done = self.done.max(self.planned);
                }
                return true;
            }
            _ => {}
        }
        false
    }

    fn start(&mut self, target: &str) {
        *self.running.entry(target.to_string()).or_default() += 1;
    }

    /// Counts a completion only for a target announced running, so a target
    /// completed twice, or never started, is not counted.
    fn complete(&mut self, target: &str, result: TargetResult, cached: bool) {
        let Some(runs) = self.running.get_mut(target) else {
            return;
        };
        *runs -= 1;
        if *runs == 0 {
            self.running.remove(target);
        }
        self.done += 1;
        match (result, cached) {
            (TargetResult::Succeeded, true) => self.cached += 1,
            (TargetResult::Failed | TargetResult::Cancelled, _) => self.failed += 1,
            _ => {}
        }
    }

    fn snapshot(&self) -> serde_json::Value {
        let state = match self.outcome {
            None => "running",
            Some(true) => "succeeded",
            Some(false) => "failed",
        };
        json!({
            "v": 1,
            "command": self.command,
            "target": self.target,
            "state": state,
            "planned": self.planned.max(self.done),
            "done": self.done,
            "cached": self.cached,
            "failed": self.failed,
            "running": self.running.keys().take(MAX_RUNNING).collect::<Vec<_>>(),
            "tests": {
                "passed": self.passed_cases,
                "failed": self.failed_cases,
                "skipped": self.skipped_cases,
            },
            "elapsed_ms": u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(path: &Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn counts_planned_targets_and_ends_with_the_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("progress.jsonl");
        let bus = RunEventBus::new(64);
        let sink = ProgressFile::spawn_at(&bus, path.clone(), "build", "//app:app");
        bus.publish(RunEvent::RunStarted { at_epoch_ms: 0 });
        bus.publish(RunEvent::TargetsPlanned {
            at_epoch_ms: 0,
            count: 3,
        });
        for (id, cached) in [("//a", true), ("//b", false)] {
            bus.publish(RunEvent::TargetStarted {
                at_epoch_ms: 0,
                target_id: id.into(),
            });
            bus.publish(RunEvent::TargetCompleted {
                at_epoch_ms: 0,
                target_id: id.into(),
                result: TargetResult::Succeeded,
                was_cached: cached,
                duration_ms: 1,
            });
        }
        bus.publish(RunEvent::TargetStarted {
            at_epoch_ms: 0,
            target_id: "//c".into(),
        });
        bus.publish(RunEvent::TargetCompleted {
            at_epoch_ms: 0,
            target_id: "//c".into(),
            result: TargetResult::Failed,
            was_cached: false,
            duration_ms: 1,
        });
        // The root target, which no scheduler announced, is not counted.
        bus.publish(RunEvent::TargetCompleted {
            at_epoch_ms: 0,
            target_id: "//app:app".into(),
            result: TargetResult::Failed,
            was_cached: false,
            duration_ms: 1,
        });
        bus.publish(RunEvent::RunCompleted {
            at_epoch_ms: 0,
            exit_status: 1,
        });
        sink.finish().await;

        let last = lines(&path).pop().unwrap();
        assert_eq!(last["state"], "failed");
        assert_eq!(last["planned"], 3);
        assert_eq!(last["done"], 3);
        assert_eq!(last["cached"], 1);
        assert_eq!(last["failed"], 1);
        assert_eq!(last["running"], json!([]));
        assert_eq!(last["command"], "build");
    }

    #[test]
    fn a_schedule_counts_batches_of_one_target_and_sums_their_tests() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("progress.jsonl");
        let progress = ScheduleProgress::start_at(path.clone(), 2);
        progress.batch_started("//t");
        progress.batch_started("//t");
        progress.batch_finished(
            "//t",
            true,
            false,
            Some(&json!({"passed": 3, "failed": 0, "skipped": 1})),
        );
        progress.finish_snapshot_for_test();
        let mid = lines(&path).pop().unwrap();
        assert_eq!(mid["running"], json!(["//t"]));
        assert_eq!(mid["done"], 1);
        progress.batch_finished(
            "//t",
            false,
            false,
            Some(&json!({"passed": 1, "failed": 2, "skipped": 0})),
        );
        progress.finish(false);

        let last = lines(&path).pop().unwrap();
        assert_eq!(last["command"], "test");
        assert_eq!(last["state"], "failed");
        assert_eq!(last["planned"], 2);
        assert_eq!(last["done"], 2);
        assert_eq!(last["failed"], 1);
        assert_eq!(last["running"], json!([]));
        assert_eq!(
            last["tests"],
            json!({"passed": 4, "failed": 2, "skipped": 1})
        );
    }

    #[tokio::test]
    async fn an_unwritable_file_stops_the_sink_without_blocking_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("progress.jsonl");
        let bus = RunEventBus::new(4);
        let sink = ProgressFile::spawn_at(&bus, path.clone(), "test", "//t");
        bus.publish(RunEvent::RunCompleted {
            at_epoch_ms: 0,
            exit_status: 0,
        });
        sink.finish().await;
        assert!(!path.exists());
    }
}
