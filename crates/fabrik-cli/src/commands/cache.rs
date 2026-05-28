//! `fabrik cache` - inspect and mutate the configured cache provider.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use fabrik_cas::{ActionResult, CacheProvider, Digest};
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::cli::{Format, Output};
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

#[derive(Serialize)]
struct BlobPutRecord {
    digest: Digest,
}

#[derive(Serialize)]
struct BlobGetRecord<'a> {
    digest: Digest,
    bytes: usize,
    output: &'a str,
}

#[derive(Serialize)]
struct ActionGetRecord {
    action: Digest,
    hit: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<ActionResult>,
}

#[derive(Serialize)]
struct ActionPutRecord {
    action: Digest,
    stored: bool,
}

#[derive(Serialize)]
struct ActionForgetRecord {
    action: Digest,
    removed: bool,
}

pub async fn print_stats(cache: &CacheProvider, output: Output) -> Result<()> {
    let s = cache.stats().await?;
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
    let body = match output.format {
        Format::Human => format!(
            "blobs:   {} ({} bytes)\nactions: {} ({} bytes)\n",
            s.blob_count, s.blob_bytes, s.action_count, s.action_bytes,
        ),
        Format::Json | Format::Toon => render::structured(output.format, &stats)?,
    };
    let mut out = tokio::io::stdout();
    out.write_all(body.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}

pub async fn put_blob(cache: &CacheProvider, path: Option<&Path>, output: Output) -> Result<()> {
    let digest = match path {
        Some(path) if path != Path::new("-") => {
            let file = tokio::fs::File::open(path)
                .await
                .with_context(|| format!("opening blob input {}", path.display()))?;
            cache.put_stream(file).await?
        }
        _ => {
            let stdin = tokio::io::stdin();
            cache.put_stream(stdin).await?
        }
    };
    let record = BlobPutRecord { digest };
    let body = match output.format {
        Format::Human => format!("{digest}\n"),
        Format::Json | Format::Toon => render::structured(output.format, &record)?,
    };
    write_stdout(body.as_bytes()).await
}

pub async fn get_blob(
    cache: &CacheProvider,
    digest: Digest,
    output_path: Option<&Path>,
    output: Output,
) -> Result<()> {
    let bytes = cache.get_blob(&digest).await?;
    match output_path {
        Some(path) if path != Path::new("-") => {
            write_output_file(path, &bytes).await?;
            let path_text = path.display().to_string();
            let record = BlobGetRecord {
                digest,
                bytes: bytes.len(),
                output: &path_text,
            };
            let body = match output.format {
                Format::Human if output.show_human_trailers() => {
                    format!("wrote {} bytes to {}\n", bytes.len(), path.display())
                }
                Format::Human => String::new(),
                Format::Json | Format::Toon => render::structured(output.format, &record)?,
            };
            write_stdout(body.as_bytes()).await
        }
        _ => write_stdout(&bytes).await,
    }
}

pub async fn get_action(cache: &CacheProvider, action: Digest, output: Output) -> Result<()> {
    let result = cache.get_action_result(&action).await?;
    let record = ActionGetRecord {
        action,
        hit: result.is_some(),
        result,
    };
    let body = match output.format {
        Format::Human => {
            if let Some(result) = &record.result {
                format!("{}\n", serde_json::to_string_pretty(result)?)
            } else {
                format!("action miss: {action}\n")
            }
        }
        Format::Json | Format::Toon => render::structured(output.format, &record)?,
    };
    write_stdout(body.as_bytes()).await
}

pub async fn put_action(
    cache: &CacheProvider,
    action: Digest,
    exit_code: i32,
    stdout: Digest,
    stderr: Digest,
    outputs: Vec<(String, Digest)>,
    output: Output,
) -> Result<()> {
    let result = ActionResult {
        exit_code,
        stdout,
        stderr,
        outputs: BTreeMap::from_iter(outputs),
    };
    cache.put_action_result(&action, &result).await?;
    let record = ActionPutRecord {
        action,
        stored: true,
    };
    let body = match output.format {
        Format::Human if output.show_human_trailers() => format!("stored action {action}\n"),
        Format::Human => String::new(),
        Format::Json | Format::Toon => render::structured(output.format, &record)?,
    };
    write_stdout(body.as_bytes()).await
}

pub async fn forget_action(cache: &CacheProvider, action: Digest, output: Output) -> Result<()> {
    let removed = cache.forget_action(&action).await?;
    let record = ActionForgetRecord { action, removed };
    let body = match output.format {
        Format::Human if output.show_human_trailers() && removed => {
            format!("forgot action {action}\n")
        }
        Format::Human if output.show_human_trailers() => format!("action not found: {action}\n"),
        Format::Human => String::new(),
        Format::Json | Format::Toon => render::structured(output.format, &record)?,
    };
    write_stdout(body.as_bytes()).await
}

async fn write_stdout(bytes: &[u8]) -> Result<()> {
    let mut out = tokio::io::stdout();
    out.write_all(bytes).await?;
    out.flush().await?;
    Ok(())
}

async fn write_output_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("creating output directory {}", parent.display()))?;
    }
    tokio::fs::write(path, bytes)
        .await
        .with_context(|| format!("writing blob output {}", path.display()))
}
