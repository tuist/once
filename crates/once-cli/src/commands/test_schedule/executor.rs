use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use once_core::{SandboxMode, TestBatch, TestPlan, TestSchedule};
use serde_json::Value;

use super::process::run_test_target;
use super::worker::{self, ScheduledBatch};

const SCHEDULE_STRATEGY: &str = "longest_estimated_duration_first_dynamic";

pub(super) struct CompletedSchedule {
    pub schedule: TestSchedule,
    pub runs: Vec<Value>,
}

pub(super) fn execute(
    executable: &Path,
    workspace: &Path,
    plan: &TestPlan,
    estimates: &BTreeMap<String, u64>,
    requested_workers: Option<usize>,
    sandbox: SandboxMode,
) -> Result<CompletedSchedule> {
    execute_with(plan, estimates, requested_workers, |batch| {
        run_test_target(executable, workspace, batch, sandbox)
    })
}

fn execute_with<F>(
    plan: &TestPlan,
    estimates: &BTreeMap<String, u64>,
    requested_workers: Option<usize>,
    run_batch: F,
) -> Result<CompletedSchedule>
where
    F: Fn(&TestBatch) -> Result<Value> + Sync,
{
    let workers = worker_count(requested_workers, plan.batches.len());
    let schedule_started_at = unix_ms_now()?;
    let schedule_started = Instant::now();
    tracing::info!(
        plan_id = %plan.id,
        batch_count = plan.batches.len(),
        workers,
        strategy = SCHEDULE_STRATEGY,
        "starting test schedule"
    );
    let mut completed = worker::run(
        workers,
        &plan.id,
        schedule_started_at,
        scheduled_batches(plan, estimates),
        &run_batch,
    );

    let finished_at_unix_ms = unix_ms_now()?;
    let duration_ms = millis(schedule_started.elapsed().as_millis());
    tracing::info!(
        plan_id = %plan.id,
        duration_ms,
        workers,
        "completed test schedule"
    );
    let order = plan
        .batches
        .iter()
        .enumerate()
        .map(|(index, batch)| (batch.id.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    completed.sort_by_key(|batch| order[batch.attempt.batch_id.as_str()]);
    let attempts = completed
        .iter()
        .map(|batch| batch.attempt.clone())
        .collect();
    let runs = completed.into_iter().map(|batch| batch.run).collect();
    let schedule = TestSchedule::new(
        &plan.id,
        SCHEDULE_STRATEGY,
        workers,
        schedule_started_at,
        finished_at_unix_ms,
        duration_ms,
        attempts,
    )?;
    Ok(CompletedSchedule { schedule, runs })
}

fn scheduled_batches(
    plan: &TestPlan,
    estimates: &BTreeMap<String, u64>,
) -> VecDeque<ScheduledBatch> {
    let mut batches = plan
        .batches
        .iter()
        .cloned()
        .map(|batch| ScheduledBatch {
            estimated_duration_ms: estimates.get(&batch.id).copied(),
            batch,
        })
        .collect::<Vec<_>>();
    batches.sort_by(|left, right| {
        right
            .estimated_duration_ms
            .unwrap_or(0)
            .cmp(&left.estimated_duration_ms.unwrap_or(0))
            .then(left.batch.id.cmp(&right.batch.id))
    });
    VecDeque::from(batches)
}

fn worker_count(requested: Option<usize>, batches: usize) -> usize {
    let available = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    requested.unwrap_or(available).min(batches).max(1)
}

pub(super) fn unix_ms_now() -> Result<i64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time precedes the Unix epoch")?;
    i64::try_from(duration.as_millis()).context("timestamp exceeds signed integer range")
}

pub(super) fn millis(value: u128) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
