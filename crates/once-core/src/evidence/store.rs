use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use anyhow::{anyhow, Context, Result};
use once_cas::Digest;
use sea_orm::{ActiveValue::Set, DatabaseConnection, EntityTrait, QueryOrder, TransactionTrait};

use super::entity;
use super::{EvidenceCacheState, EvidenceRecord, EvidenceStatus, EvidenceSubject};
use crate::WorkspaceStore;

type EvidenceDatabaseLock = Arc<tokio::sync::Mutex<()>>;
type EvidenceDatabase = Arc<tokio::sync::OnceCell<DatabaseConnection>>;

static EVIDENCE_DATABASE_LOCKS: LazyLock<Mutex<BTreeMap<PathBuf, EvidenceDatabaseLock>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
static EVIDENCE_DATABASES: LazyLock<Mutex<BTreeMap<PathBuf, EvidenceDatabase>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

#[derive(Debug, Clone)]
pub struct EvidenceStore {
    store: WorkspaceStore,
    database: EvidenceDatabase,
}

impl EvidenceStore {
    pub fn open_workspace(workspace: impl AsRef<Path>) -> Self {
        let store = WorkspaceStore::open(workspace);
        let database = evidence_database(store.path());
        Self { store, database }
    }

    pub fn path(&self) -> &Path {
        self.store.path()
    }

    pub async fn append(&self, record: &EvidenceRecord) -> Result<()> {
        self.append_many(std::slice::from_ref(record)).await
    }

    pub async fn append_many(&self, records: &[EvidenceRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let lock = evidence_database_lock(self.path());
        let _guard = lock.lock().await;
        let db = self.database().await?;
        let transaction = db.begin().await.context("starting evidence transaction")?;
        let models = records
            .iter()
            .map(record_to_active_model)
            .collect::<Result<Vec<_>>>()?;
        entity::Entity::insert_many(models)
            .exec(&transaction)
            .await
            .with_context(|| format!("writing {} evidence records", records.len()))?;
        transaction
            .commit()
            .await
            .context("committing evidence transaction")?;
        Ok(())
    }

    pub async fn load(&self) -> Result<Vec<EvidenceRecord>> {
        if !self.path().exists() {
            return Ok(Vec::new());
        }
        let lock = evidence_database_lock(self.path());
        let _guard = lock.lock().await;
        let db = self.database().await?;
        entity::Entity::find()
            .order_by_asc(entity::Column::CreatedAtUnixMs)
            .all(db)
            .await
            .with_context(|| format!("reading evidence records from `{}`", self.path().display()))?
            .into_iter()
            .map(record_from_model)
            .collect()
    }

    async fn database(&self) -> Result<&DatabaseConnection> {
        self.database.get_or_try_init(|| self.store.connect()).await
    }
}

fn evidence_database_lock(path: &Path) -> EvidenceDatabaseLock {
    let mut locks = EVIDENCE_DATABASE_LOCKS
        .lock()
        .expect("evidence database lock poisoned");
    Arc::clone(
        locks
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
    )
}

fn evidence_database(path: &Path) -> EvidenceDatabase {
    let mut databases = EVIDENCE_DATABASES
        .lock()
        .expect("evidence database map lock poisoned");
    Arc::clone(
        databases
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new())),
    )
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
        input_fingerprint_json: Set(record
            .input_fingerprint
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .context("serializing evidence input fingerprint")?),
        cache: Set(record.cache.as_str().to_string()),
        exit_code: Set(record.exit_code),
        stdout_digest: Set(record.stdout.map(|digest| digest.to_string())),
        stderr_digest: Set(record.stderr.map(|digest| digest.to_string())),
        outputs_json: Set(
            serde_json::to_string(&record.outputs).context("serializing evidence outputs")?
        ),
        created_at_unix_ms: Set(record.created_at_unix_ms),
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
        input_fingerprint: model
            .input_fingerprint_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .context("parsing evidence input fingerprint")?,
        cache: EvidenceCacheState::from_storage(&model.cache)?,
        exit_code: model.exit_code,
        stdout: parse_optional_digest(model.stdout_digest.as_deref(), "stdout_digest")?,
        stderr: parse_optional_digest(model.stderr_digest.as_deref(), "stderr_digest")?,
        outputs: serde_json::from_str::<BTreeMap<String, Digest>>(&model.outputs_json)
            .context("parsing evidence outputs")?,
        created_at_unix_ms: model.created_at_unix_ms,
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

    use crate::InputDigestBuilder;

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
        let mut builder = InputDigestBuilder::new(b"test");
        builder.push_bytes_component("toolchain", "identity", b"rust-1.96");
        let fingerprint = builder.finish_with_fingerprint();
        let record = EvidenceRecord::from_action_result_with_fingerprint(
            EvidenceSubject::target("cli", "test"),
            action,
            Some(fingerprint.input_digest),
            Some(fingerprint),
            EvidenceCacheState::Miss,
            &result,
        )
        .unwrap();

        store.append(&record).await.unwrap();

        let loaded = store.load().await.unwrap();
        assert_eq!(loaded, vec![record]);
        assert!(store.path().is_file());
    }

    #[tokio::test]
    async fn evidence_store_appends_records_in_one_batch() {
        let tmp = TempDir::new().unwrap();
        let store = EvidenceStore::open_workspace(tmp.path());
        let result = ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs: BTreeMap::new(),
        };
        let records = [b"one", b"two"].map(|action| {
            EvidenceRecord::from_action_result(
                EvidenceSubject::target("cli", "build"),
                Digest::of_bytes(action),
                None,
                EvidenceCacheState::Hit,
                &result,
            )
            .unwrap()
        });

        store.append_many(&records).await.unwrap();

        assert_eq!(store.load().await.unwrap(), records);
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
