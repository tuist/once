use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use once_cas::{CacheProvider, Digest};
use serde::Serialize;
use tokio::fs;

use super::runtime_descriptor::{RuntimeDescriptor, RuntimeRpcDescriptor};
use crate::cli::CACHE_DIR;

pub(super) struct RuntimeSession {
    pub(super) dir: PathBuf,
    pub(super) socket: PathBuf,
}

pub(super) async fn prepare(
    workspace: &Path,
    target_id: &str,
    runtime: &mut RuntimeDescriptor,
    socket: Option<PathBuf>,
    cache: &CacheProvider,
    stdout: Option<&Digest>,
    stderr: Option<&Digest>,
) -> Result<RuntimeSession> {
    let name = session_name(target_id);
    let dir = workspace.join(CACHE_DIR).join("runtime").join(&name);
    let socket = socket.unwrap_or_else(|| default_socket(&dir, &name));
    fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("creating runtime session {}", dir.display()))?;
    write_log(cache, stdout, &dir.join("stdout.log")).await?;
    write_log(cache, stderr, &dir.join("stderr.log")).await?;

    runtime.session = Some(display_path(workspace, &dir));
    runtime.rpc = Some(RuntimeRpcDescriptor::new(display_path(workspace, &socket)));
    write_session_json(&dir, target_id, runtime).await?;
    Ok(RuntimeSession { dir, socket })
}

async fn write_log(cache: &CacheProvider, digest: Option<&Digest>, path: &Path) -> Result<()> {
    match digest {
        Some(digest) => cache.copy_blob_to_file(digest, path).await?,
        None => fs::write(path, [])
            .await
            .with_context(|| format!("writing {}", path.display()))?,
    }
    Ok(())
}

fn default_socket(dir: &Path, session_name: &str) -> PathBuf {
    let socket = dir.join("control.sock");
    if socket.as_os_str().len() < 100 {
        return socket;
    }
    std::env::temp_dir().join(format!("once-{session_name}.sock"))
}

fn session_name(target_id: &str) -> String {
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let target = target_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("{target}-{started}")
}

async fn write_session_json(
    dir: &Path,
    target_id: &str,
    runtime: &RuntimeDescriptor,
) -> Result<()> {
    let raw = serde_json::to_vec_pretty(&SessionJson { target_id, runtime })?;
    fs::write(dir.join("session.json"), raw)
        .await
        .with_context(|| format!("writing {}", dir.join("session.json").display()))
}

fn display_path(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

#[derive(Serialize)]
struct SessionJson<'a> {
    target_id: &'a str,
    runtime: &'a RuntimeDescriptor,
}
