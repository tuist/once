use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use struo_cas::CacheProvider;
use struo_core::{ResourceLimits, ResourcePool, ResourceRequest};
use struo_frontend::Diagnostic;

use super::registration::{failure, select};
use super::storage;
use crate::commands::graph::TriggerBatch;

pub(crate) async fn run(
    workspace: &Path,
    id: &str,
    event: Value,
    limits: ResourceLimits,
) -> Result<Value> {
    let mut lock = storage::lock(workspace)?;
    let _guard = lock
        .try_write()
        .context("another trigger operation owns this workspace")?;
    let resources = Arc::new(ResourcePool::new(limits.clone()));
    let preparation = resources
        .acquire(ResourceRequest::new(limits.cpu_slots, limits.memory_bytes))
        .await;
    let selection = select(workspace, id, event)?;
    verify_checkout(workspace, &selection.observation.revision)?;
    let run_id = uuid::Uuid::now_v7().to_string();
    let path = workspace
        .join(".struo/triggers/runs")
        .join(format!("{run_id}.json"));
    let mut record = json!({
        "schema": "struo.trigger_run.v1",
        "id": run_id,
        "mode": "simulation",
        "status": "preparing",
        "selection": selection,
        "memory_limit": limits.memory_bytes,
        "struo_version": env!("CARGO_PKG_VERSION"),
    });
    storage::atomic_write(&path, &serde_json::to_vec(&record)?)?;
    let cache = CacheProvider::open_local(workspace.join(".struo/triggers/cache"));
    let prepared =
        TriggerBatch::prepare(workspace, &cache, &selection.trigger.invoke, resources).await;
    let batch = match prepared.and_then(|batch| {
        verify_checkout(workspace, &selection.observation.revision)?;
        Ok(batch)
    }) {
        Ok(batch) => batch,
        Err(error) => {
            record["status"] = json!("failed");
            record["error"] = json!(format!("{error:#}"));
            record["diagnostics"] = json!([Diagnostic::new("trigger_preparation_failed", format!("{error:#}"))
                .with_attribute("invoke")
                .with_repair("Inspect the selected targets with struo query target and struo query capabilities; repair missing targets or capabilities without narrowing the active registration")]);
            storage::atomic_write(&path, &serde_json::to_vec(&record)?)?;
            return Ok(record);
        }
    };
    record["resolution"] = batch.resolution.clone();
    record["status"] = json!("running");
    storage::atomic_write(&path, &serde_json::to_vec(&record)?)?;
    drop(preparation);
    let outcomes = batch.execute(workspace).await;
    record["status"] = json!(
        if outcomes.iter().all(|entry| entry["status"] == "passed") {
            "passed"
        } else {
            "failed"
        }
    );
    record["outcomes"] = json!(outcomes);
    if let Err(error) = verify_checkout(workspace, &selection.observation.revision) {
        record["status"] = json!("failed");
        record["error"] = json!(format!("{error:#}"));
        record["diagnostics"] =
            json!([
                Diagnostic::new("trigger_checkout_changed", format!("{error:#}"))
                    .with_attribute("revision")
                    .with_repair("Keep the exact checkout unchanged throughout trigger execution")
            ]);
    }
    storage::atomic_write(&path, &serde_json::to_vec(&record)?)?;
    tracing::info!(trigger = id, run = %run_id, status = %record["status"], "finished trigger simulation");
    Ok(record)
}

fn verify_checkout(workspace: &Path, revision: &str) -> Result<()> {
    if storage::git(workspace, &["rev-parse", "HEAD"])?.trim() != revision {
        return Err(failure(
            "trigger_revision_mismatch",
            "checkout HEAD differs from the observation revision",
            "Check out the exact observed commit before running the trigger",
        ));
    }
    let status = storage::git(
        workspace,
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?;
    if !status.trim().is_empty() {
        return Err(failure("trigger_dirty_checkout", "trigger simulation requires a clean checkout", "Commit or remove local changes, keep event JSON outside the checkout, and ignore .struo/ runtime state"));
    }
    Ok(())
}
