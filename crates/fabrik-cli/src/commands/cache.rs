//! `fabrik cache stats` — print blob/action counts for a workspace.

use anyhow::Result;
use fabrik_cas::Cas;
use tokio::io::AsyncWriteExt;

pub async fn print_stats(cas: &Cas) -> Result<()> {
    let s = cas.stats().await?;
    let body = format!(
        "blobs:   {} ({} bytes)\nactions: {} ({} bytes)\n",
        s.blob_count, s.blob_bytes, s.action_count, s.action_bytes,
    );
    let mut out = tokio::io::stdout();
    out.write_all(body.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}
