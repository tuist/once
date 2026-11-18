use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use struo_cas::CacheProvider;
use struo_core::{
    ReproducibilityReport, ReproducibilityTrial, ResourceLimits, ResourcePool, ResourceRequest,
    WorkspacePath,
};
use tokio::io::AsyncWriteExt;

use crate::cli::{Format, Output};

mod artifacts;
mod retention;
mod snapshot;

#[derive(Debug, Serialize, Deserialize)]
pub struct VerificationReport {
    pub schema: String,
    pub id: String,
    pub subject: String,
    pub scope: String,
    pub limitations: Vec<String>,
    pub success: bool,
    #[serde(flatten)]
    pub comparison: ReproducibilityReport,
    pub trial_directories: Vec<String>,
    pub artifact_differences: Vec<artifacts::ArtifactDifference>,
    pub differences_truncated: bool,
    pub inspect_command: String,
}

pub async fn run<F, Fut>(
    workspace: &Path,
    subject: String,
    excluded_outputs: Vec<WorkspacePath>,
    resource_limits: ResourceLimits,
    run_trial: F,
) -> Result<VerificationReport>
where
    F: Fn(PathBuf, CacheProvider) -> Fut,
    Fut: Future<Output = Result<ReproducibilityTrial>>,
{
    let id = uuid::Uuid::now_v7().to_string();
    let relative = format!(".struo/verifications/{id}");
    let root = workspace.join(&relative);
    let baseline = root.join("baseline");
    let execution_root = root.join("workspace");
    let resources = Arc::new(ResourcePool::new(resource_limits));
    let permit = resources.acquire(ResourceRequest::default()).await;
    let source = workspace.to_path_buf();
    let destination = baseline.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        snapshot::copy(&source, &destination, &excluded_outputs)
    })
    .await??;
    let mut results = Vec::new();
    let mut trial_directories = Vec::new();
    for number in 1..=2 {
        let directory = root.join(format!("trial-{number}"));
        let cache = CacheProvider::open_local(directory.join("cache"));
        let permit = resources.acquire(ResourceRequest::default()).await;
        let source = baseline.clone();
        let destination = execution_root.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if destination.exists() {
                std::fs::remove_dir_all(&destination)?;
            }
            snapshot::copy(&source, &destination, &[])
        })
        .await??;
        let mut trial = match run_trial(execution_root.clone(), cache.clone()).await {
            Ok(trial) => trial,
            Err(error) => ReproducibilityTrial {
                action_digest: None,
                result: None,
                error: Some(format!("{error:#}")),
                log_paths: Vec::new(),
            },
        };
        if let Err(error) = struo_core::EvidenceStore::open_workspace(&execution_root)
            .close()
            .await
        {
            trial.error = Some(format!("closing trial evidence: {error:#}"));
        }
        retention::retain(&directory, &execution_root, &cache, &resources, &mut trial).await?;
        results.push(trial);
        trial_directories.push(format!("{relative}/trial-{number}"));
    }
    let permit = resources.acquire(ResourceRequest::default()).await;
    let retained = root.clone();
    let (comparison, artifact_differences, differences_truncated) =
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut comparison = ReproducibilityReport::from_trials(results);
            let (differences, truncated) = artifacts::compare(
                &retained.join("trial-1/outputs"),
                &retained.join("trial-2/outputs"),
                &mut comparison,
            )?;
            Ok::<_, anyhow::Error>((comparison, differences, truncated))
        })
        .await??;
    let report = VerificationReport {
        schema: "struo.verification.v1".into(), id: id.clone(), subject,
        scope: "Two executions from the same workspace snapshot at the same private path. Dependencies and fingerprints run in each trial with separate, initially empty local Struo action caches.".into(),
        limitations: vec![
            "This is an observation, not proof of reproducibility. No randomness, clock, or thread scheduling control is applied.".into(),
            "Host toolchains, host tool caches, absolute paths, network services, and external side effects are not isolated. Only run commands whose repeated execution you authorize.".into(),
            "The snapshot excludes Struo runtime state and known declared script outputs, except outputs overlapping declared inputs. Other pre-existing generated files remain part of the starting snapshot.".into(),
            "Comparison covers the selected command or target's declared result, not prerequisite-only artifacts and logs.".into(),
        ],
        success: comparison.passed(), comparison, trial_directories, artifact_differences, differences_truncated,
        inspect_command: format!("struo query verification {id}"),
    };
    std::fs::write(
        root.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    let permit = resources.acquire(ResourceRequest::default()).await;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        std::fs::remove_dir_all(baseline)?;
        if execution_root.exists() {
            std::fs::remove_dir_all(execution_root)?;
        }
        Ok::<_, std::io::Error>(())
    })
    .await??;
    Ok(report)
}

pub fn load(workspace: &Path, id: &str) -> Result<VerificationReport> {
    let id = uuid::Uuid::parse_str(id)
        .context("verification id must be a UUID returned by a verification check")?;
    let path = workspace.join(format!(".struo/verifications/{id}/report.json"));
    let file = std::fs::File::open(&path).with_context(|| format!("no retained verification `{id}`; run a build or script with --verify-reproducible first"))?;
    Ok(serde_json::from_reader(file)?)
}

pub async fn write(report: &VerificationReport, output: Output) -> Result<ExitCode> {
    let body = if output.format == Format::Human {
        artifacts::render(report)
    } else {
        crate::render::structured(output.format, report)?
    };
    let mut stdout = tokio::io::stdout();
    stdout.write_all(body.as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    Ok(if report.success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
