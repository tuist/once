//! `fabrik cache` - inspect and mutate the configured cache provider.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use fabrik_cas::{ActionResult, CacheProvider, Digest};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

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
struct HashRecord {
    digest: Digest,
    mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parts: Option<usize>,
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

pub async fn hash(inputs: Vec<String>, combine: bool, output: Output) -> Result<()> {
    let record = if combine {
        let parts = parse_digest_inputs(&inputs)?;
        let digest = combine_digests(&parts);
        HashRecord {
            digest,
            mode: "combine",
            bytes: None,
            parts: Some(parts.len()),
        }
    } else {
        let input = match inputs.as_slice() {
            [] => None,
            [input] => Some(input.as_str()),
            _ => anyhow::bail!("cache hash accepts at most one input path without --combine"),
        };
        let (digest, bytes) = match input {
            Some(path) if path != "-" => {
                let file = tokio::fs::File::open(path)
                    .await
                    .with_context(|| format!("opening hash input {path}"))?;
                hash_reader(file).await?
            }
            _ => hash_reader(tokio::io::stdin()).await?,
        };
        HashRecord {
            digest,
            mode: "bytes",
            bytes: Some(bytes),
            parts: None,
        }
    };

    let body = match output.format {
        Format::Human => format!("{}\n", record.digest),
        Format::Json | Format::Toon => render::structured(output.format, &record)?,
    };
    write_stdout(body.as_bytes()).await
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

fn parse_digest_inputs(inputs: &[String]) -> Result<Vec<Digest>> {
    if inputs.is_empty() {
        anyhow::bail!("cache hash --combine requires at least one digest");
    }
    inputs
        .iter()
        .map(|input| {
            Digest::from_hex(input).with_context(|| {
                format!("expected a 64-character lowercase BLAKE3 digest, got `{input}`")
            })
        })
        .collect()
}

fn combine_digests(parts: &[Digest]) -> Digest {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"fabrik.hash.combine.v1\0");
    for part in parts {
        hasher.update(part.as_bytes());
    }
    Digest::from_bytes(*hasher.finalize().as_bytes())
}

async fn hash_reader<R: AsyncRead + Unpin>(mut reader: R) -> Result<(Digest, u64)> {
    let mut hasher = blake3::Hasher::new();
    let mut bytes = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        hasher.update(&buf[..n]);
    }
    Ok((Digest::from_bytes(*hasher.finalize().as_bytes()), bytes))
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
