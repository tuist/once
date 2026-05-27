use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use fabrik_cas::{CacheProvider, Digest};
use fabrik_core::{Action, CacheState, InputDigestBuilder, RunOpts, Runner};
use fabrik_frontend::DependencyEntry;

use super::paths::entry_path;

pub(in crate::commands::deps) struct CachedResolution {
    pub(in crate::commands::deps) stdout: Vec<u8>,
    pub(in crate::commands::deps) cache: CacheState,
}

pub(in crate::commands::deps) async fn run_cached_resolution(
    workspace: &Path,
    cache: &CacheProvider,
    action: Action,
    command_name: &str,
) -> Result<CachedResolution> {
    let outcome = Runner::with_cache(cache.clone(), workspace, RunOpts::default())
        .run(&action)
        .await
        .with_context(|| format!("executing {command_name}"))?;
    if outcome.result.exit_code != 0 {
        let stderr = cache.get_blob(&outcome.result.stderr).await.map_or_else(
            |_| "<stderr unavailable>".to_string(),
            |bytes| String::from_utf8_lossy(&bytes).trim().to_string(),
        );
        return Err(anyhow!(
            "{command_name} failed (exit {}): {stderr}",
            outcome.result.exit_code
        ));
    }
    let stdout = cache
        .get_blob(&outcome.result.stdout)
        .await
        .with_context(|| format!("reading cached stdout for {command_name}"))?;
    Ok(CachedResolution {
        stdout,
        cache: outcome.cache,
    })
}

pub(in crate::commands::deps) fn resolution_input_digest(
    workspace: &Path,
    entry: &DependencyEntry,
    file_names: &[&str],
) -> Result<Digest> {
    let mut paths = std::collections::BTreeSet::new();
    paths.insert(entry_path(workspace, entry, &entry.manifest));
    if let Some(lockfile) = &entry.lockfile {
        let path = entry_path(workspace, entry, lockfile);
        if path.is_file() {
            paths.insert(path);
        }
    }
    collect_resolution_files(workspace, file_names, &mut paths)?;

    let mut builder = InputDigestBuilder::new(b"fabrik.deps.resolution.input.v1\0");
    for path in paths {
        let rel = path
            .strip_prefix(workspace)
            .with_context(|| format!("resolving {} against workspace", path.display()))?
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        builder
            .push_source(workspace, &rel)
            .with_context(|| format!("hashing dependency resolution input {rel}"))?;
    }
    builder.push_bytes(format!("entry:{}", entry.name).as_bytes());
    builder.push_bytes(format!("ecosystem:{:?}", entry.ecosystem).as_bytes());
    builder.push_bytes(format!("workspace:{}", workspace.display()).as_bytes());
    // Bumping the generated-external format must invalidate a cached
    // resolution so the next `deps sync` rewrites the generated tree
    // with the new shape rather than reusing the stale one.
    builder.push_bytes(
        format!(
            "generated-external-format:{}",
            fabrik_frontend::GENERATED_EXTERNAL_FORMAT_VERSION
        )
        .as_bytes(),
    );
    Ok(builder.finish())
}

fn collect_resolution_files(
    dir: &Path,
    file_names: &[&str],
    out: &mut std::collections::BTreeSet<PathBuf>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("reading metadata for {}", path.display()))?;
        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // Exclude generated caches, VCS metadata, and dependency
            // stores whose contents are derived from the manifests
            // being hashed here.
            if matches!(
                name.as_ref(),
                ".fabrik" | ".git" | "node_modules" | "target" | "vendor"
            ) {
                continue;
            }
            collect_resolution_files(&path, file_names, out)?;
        } else if file_type.is_file() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if file_names.iter().any(|candidate| *candidate == name) {
                out.insert(path);
            }
        }
    }
    Ok(())
}
