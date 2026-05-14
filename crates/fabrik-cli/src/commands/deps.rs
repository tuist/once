use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use fabrik_cas::{Cas, Digest};
use fabrik_core::{Action, CacheState, InputDigestBuilder, RunOpts, Runner};
use fabrik_frontend::{DependencyEcosystem, DependencyEntry};
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::cli::Format;
use crate::render;

use super::vendor_graph::{write_graph_to, ResolvedGraph};

pub(super) struct SyncReport {
    pub(super) name: String,
    pub(super) ecosystem: DependencyEcosystem,
    pub(super) lockfile: PathBuf,
    pub(super) manifest: Option<PathBuf>,
    pub(super) packages: usize,
    pub(super) declared: Option<usize>,
    pub(super) skipped: Option<usize>,
    pub(super) skipped_names: Vec<String>,
    pub(super) resolution_cache: Option<CacheState>,
}

pub async fn sync(
    workspace: &Path,
    cas: &Cas,
    format: Format,
    name: Option<&str>,
) -> Result<ExitCode> {
    let entries = selected_entries(workspace, name)?;
    let mut reports = Vec::new();
    for entry in entries {
        reports.push(sync_entry(workspace, cas, &entry).await?);
    }
    write_sync_report(format, &reports).await?;
    Ok(ExitCode::SUCCESS)
}

fn selected_entries(workspace: &Path, name: Option<&str>) -> Result<Vec<DependencyEntry>> {
    let entries = fabrik_frontend::load_dependency_entries(workspace)
        .context("loading dependency entries from fabrik.toml")?;
    let selected: Vec<_> = entries
        .into_iter()
        .filter(|entry| name.is_none_or(|name| entry.name == name))
        .collect();

    if selected.is_empty() {
        return match name {
            Some(name) => Err(anyhow!("no dependency entry named `{name}` in fabrik.toml")),
            None => Err(anyhow!("no [[deps]] declarations in fabrik.toml")),
        };
    }
    for entry in &selected {
        validate_entry_name(entry)?;
    }
    Ok(selected)
}

fn validate_entry_name(entry: &DependencyEntry) -> Result<()> {
    if entry.name.is_empty()
        || entry.name == "."
        || entry.name == ".."
        || entry.name.contains(['/', '\\', ':'])
    {
        return Err(anyhow!(
            "dependency entry name `{}` must be a single path segment",
            entry.name
        ));
    }
    Ok(())
}

async fn sync_entry(workspace: &Path, cas: &Cas, entry: &DependencyEntry) -> Result<SyncReport> {
    match entry.ecosystem {
        DependencyEcosystem::Rust => super::vendor_rust::sync(workspace, cas, entry).await,
        DependencyEcosystem::Swift => {
            let lockfile = required_lockfile(workspace, entry)?;
            let graph = super::vendor_swift::load_graph(&lockfile).await?;
            write_graph_entry(workspace, entry, graph, None).await
        }
        DependencyEcosystem::Go => {
            let resolved = super::vendor_go::load_graph(workspace, cas, entry).await?;
            write_graph_entry(workspace, entry, resolved.graph, Some(resolved.cache)).await
        }
        DependencyEcosystem::Elixir => {
            let lockfile = required_lockfile(workspace, entry)?;
            let graph = super::vendor_elixir::load_graph(&lockfile).await?;
            write_graph_entry(workspace, entry, graph, None).await
        }
    }
}

async fn write_graph_entry(
    workspace: &Path,
    entry: &DependencyEntry,
    graph: ResolvedGraph,
    resolution_cache: Option<CacheState>,
) -> Result<SyncReport> {
    let package_count = graph.packages.len();
    let output = graph_output_path(workspace, entry, graph.ecosystem.lockfile_name());
    write_graph_to(&output, &graph).await?;
    Ok(SyncReport {
        name: entry.name.clone(),
        ecosystem: entry.ecosystem,
        lockfile: output,
        manifest: None,
        packages: package_count,
        declared: None,
        skipped: None,
        skipped_names: Vec::new(),
        resolution_cache,
    })
}

pub(super) struct CachedResolution {
    pub(super) stdout: Vec<u8>,
    pub(super) cache: CacheState,
}

