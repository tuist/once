use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use sea_orm::{Database, DatabaseConnection};
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
        let db = Database::connect(sqlite_url(&self.path)?)
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
