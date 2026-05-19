use std::path::Path;
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use fabrik_cas::Cas;
use fabrik_core::CacheState;
use fabrik_frontend::{DependencyEcosystem, DependencyEntry};

use crate::cli::Format;

mod elixir;
mod go;
mod graph;
mod paths;
mod report;
mod resolution;
mod rust;
mod swift;

use paths::required_lockfile;
pub(in crate::commands::deps) use paths::{entry_path, graph_output_path};
use report::write_sync_report;
pub(in crate::commands::deps) use report::SyncReport;
pub(in crate::commands::deps) use resolution::{
    resolution_input_digest, run_cached_resolution, CachedResolution,
};

use graph::{write_graph_to, ResolvedGraph};

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
        DependencyEcosystem::Rust => rust::sync(workspace, cas, entry).await,
        DependencyEcosystem::Swift => {
            let lockfile = required_lockfile(workspace, entry)?;
            let graph = swift::load_graph(&lockfile).await?;
            write_graph_entry(workspace, entry, graph, None).await
        }
        DependencyEcosystem::Go => {
            let resolved = go::load_graph(workspace, cas, entry).await?;
            write_graph_entry(workspace, entry, resolved.graph, Some(resolved.cache)).await
        }
        DependencyEcosystem::Elixir => {
            let lockfile = required_lockfile(workspace, entry)?;
            let graph = elixir::load_graph(&lockfile).await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> DependencyEntry {
        DependencyEntry {
            package: String::new(),
            name: name.to_string(),
            ecosystem: DependencyEcosystem::Rust,
            manifest: "Cargo.toml".to_string(),
            lockfile: None,
            output: None,
        }
    }

    #[test]
    fn dependency_entry_name_must_be_a_single_path_segment() {
        for name in ["", ".", "..", "nested/name", r"nested\name", "cargo:serde"] {
            let err = validate_entry_name(&entry(name)).unwrap_err();
            assert!(
                err.to_string().contains("must be a single path segment"),
                "unexpected error for {name:?}: {err}"
            );
        }
    }

    #[test]
    fn dependency_entry_name_accepts_simple_segment() {
        validate_entry_name(&entry("cargo")).unwrap();
    }
}
