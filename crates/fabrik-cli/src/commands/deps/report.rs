use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::Result;
use fabrik_core::CacheState;
use fabrik_frontend::DependencyEcosystem;
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::cli::Format;
use crate::render;

pub(in crate::commands::deps) struct SyncReport {
    pub(in crate::commands::deps) name: String,
    pub(in crate::commands::deps) ecosystem: DependencyEcosystem,
    pub(in crate::commands::deps) lockfile: PathBuf,
    pub(in crate::commands::deps) manifest: Option<PathBuf>,
    pub(in crate::commands::deps) packages: usize,
    pub(in crate::commands::deps) declared: Option<usize>,
    pub(in crate::commands::deps) skipped: Option<usize>,
    pub(in crate::commands::deps) skipped_names: Vec<String>,
    pub(in crate::commands::deps) resolution_cache: Option<CacheState>,
}

pub(in crate::commands::deps) async fn write_sync_report(
    format: Format,
    reports: &[SyncReport],
) -> Result<()> {
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

pub(in crate::commands::deps) fn ecosystem_name(ecosystem: DependencyEcosystem) -> &'static str {
    match ecosystem {
        DependencyEcosystem::Rust => "rust",
        DependencyEcosystem::Swift => "swift",
        DependencyEcosystem::Go => "go",
        DependencyEcosystem::Elixir => "elixir",
    }
}
