//! `fabrik cache stats` - print blob/action counts for a workspace.

use anyhow::Result;
use fabrik_cas::Cas;
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::cli::Format;
use crate::render;

#[derive(Serialize)]
struct CacheEntry {
    count: u64,
    bytes: u64,
}

#[derive(Serialize)]
struct CacheStats {
    blobs: CacheEntry,
    actions: CacheEntry,
}

pub async fn print_stats(cas: &Cas, format: Format) -> Result<()> {
    let s = cas.stats().await?;
    let stats = CacheStats {
        blobs: CacheEntry {
            count: s.blob_count,
            bytes: s.blob_bytes,
        },
        actions: CacheEntry {
            count: s.action_count,
            bytes: s.action_bytes,
        },
    };
    let body = match format {
        Format::Human => format!(
            "blobs:   {} ({} bytes)\nactions: {} ({} bytes)\n",
            s.blob_count, s.blob_bytes, s.action_count, s.action_bytes,
        ),
        Format::Json | Format::Toon => render::structured(format, &stats)?,
    };
    let mut out = tokio::io::stdout();
    out.write_all(body.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}
