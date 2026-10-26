use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{ensure, Context, Result};
use once_core::{ResourceLimits, SandboxMode};
use once_events_client::proto::TestRunContext;
use serde_json::json;

use crate::cli::Output;
use crate::commands::query::test_plan::portable::PortableTestPlan;

#[derive(Clone)]
pub(crate) struct Invocation {
    pub path: PathBuf,
    pub external_group: Option<String>,
    pub partition_index: Option<u32>,
}

impl Invocation {
    pub fn context(&self, plan_id: &str, batch_id: &str) -> Result<TestRunContext> {
        ensure!(
            self.external_group.is_some() == self.partition_index.is_some(),
            "external group and partition index must be supplied together"
        );
        let external_group_id = self
            .external_group
            .as_deref()
            .map(uuid::Uuid::parse_str)
            .transpose()
            .context("external test group must be a UUID")?
            .map(|id| id.to_string())
            .unwrap_or_default();
        Ok(TestRunContext {
            external_group_id,
            partition_index: self.partition_index.unwrap_or(0),
            plan_id: plan_id.to_string(),
            batch_id: batch_id.to_string(),
        })
    }

    pub fn add_to_command(&self, command: &mut std::process::Command, batch_id: &str) {
        command
            .env_remove("TUIST_SHARD_PLAN_ID")
            .arg("--plan")
            .arg(&self.path)
            .arg("--batch")
            .arg(batch_id)
            .arg("--internal-test-batch");
        if let (Some(group), Some(index)) = (&self.external_group, self.partition_index) {
            command
                .arg("--external-group")
                .arg(group)
                .arg("--partition-index")
                .arg(index.to_string());
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn run_batch(
        &self,
        workspace: &Path,
        graph: &[once_frontend::GraphTarget],
        output: Output,
        batches: &[String],
        sandbox: SandboxMode,
        resource_limits: ResourceLimits,
        resolved: &crate::commands::graph::ResolvedConfiguration,
    ) -> Result<ExitCode> {
        let portable = PortableTestPlan::read(&self.path)?;
        portable.validate(workspace, graph)?;
        ensure!(
            batches.len() == 1,
            "a batch subprocess requires exactly one batch"
        );
        let selected = portable.select(batches)?;
        let batch = &selected.batches[0];
        let manifest = portable
            .bindings
            .iter()
            .find(|binding| binding.target == batch.target)
            .and_then(|binding| binding.manifest.as_ref());
        let context = self.context(&portable.id, &batch.id)?;
        let xdg = once_core::Xdg::from_env();
        let cache = crate::cache_provider::resolve(workspace, &xdg)?;
        Box::pin(crate::commands::graph::test_with_filters(
            workspace,
            &cache,
            output,
            &batch.target,
            sandbox,
            &batch.test_filters,
            (!batch.test_filters.is_empty()).then_some(batch.id.as_str()),
            resource_limits,
            resolved,
            manifest,
            Some(context),
        ))
        .await
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    workspace: &Path,
    graph: Vec<once_frontend::GraphTarget>,
    output: Output,
    invocation: Invocation,
    batches: &[String],
    requested_workers: Option<usize>,
    sandbox: SandboxMode,
    resource_limits: ResourceLimits,
) -> Result<ExitCode> {
    let value = execute(
        workspace,
        graph,
        invocation,
        batches,
        requested_workers,
        sandbox,
        resource_limits,
        false,
    )
    .await?;
    let success = value["success"] == true;
    crate::commands::query::write_body(
        output,
        || {
            format!(
                "{}\n",
                if success {
                    "Test partition passed"
                } else {
                    "Test partition failed or incomplete"
                }
            )
        },
        &value,
    )
    .await?;
    Ok(if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute(
    workspace: &Path,
    graph: Vec<once_frontend::GraphTarget>,
    invocation: Invocation,
    batches: &[String],
    requested_workers: Option<usize>,
    sandbox: SandboxMode,
    resource_limits: ResourceLimits,
    reporting: bool,
) -> Result<serde_json::Value> {
    let portable = PortableTestPlan::read(&invocation.path)?;
    portable.validate(workspace, &graph)?;
    invocation.context(&portable.id, "")?;
    let selected = portable.select(batches)?;
    let selected_batch_ids = selected
        .batches
        .iter()
        .map(|batch| batch.id.clone())
        .collect::<Vec<_>>();
    // Children read a stable copy, not a coordinator file that can be replaced
    // between parent validation and child startup.
    let file = tempfile::NamedTempFile::new()?;
    serde_json::to_writer(file.as_file(), &portable)?;
    let invocation = Invocation {
        path: file.path().to_path_buf(),
        ..invocation
    };
    let report = super::execute_validated(
        workspace,
        graph,
        selected,
        requested_workers,
        sandbox,
        resource_limits,
        Some(invocation),
        reporting,
    )
    .await?;
    let complete = report.runs.len() == selected_batch_ids.len()
        && report.runs.iter().all(|run| {
            run.get("error").is_none()
                && run["exit_code"].as_i64().is_some_and(|code| code >= 0)
                && (report.plan.batches.iter().any(|batch| {
                    batch.id == run["batch_id"]
                        && batch.test_filters.is_empty()
                        && run["success"] == true
                        && run["record"]["status"] == "completed"
                }) || (!run["results"].is_null() && run["results_error"].is_null()))
        });
    let success = complete && report.succeeded();
    Ok(json!({
        "schema": "once.portable_test_execution.v1", "plan_id": portable.id,
        "selected_batch_ids": selected_batch_ids, "complete": complete,
        "success": success, "execution": report,
    }))
}
