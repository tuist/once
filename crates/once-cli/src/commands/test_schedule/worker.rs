use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use once_core::{TestBatch, TestBatchAttempt, TestBatchAttemptSpec};
use serde_json::Value;

use super::executor::{millis, unix_ms_now};
use super::process::classify_run;

pub(super) struct ScheduledBatch {
    pub batch: TestBatch,
    pub estimated_duration_ms: Option<u64>,
}

pub(super) struct CompletedBatch {
    pub attempt: TestBatchAttempt,
    pub run: Value,
}

pub(super) fn run<F>(
    workers: usize,
    plan_id: &str,
    schedule_started_at: i64,
    batches: VecDeque<ScheduledBatch>,
    run_batch: &F,
) -> Vec<CompletedBatch>
where
    F: Fn(&TestBatch) -> Result<Value> + Sync,
{
    let queue = Arc::new(Mutex::new(batches));
    let completed = Arc::new(Mutex::new(Vec::new()));
    std::thread::scope(|scope| {
        for worker_index in 0..workers {
            let queue = Arc::clone(&queue);
            let completed = Arc::clone(&completed);
            let worker = format!("local-{}", worker_index + 1);
            scope.spawn(move || {
                run_worker(
                    plan_id,
                    &worker,
                    schedule_started_at,
                    &queue,
                    &completed,
                    run_batch,
                );
            });
        }
    });
    Arc::into_inner(completed)
        .expect("all test schedule workers joined")
        .into_inner()
        .expect("test schedule result lock poisoned")
}

fn run_worker<F>(
    plan_id: &str,
    worker: &str,
    schedule_started_at: i64,
    queue: &Mutex<VecDeque<ScheduledBatch>>,
    completed: &Mutex<Vec<CompletedBatch>>,
    run_batch: &F,
) where
    F: Fn(&TestBatch) -> Result<Value>,
{
    loop {
        let next = queue
            .lock()
            .expect("test schedule queue lock poisoned")
            .pop_front();
        let Some(scheduled) = next else {
            break;
        };
        tracing::info!(
            plan_id,
            batch_id = %scheduled.batch.id,
            target = %scheduled.batch.target,
            worker,
            estimated_duration_ms = ?scheduled.estimated_duration_ms,
            "starting test batch attempt"
        );
        let started_at_unix_ms = unix_ms_now().unwrap_or(schedule_started_at);
        let started = Instant::now();
        let result = run_batch(&scheduled.batch);
        let duration_ms = millis(started.elapsed().as_millis());
        let elapsed_ms = i64::try_from(duration_ms).unwrap_or(i64::MAX);
        let finished_at_unix_ms =
            unix_ms_now().unwrap_or_else(|_| started_at_unix_ms.saturating_add(elapsed_ms));
        let (run, status, exit_code, cache) = classify_run(&scheduled.batch, result);
        tracing::info!(
            plan_id,
            batch_id = %scheduled.batch.id,
            target = %scheduled.batch.target,
            worker,
            duration_ms,
            status = ?status,
            exit_code = ?exit_code,
            cache = ?cache,
            "completed test batch attempt"
        );
        let attempt = TestBatchAttempt::new(TestBatchAttemptSpec {
            id: uuid::Uuid::now_v7().to_string(),
            plan_id: plan_id.to_string(),
            batch_id: scheduled.batch.id,
            target: scheduled.batch.target,
            attempt: 1,
            placement: "local".to_string(),
            worker: worker.to_string(),
            estimated_duration_ms: scheduled.estimated_duration_ms,
            started_at_unix_ms,
            finished_at_unix_ms,
            duration_ms,
            status,
            exit_code,
            cache,
        })
        .expect("scheduler produced a valid test attempt");
        completed
            .lock()
            .expect("test schedule result lock poisoned")
            .push(CompletedBatch { attempt, run });
    }
}
