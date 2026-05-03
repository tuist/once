//! `fabrik targets` — list every declared target in the workspace.

use std::path::Path;

use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;

pub async fn print_targets(workspace: &Path) -> Result<()> {
    let targets = fabrik_frontend::load_workspace(workspace).context("loading workspace")?;
    let mut out = tokio::io::stdout();
    for t in targets {
        out.write_all(format!("{} {}\n", t.kind, t.label()).as_bytes())
            .await?;
    }
    out.flush().await?;
    Ok(())
}
