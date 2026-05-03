//! `fabrik cache stats` — print blob/action counts for a workspace.

use anyhow::Result;
use fabrik_cas::Cas;
use tokio::io::AsyncWriteExt;

use crate::cli::Format;

pub async fn print_stats(cas: &Cas, format: Format) -> Result<()> {
    let s = cas.stats().await?;
    let body = match format {
        Format::Human => format!(
            "blobs:   {} ({} bytes)\nactions: {} ({} bytes)\n",
            s.blob_count, s.blob_bytes, s.action_count, s.action_bytes,
        ),
        Format::Json => {
            let v = serde_json::json!({
                "blobs": { "count": s.blob_count, "bytes": s.blob_bytes },
                "actions": { "count": s.action_count, "bytes": s.action_bytes },
            });
            format!("{v}\n")
        }
    };
    let mut out = tokio::io::stdout();
    out.write_all(body.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}
