use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use once_cas::Digest;
use sea_orm::{ActiveValue::Set, EntityTrait, QueryOrder};

use super::entity;
use super::{EvidenceCacheState, EvidenceRecord, EvidenceStatus, EvidenceSubject};
use crate::WorkspaceStore;

#[derive(Debug, Clone)]
pub struct EvidenceStore {
    store: WorkspaceStore,
}

impl EvidenceStore {
    pub fn open_workspace(workspace: impl AsRef<Path>) -> Self {
        Self {
            store: WorkspaceStore::open(workspace),
        }
    }

    pub fn path(&self) -> &Path {
        self.store.path()
    }

    pub async fn append(&self, record: &EvidenceRecord) -> Result<()> {
        let db = self.store.connect().await?;
        let model = record_to_active_model(record)?;
        entity::Entity::insert(model)
            .exec(&db)
            .await
            .with_context(|| format!("writing evidence record `{}`", record.id))?;
        Ok(())
    }

    pub async fn load(&self) -> Result<Vec<EvidenceRecord>> {
        if !self.path().exists() {
            return Ok(Vec::new());
        }
        let db = self.store.connect().await?;
        entity::Entity::find()
            .order_by_asc(entity::Column::CreatedAtUnixMs)
            .all(&db)
            .await
            .with_context(|| format!("reading evidence records from `{}`", self.path().display()))?
            .into_iter()
            .map(record_from_model)
            .collect()
    }
}

fn record_to_active_model(record: &EvidenceRecord) -> Result<entity::ActiveModel> {
    Ok(entity::ActiveModel {
        id: Set(record.id.clone()),
        schema: Set(record.schema.clone()),
        kind: Set(record.kind.clone()),
        subject_kind: Set(record.subject.kind.clone()),
        subject_id: Set(record.subject.id.clone()),
        subject_capability: Set(record.subject.capability.clone()),
        status: Set(record.status.as_str().to_string()),
        action_digest: Set(record.action_digest.to_string()),
        input_digest: Set(record.input_digest.map(|digest| digest.to_string())),
        cache: Set(record.cache.as_str().to_string()),
        exit_code: Set(record.exit_code),
        stdout_digest: Set(record.stdout.map(|digest| digest.to_string())),
        stderr_digest: Set(record.stderr.map(|digest| digest.to_string())),
        outputs_json: Set(
            serde_json::to_string(&record.outputs).context("serializing evidence outputs")?
        ),
        created_at_unix_ms: Set(i64::try_from(record.created_at_unix_ms)
            .context("evidence timestamp does not fit SQLite integer")?),
    })
}

fn record_from_model(model: entity::Model) -> Result<EvidenceRecord> {
    Ok(EvidenceRecord {
        schema: model.schema,
        id: model.id,
        kind: model.kind,
        subject: EvidenceSubject {
            kind: model.subject_kind,
            id: model.subject_id,
            capability: model.subject_capability,
        },
        status: EvidenceStatus::from_storage(&model.status)?,
        action_digest: parse_digest(&model.action_digest, "action_digest")?,
        input_digest: parse_optional_digest(model.input_digest.as_deref(), "input_digest")?,
        cache: EvidenceCacheState::from_storage(&model.cache)?,
        exit_code: model.exit_code,
        stdout: parse_optional_digest(model.stdout_digest.as_deref(), "stdout_digest")?,
        stderr: parse_optional_digest(model.stderr_digest.as_deref(), "stderr_digest")?,
        outputs: serde_json::from_str::<BTreeMap<String, Digest>>(&model.outputs_json)
            .context("parsing evidence outputs")?,
        created_at_unix_ms: u128::try_from(model.created_at_unix_ms)
            .context("evidence timestamp cannot be negative")?,
    })
}

fn parse_optional_digest(raw: Option<&str>, field: &str) -> Result<Option<Digest>> {
    raw.map(|value| parse_digest(value, field)).transpose()
}

fn parse_digest(raw: &str, field: &str) -> Result<Digest> {
    Digest::from_hex(raw).ok_or_else(|| anyhow!("invalid evidence {field} `{raw}`"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use once_cas::ActionResult;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn evidence_store_uses_workspace_database_path() {
        let tmp = TempDir::new().unwrap();
        let store = EvidenceStore::open_workspace(tmp.path());

        assert_eq!(store.path(), &tmp.path().join(".once").join("once.sqlite"));
    }

    #[tokio::test]
    async fn evidence_store_appends_and_loads_records() {
        let tmp = TempDir::new().unwrap();
        let store = EvidenceStore::open_workspace(tmp.path());
        let action = Digest::of_bytes(b"action");
        let result = ActionResult {
            exit_code: 0,
            stdout: Some(Digest::of_bytes(b"stdout")),
            stderr: None,
            outputs: BTreeMap::from([("out.txt".to_string(), Digest::of_bytes(b"out"))]),
        };
        let record = EvidenceRecord::from_action_result(
            EvidenceSubject::target("cli", "test"),
            action,
            Some(Digest::of_bytes(b"input")),
            EvidenceCacheState::Miss,
            &result,
        );

        store.append(&record).await.unwrap();

        let loaded = store.load().await.unwrap();
        assert_eq!(loaded, vec![record]);
        assert!(store.path().is_file());
    }

    #[tokio::test]
    async fn evidence_store_loads_empty_when_database_is_missing() {
        let tmp = TempDir::new().unwrap();
        let store = EvidenceStore::open_workspace(tmp.path());

        let loaded = store.load().await.unwrap();

        assert!(loaded.is_empty());
        assert!(!store.path().exists());
    }
}
