use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use sea_orm::sqlx::sqlite::{SqliteJournalMode, SqliteSynchronous};
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

use crate::evidence::migration::Migrator;

#[derive(Debug, Clone)]
pub struct WorkspaceStore {
    path: PathBuf,
}

impl WorkspaceStore {
    pub fn open(workspace: impl AsRef<Path>) -> Self {
        Self {
            path: workspace.as_ref().join(".once").join("once.sqlite"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn connect(&self) -> Result<DatabaseConnection> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("creating Once state directory `{}`", parent.display()))?;
        }
        let mut options = ConnectOptions::new(sqlite_url(&self.path)?);
        // A build appends evidence as each target finishes, so a commit that
        // waits for the disk holds up the targets behind it. The write-ahead
        // log commits without syncing and still cannot be torn by a crash; at
        // worst a power loss drops the last records written.
        options.map_sqlx_sqlite_opts(|sqlite| {
            sqlite
                .journal_mode(SqliteJournalMode::Wal)
                .synchronous(SqliteSynchronous::Normal)
        });
        let db = Database::connect(options)
            .await
            .with_context(|| format!("opening Once database `{}`", self.path.display()))?;
        Migrator::up(&db, None)
            .await
            .with_context(|| format!("migrating Once database `{}`", self.path.display()))?;
        Ok(db)
    }
}

fn sqlite_url(path: &Path) -> Result<String> {
    let path = path
        .to_str()
        .ok_or_else(|| anyhow!("Once database path must be UTF-8"))?;
    Ok(format!("sqlite://{path}?mode=rwc"))
}