pub(super) async fn run_cached_resolution(
    workspace: &Path,
    cas: &Cas,
    action: Action,
    command_name: &str,
) -> Result<CachedResolution> {
    let outcome = Runner::new(cas.clone(), workspace, RunOpts::default())
        .run(&action)
        .await
        .with_context(|| format!("executing {command_name}"))?;
    if outcome.result.exit_code != 0 {
        let stderr = cas.get_blob(&outcome.result.stderr).await.map_or_else(
            |_| "<stderr unavailable>".to_string(),
            |bytes| String::from_utf8_lossy(&bytes).trim().to_string(),
        );
        return Err(anyhow!(
            "{command_name} failed (exit {}): {stderr}",
            outcome.result.exit_code
        ));
    }
    let stdout = cas
        .get_blob(&outcome.result.stdout)
        .await
        .with_context(|| format!("reading cached stdout for {command_name}"))?;
    Ok(CachedResolution {
        stdout,
        cache: outcome.cache,
    })
}

pub(super) fn resolution_input_digest(
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

pub(super) fn entry_path(workspace: &Path, entry: &DependencyEntry, path: &str) -> PathBuf {
    workspace.join(&entry.package).join(path)
}

pub(super) fn graph_output_path(
    workspace: &Path,
    entry: &DependencyEntry,
    default_name: &str,
) -> PathBuf {
    entry.output.as_ref().map_or_else(
        || {
            workspace
                .join("vendor")
                .join(&entry.name)
                .join(default_name)
        },
        |output| entry_path(workspace, entry, output),
    )
}

pub(super) fn required_lockfile(workspace: &Path, entry: &DependencyEntry) -> Result<PathBuf> {
    let lockfile = entry.lockfile.as_deref().ok_or_else(|| {
        anyhow!(
            "dependency entry `{}` for {} must declare `lockfile`",
            entry.name,
            ecosystem_name(entry.ecosystem)
        )
    })?;
    Ok(entry_path(workspace, entry, lockfile))
}

async fn write_sync_report(format: Format, reports: &[SyncReport]) -> Result<()> {
    match format {
        Format::Human => {
            let mut err = tokio::io::stderr();
            for report in reports {
                let mut line = format!(
                    "fabrik: deps synced {name} ({ecosystem}) to {lockfile} ({packages} packages)",
                    name = report.name,
                    ecosystem = ecosystem_name(report.ecosystem),
                    lockfile = report.lockfile.display(),
                    packages = report.packages,
                );
                if let Some(manifest) = &report.manifest {
                    let _ = write!(line, ", generated {}", manifest.display());
                }
                if let (Some(declared), Some(skipped)) = (report.declared, report.skipped) {
                    let _ = write!(line, ", {declared} declared, {skipped} skipped");
                }
                if let Some(cache) = report.resolution_cache {
                    let _ = write!(line, ", resolution {}", cache_tag(cache));
                }
                line.push('\n');
                err.write_all(line.as_bytes()).await?;
            }
            err.flush().await?;
        }
        Format::Json | Format::Toon => {
            let mut out = tokio::io::stdout();
            let payload: Vec<_> = reports.iter().map(SyncReportPayload::from).collect();
            let body = render::structured(format, &payload)?;
            out.write_all(body.as_bytes()).await?;
            out.flush().await?;
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct SyncReportPayload {
    name: String,
    ecosystem: &'static str,
    lockfile: String,
    manifest: Option<String>,
    packages: usize,
    declared: Option<usize>,
    skipped: Option<usize>,
    skipped_packages: Vec<String>,
    resolution_cache: Option<&'static str>,
}

impl From<&SyncReport> for SyncReportPayload {
    fn from(report: &SyncReport) -> Self {
        Self {
            name: report.name.clone(),
            ecosystem: ecosystem_name(report.ecosystem),
            lockfile: report.lockfile.display().to_string(),
            manifest: report
                .manifest
                .as_ref()
                .map(|path| path.display().to_string()),
            packages: report.packages,
            declared: report.declared,
            skipped: report.skipped,
            skipped_packages: report.skipped_names.clone(),
            resolution_cache: report.resolution_cache.map(cache_tag),
        }
    }
}

fn cache_tag(cache: CacheState) -> &'static str {
    match cache {
        CacheState::Hit => "hit",
        CacheState::Miss => "miss",
    }
}

fn ecosystem_name(ecosystem: DependencyEcosystem) -> &'static str {
    match ecosystem {
        DependencyEcosystem::Rust => "rust",
        DependencyEcosystem::Swift => "swift",
        DependencyEcosystem::Go => "go",
        DependencyEcosystem::Elixir => "elixir",
    }
}
