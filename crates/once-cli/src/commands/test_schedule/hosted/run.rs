use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use once_core::{ResourceLimits, SandboxMode};
use serde_json::{json, Value};

use super::{artifact, client, metadata, report};
use crate::cli::Output;
use crate::commands::query::test_plan::portable::PortableTestPlan;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    workspace: &Path,
    output: Output,
    id: &str,
    index: Option<u32>,
    attempt: Option<u32>,
    revision: u32,
    jobs: Option<usize>,
    sandbox: SandboxMode,
    limits: ResourceLimits,
) -> Result<ExitCode> {
    let id = uuid::Uuid::parse_str(id)
        .context("--shard-plan must be an exact hosted plan UUID")?
        .to_string();
    let index = match index {
        Some(index) => index,
        None => {
            number("TUIST_SHARD_INDEX")?.context("pass --shard-index or set TUIST_SHARD_INDEX")?
        }
    };
    let attempt = match attempt {
        Some(attempt) => attempt,
        None => match number("TUIST_SHARD_ATTEMPT")? {
            Some(attempt) => attempt,
            None => number("GITHUB_RUN_ATTEMPT")?.unwrap_or(1),
        },
    };
    ensure!(attempt > 0, "--shard-attempt must be positive");
    super::super::validate_workers(jobs)?;
    let host = client::Host::resolve(workspace)?;
    let assignment = host.assignment(&id, index).await?;
    let modules = assignment_modules(&assignment, &id)?;
    tracing::info!(shard_plan_id = %id, shard_index = index, shard_attempt = attempt, report_revision = revision, assigned_batches = modules.len(), "executing hosted test partition");
    let started = Instant::now();
    let mut payload = metadata::collect(workspace).await;
    payload["ran_at"] = chrono::Utc::now()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        .into();
    let result = execute(
        workspace,
        &host,
        &assignment,
        &id,
        index,
        &modules,
        jobs,
        sandbox,
        limits,
    )
    .await;
    let (cases, success) = result
        .as_ref()
        .map_or((vec![], false), std::clone::Clone::clone);
    payload.as_object_mut().context("invalid report metadata")?.extend(json!({
        "id": uuid::Uuid::now_v7().to_string(), "build_system": "once", "scheme": "once test",
        "duration": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX).min(i32::MAX as u64),
        "status": if success {"success"} else {"failure"}, "test_modules": cases,
        "shard_plan_id": id, "shard_index": index, "shard_attempt": attempt, "shard_report_revision": revision,
    }).as_object().unwrap().clone());
    let signal = result
        .as_ref()
        .err()
        .and_then(|error| error.downcast_ref::<super::super::cancellation::Interrupted>())
        .map(|error| error.signal);
    let submitted = if signal.is_some() {
        tokio::time::timeout(Duration::from_secs(3), host.post("", &payload))
            .await
            .context("cancelled partition report upload timed out")
            .and_then(|value| value)
    } else {
        host.post("", &payload).await
    };
    if let Some(signal) = signal {
        if let Err(error) = submitted {
            tracing::warn!(%error, "cancelled partition report upload failed");
        }
        crate::termination::terminate(signal);
    }
    if let Err(error) = submitted {
        if let Err(execution_error) = result {
            return Err(execution_error)
                .context(format!("partition report upload also failed: {error:#}"));
        }
        return Err(error).context("uploading immutable partition report");
    }
    tracing::info!(shard_plan_id = %id, shard_index = index, success, "submitted immutable hosted test partition report");
    result?;
    crate::commands::query::write_body(
        output,
        || {
            format!(
                "Hosted test partition {}\n",
                if success { "passed" } else { "failed" }
            )
        },
        &payload,
    )
    .await?;
    Ok(if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[allow(clippy::too_many_arguments)]
async fn execute(
    workspace: &Path,
    host: &client::Host,
    assignment: &Value,
    id: &str,
    index: u32,
    modules: &[String],
    jobs: Option<usize>,
    sandbox: SandboxMode,
    limits: ResourceLimits,
) -> Result<(Vec<Value>, bool)> {
    let urls = assignment["download_urls"]
        .as_array()
        .context("missing portable plan download URLs")?;
    ensure!(
        urls.len() == 1,
        "hosted portable plan requires exactly one artifact"
    );
    let directory = tempfile::tempdir()?;
    let artifact = directory.path().join("artifact.json");
    host.download(urls[0].as_str().context("invalid artifact URL")?, &artifact)
        .await?;
    let path = directory.path().join("plan.json");
    artifact::unpack(&artifact, id, &path, limits.clone()).await?;
    let portable = PortableTestPlan::read(&path)?;
    let graph = once_frontend::load_graph_workspace(workspace)?;
    portable.validate(workspace, &graph)?;
    portable.select(modules)?;
    // An empty hosted assignment is not the portable CLI's implicit all-batches selection.
    if modules.is_empty() {
        return Ok((vec![], true));
    }
    let value = super::super::portable::execute(
        workspace,
        graph,
        super::super::portable::Invocation {
            path,
            external_group: Some(id.to_string()),
            partition_index: Some(index),
        },
        modules,
        jobs,
        sandbox,
        limits,
        true,
    )
    .await?;
    report::project(&value, &portable, modules)
}

fn assignment_modules(value: &Value, id: &str) -> Result<Vec<String>> {
    ensure!(
        uuid::Uuid::parse_str(
            value["shard_plan_id"]
                .as_str()
                .context("missing assignment plan UUID")?
        )? == uuid::Uuid::parse_str(id)?,
        "assignment belongs to another hosted plan"
    );
    ensure!(
        value["suites"]
            .as_object()
            .is_some_and(serde_json::Map::is_empty)
            && value
                .get("skip")
                .is_none_or(|skip| skip.as_array().is_some_and(Vec::is_empty)),
        "Once requires module-granularity batch assignments without suite overrides"
    );
    let modules: Vec<String> = serde_json::from_value(value["modules"].clone())?;
    ensure!(
        modules.iter().collect::<BTreeSet<_>>().len() == modules.len(),
        "duplicate hosted batch assignment"
    );
    Ok(modules)
}

fn number(name: &str) -> Result<Option<u32>> {
    metadata::variable(name)
        .map(|value| {
            value
                .parse()
                .with_context(|| format!("{name} must be a UInt32"))
        })
        .transpose()
}
