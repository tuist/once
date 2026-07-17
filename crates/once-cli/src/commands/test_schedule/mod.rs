mod executor;
mod process;
mod results;
mod worker;

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use once_core::{SandboxMode, TestPlan, TestSchedule, TestTimingStore};
use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncWriteExt;

use crate::cli::{Format, Output};
use crate::render;

pub(crate) const MAX_TEST_WORKERS: usize = 256;

#[derive(Debug, Serialize)]
pub(crate) struct TestExecutionReport {
    pub plan: TestPlan,
    pub schedule: TestSchedule,
    pub runs: Vec<Value>,
}

impl TestExecutionReport {
    fn succeeded(&self) -> bool {
        self.runs
            .iter()
            .all(|run| run.get("success").and_then(Value::as_bool) == Some(true))
    }
}

pub(crate) async fn execute(
    workspace: &Path,
    graph: Option<Vec<once_frontend::GraphTarget>>,
    plan: TestPlan,
    requested_workers: Option<usize>,
    sandbox: SandboxMode,
) -> Result<TestExecutionReport> {
    validate_workers(requested_workers)?;
    if plan.batches.is_empty() {
        anyhow::bail!("test plan contains no batches");
    }
    let graph = match graph {
        Some(graph) => graph,
        None => once_frontend::load_graph_workspace(workspace).context("loading graph")?,
    };
    validate_plan_targets(workspace, &graph, &plan)?;
    let store = TestTimingStore::open_workspace(workspace);
    let estimates = store.duration_estimates().await?;
    let executable = std::env::current_exe().context("resolving current once executable")?;
    let workspace_path = workspace.to_path_buf();
    let scheduler_workspace = workspace_path.clone();
    let blocking_plan = plan.clone();
    let completed = tokio::task::spawn_blocking(move || {
        executor::execute(
            &executable,
            &scheduler_workspace,
            &blocking_plan,
            &estimates,
            requested_workers,
            sandbox,
        )
    })
    .await
    .context("joining test scheduler")??;
    results::persist(&workspace_path, &plan, &completed.runs)?;
    store.append(&completed.schedule.attempts).await?;
    Ok(TestExecutionReport {
        plan,
        schedule: completed.schedule,
        runs: completed.runs,
    })
}

pub(crate) async fn run(
    workspace: &Path,
    graph: Option<Vec<once_frontend::GraphTarget>>,
    output: Output,
    plan: TestPlan,
    requested_workers: Option<usize>,
    sandbox: SandboxMode,
) -> Result<ExitCode> {
    let report = execute(workspace, graph, plan, requested_workers, sandbox).await?;
    let succeeded = report.succeeded();
    write_report(output, &report).await?;
    Ok(if succeeded {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn validate_workers(workers: Option<usize>) -> Result<()> {
    if let Some(workers) = workers {
        if workers == 0 {
            anyhow::bail!("test worker count must be greater than zero");
        }
        if workers > MAX_TEST_WORKERS {
            anyhow::bail!("test worker count must not exceed {MAX_TEST_WORKERS}");
        }
    }
    Ok(())
}

fn validate_plan_targets(
    workspace: &Path,
    graph: &[once_frontend::GraphTarget],
    plan: &TestPlan,
) -> Result<()> {
    for batch in &plan.batches {
        let target = graph
            .iter()
            .find(|target| target.label.id == batch.target)
            .with_context(|| format!("no target matches `{}`", batch.target))?;
        if !target
            .capabilities
            .iter()
            .any(|capability| capability.name == "test")
        {
            anyhow::bail!(
                "target `{}` does not expose the test capability",
                batch.target
            );
        }
        if !batch.test_filters.is_empty() {
            let manifest = crate::commands::query::test_manifest_record(workspace, &batch.target)?;
            for test_filter in &batch.test_filters {
                crate::commands::query::validate_test_unit(&manifest, &batch.target, test_filter)?;
            }
        }
    }
    Ok(())
}

async fn write_report(output: Output, report: &TestExecutionReport) -> Result<()> {
    let body = match output.format {
        Format::Human => render_human(report),
        Format::Json | Format::Toon => render::structured(output.format, report)?,
    };
    let mut stdout = tokio::io::stdout();
    stdout.write_all(body.as_bytes()).await?;
    stdout.flush().await?;
    Ok(())
}

fn render_human(report: &TestExecutionReport) -> String {
    let passed = report
        .runs
        .iter()
        .filter(|run| run.get("success").and_then(Value::as_bool) == Some(true))
        .count();
    format!(
        "once: ran {} test batches across {} local workers, {} passed, {} failed, {} ms\nplan: {}\nschedule: {}\n",
        report.runs.len(),
        report.schedule.workers,
        passed,
        report.runs.len().saturating_sub(passed),
        report.schedule.duration_ms,
        report.plan.id,
        report.schedule.id,
    )
}
