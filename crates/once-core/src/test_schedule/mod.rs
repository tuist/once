mod entity;
mod store;

use anyhow::{ensure, Context, Result};
use once_cas::Digest;
use serde::{Deserialize, Serialize};

pub use store::TestTimingStore;

pub const TEST_BATCH_ATTEMPT_SCHEMA: &str = "once.test_batch_attempt.v1";
pub const TEST_SCHEDULE_SCHEMA: &str = "once.test_schedule.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestBatchStatus {
    Passed,
    Failed,
    Error,
}

impl TestBatchStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Error => "error",
        }
    }

    pub(crate) fn from_storage(raw: &str) -> Result<Self> {
        match raw {
            "passed" => Ok(Self::Passed),
            "failed" => Ok(Self::Failed),
            "error" => Ok(Self::Error),
            _ => anyhow::bail!("unknown test batch status `{raw}`"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestBatchAttemptSpec {
    pub id: String,
    pub plan_id: String,
    pub batch_id: String,
    pub target: String,
    pub attempt: u32,
    pub placement: String,
    pub worker: String,
    pub estimated_duration_ms: Option<u64>,
    pub started_at_unix_ms: i64,
    pub finished_at_unix_ms: i64,
    pub duration_ms: u64,
    pub status: TestBatchStatus,
    pub exit_code: Option<i32>,
    pub cache: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestBatchAttempt {
    pub schema: String,
    pub id: String,
    pub plan_id: String,
    pub batch_id: String,
    pub target: String,
    pub attempt: u32,
    pub placement: String,
    pub worker: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_duration_ms: Option<u64>,
    pub started_at_unix_ms: i64,
    pub finished_at_unix_ms: i64,
    pub duration_ms: u64,
    pub status: TestBatchStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<String>,
}

impl TestBatchAttempt {
    pub fn new(spec: TestBatchAttemptSpec) -> Result<Self> {
        ensure!(!spec.id.is_empty(), "test batch attempt id cannot be empty");
        ensure!(
            spec.attempt > 0,
            "test batch attempt must be greater than zero"
        );
        ensure!(
            spec.finished_at_unix_ms >= spec.started_at_unix_ms,
            "test batch finish time precedes its start time"
        );
        Ok(Self {
            schema: TEST_BATCH_ATTEMPT_SCHEMA.to_string(),
            id: spec.id,
            plan_id: spec.plan_id,
            batch_id: spec.batch_id,
            target: spec.target,
            attempt: spec.attempt,
            placement: spec.placement,
            worker: spec.worker,
            estimated_duration_ms: spec.estimated_duration_ms,
            started_at_unix_ms: spec.started_at_unix_ms,
            finished_at_unix_ms: spec.finished_at_unix_ms,
            duration_ms: spec.duration_ms,
            status: spec.status,
            exit_code: spec.exit_code,
            cache: spec.cache,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestSchedule {
    pub schema: String,
    pub id: String,
    pub plan_id: String,
    pub strategy: String,
    pub workers: usize,
    pub started_at_unix_ms: i64,
    pub finished_at_unix_ms: i64,
    pub duration_ms: u64,
    pub attempts: Vec<TestBatchAttempt>,
}

impl TestSchedule {
    pub fn new(
        plan_id: impl Into<String>,
        strategy: impl Into<String>,
        workers: usize,
        started_at_unix_ms: i64,
        finished_at_unix_ms: i64,
        duration_ms: u64,
        mut attempts: Vec<TestBatchAttempt>,
    ) -> Result<Self> {
        ensure!(workers > 0, "test schedule must have at least one worker");
        ensure!(
            finished_at_unix_ms >= started_at_unix_ms,
            "test schedule finish time precedes its start time"
        );
        let plan_id = plan_id.into();
        let strategy = strategy.into();
        attempts.sort_by(|left, right| {
            left.started_at_unix_ms
                .cmp(&right.started_at_unix_ms)
                .then(left.batch_id.cmp(&right.batch_id))
        });
        let attempt_ids = attempts
            .iter()
            .map(|attempt| attempt.id.as_str())
            .collect::<Vec<_>>();
        let id = stable_id(
            "test-schedule",
            &(
                &plan_id,
                &strategy,
                workers,
                started_at_unix_ms,
                finished_at_unix_ms,
                &attempt_ids,
            ),
        )?;
        Ok(Self {
            schema: TEST_SCHEDULE_SCHEMA.to_string(),
            id,
            plan_id,
            strategy,
            workers,
            started_at_unix_ms,
            finished_at_unix_ms,
            duration_ms,
            attempts,
        })
    }
}

fn stable_id<T: Serialize>(domain: &str, value: &T) -> Result<String> {
    let mut material = domain.as_bytes().to_vec();
    material.push(0);
    material.extend(serde_json::to_vec(value).context("serializing test schedule identity")?);
    Ok(Digest::of_bytes(&material).to_string())
}

#[cfg(test)]
mod tests;
