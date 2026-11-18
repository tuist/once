use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use sea_orm::{ActiveValue::Set, DbBackend, EntityTrait, FromQueryResult, QueryOrder, Statement};

use super::entity;
use super::{TestBatchAttempt, TestBatchStatus};
use crate::WorkspaceStore;

const DURATION_SAMPLE_LIMIT: usize = 20;

#[derive(FromQueryResult)]
struct DurationSample {
    batch_id: String,
    duration_ms: i64,
}

#[derive(Debug, Clone)]
pub struct TestTimingStore {
    store: WorkspaceStore,
}

impl TestTimingStore {
    pub fn open_workspace(workspace: impl AsRef<Path>) -> Self {
        Self {
            store: WorkspaceStore::open(workspace),
        }
    }

    pub fn path(&self) -> &Path {
        self.store.path()
    }

    pub async fn append(&self, attempts: &[TestBatchAttempt]) -> Result<()> {
        if attempts.is_empty() {
            return Ok(());
        }
        let db = self.store.connect().await?;
        let models = attempts
            .iter()
            .map(record_to_active_model)
            .collect::<Result<Vec<_>>>()?;
        entity::Entity::insert_many(models)
            .exec(&db)
            .await
            .context("writing test batch attempts")?;
        Ok(())
    }

    pub async fn load(&self) -> Result<Vec<TestBatchAttempt>> {
        if !self.path().exists() {
            return Ok(Vec::new());
        }
        let db = self.store.connect().await?;
        entity::Entity::find()
            .order_by_asc(entity::Column::StartedAtUnixMs)
            .all(&db)
            .await
            .with_context(|| format!("reading test timings from `{}`", self.path().display()))?
            .into_iter()
            .map(record_from_model)
            .collect()
    }

    pub async fn duration_estimates(&self) -> Result<BTreeMap<String, u64>> {
        if !self.path().exists() {
            return Ok(BTreeMap::new());
        }
        let db = self.store.connect().await?;
        // Bound the read in SQL to the most recent successful, uncached attempts
        // per batch rather than deserializing the whole history on every schedule.
        // DURATION_SAMPLE_LIMIT is an internal constant, so inlining it is safe.
        let sql = format!(
            "SELECT batch_id, duration_ms FROM ( \
                 SELECT batch_id, duration_ms, \
                     ROW_NUMBER() OVER ( \
                         PARTITION BY batch_id ORDER BY started_at_unix_ms DESC, id DESC \
                     ) AS recency \
                 FROM test_batch_attempts \
                 WHERE status = 'passed' AND (cache IS NULL OR cache <> 'hit') \
             ) WHERE recency <= {DURATION_SAMPLE_LIMIT}"
        );
        let rows =
            DurationSample::find_by_statement(Statement::from_string(DbBackend::Sqlite, sql))
                .all(&db)
                .await
                .with_context(|| {
                    format!("reading test timings from `{}`", self.path().display())
                })?;
        let mut samples = BTreeMap::<String, Vec<u64>>::new();
        for row in rows {
            let duration = u64::try_from(row.duration_ms).unwrap_or(0);
            samples.entry(row.batch_id).or_default().push(duration);
        }
        Ok(samples
            .into_iter()
            .map(|(batch_id, mut durations)| {
                durations.sort_unstable();
                let middle = durations.len() / 2;
                let estimate = if durations.len().is_multiple_of(2) {
                    durations[middle - 1].saturating_add(durations[middle]) / 2
                } else {
                    durations[middle]
                };
                (batch_id, estimate)
            })
            .collect())
    }
}

fn record_to_active_model(record: &TestBatchAttempt) -> Result<entity::ActiveModel> {
    Ok(entity::ActiveModel {
        id: Set(record.id.clone()),
        schema: Set(record.schema.clone()),
        plan_id: Set(record.plan_id.clone()),
        batch_id: Set(record.batch_id.clone()),
        target: Set(record.target.clone()),
        attempt: Set(i32::try_from(record.attempt).context("test attempt exceeds SQLite integer")?),
        placement: Set(record.placement.clone()),
        worker: Set(record.worker.clone()),
        estimated_duration_ms: Set(record
            .estimated_duration_ms
            .map(|value| i64::try_from(value).context("estimated duration exceeds SQLite integer"))
            .transpose()?),
        started_at_unix_ms: Set(record.started_at_unix_ms),
        finished_at_unix_ms: Set(record.finished_at_unix_ms),
        duration_ms: Set(
            i64::try_from(record.duration_ms).context("duration exceeds SQLite integer")?
        ),
        status: Set(record.status.as_str().to_string()),
        exit_code: Set(record.exit_code),
        cache: Set(record.cache.clone()),
    })
}

fn record_from_model(model: entity::Model) -> Result<TestBatchAttempt> {
    Ok(TestBatchAttempt {
        schema: model.schema,
        id: model.id,
        plan_id: model.plan_id,
        batch_id: model.batch_id,
        target: model.target,
        attempt: u32::try_from(model.attempt).context("negative stored test attempt")?,
        placement: model.placement,
        worker: model.worker,
        estimated_duration_ms: model
            .estimated_duration_ms
            .map(|value| u64::try_from(value).context("negative stored duration estimate"))
            .transpose()?,
        started_at_unix_ms: model.started_at_unix_ms,
        finished_at_unix_ms: model.finished_at_unix_ms,
        duration_ms: u64::try_from(model.duration_ms).context("negative stored duration")?,
        status: TestBatchStatus::from_storage(&model.status)?,
        exit_code: model.exit_code,
        cache: model.cache,
    })
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::{TestBatchAttemptSpec, TestBatchStatus};

    #[tokio::test]
    async fn estimates_use_median_of_successful_uncached_attempts() {
        let workspace = TempDir::new().unwrap();
        let store = TestTimingStore::open_workspace(workspace.path());
        let attempts = vec![
            attempt(10, TestBatchStatus::Passed, "miss", 1),
            attempt(30, TestBatchStatus::Passed, "miss", 2),
            attempt(1000, TestBatchStatus::Passed, "hit", 3),
            attempt(2, TestBatchStatus::Failed, "miss", 4),
        ];

        store.append(&attempts).await.unwrap();

        assert_eq!(store.duration_estimates().await.unwrap()["batch"], 20);
        assert_eq!(store.load().await.unwrap(), attempts);
    }

    fn attempt(
        duration_ms: u64,
        status: TestBatchStatus,
        cache: &str,
        started_at_unix_ms: i64,
    ) -> TestBatchAttempt {
        TestBatchAttempt::new(TestBatchAttemptSpec {
            id: format!("attempt-{started_at_unix_ms}"),
            plan_id: "plan".to_string(),
            batch_id: "batch".to_string(),
            target: "tests/unit".to_string(),
            attempt: 1,
            placement: "local".to_string(),
            worker: "local-1".to_string(),
            estimated_duration_ms: None,
            started_at_unix_ms,
            finished_at_unix_ms: started_at_unix_ms + 1,
            duration_ms,
            status,
            exit_code: Some(i32::from(status != TestBatchStatus::Passed)),
            cache: Some(cache.to_string()),
        })
        .unwrap()
    }
}
