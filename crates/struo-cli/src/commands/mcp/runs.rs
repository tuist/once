//! Child `struo` invocations behind the build, run, and test tools.

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use serde::Serialize;
use struo_core::ResourceLimits;

use super::RunTestsArgs;

#[cfg(test)]
pub(super) fn run_test_plan(
    workspace: &std::path::Path,
    args: &Value,
) -> Result<crate::commands::query::test_plan::TestPlan> {
    let args: RunTestsArgs = super::parse_args(args)?;
    run_test_plan_args(workspace, &args)
}

pub(super) fn run_test_plan_args(
    workspace: &std::path::Path,
    args: &RunTestsArgs,
) -> Result<crate::commands::query::test_plan::TestPlan> {
    anyhow::ensure!(
        args.test_unit.is_none() || args.test_batch.is_none(),
        "`test_unit` and `test_batch` are mutually exclusive"
    );
    anyhow::ensure!(
        (args.test_unit.is_none() && args.test_batch.is_none()) || args.changed_paths.is_empty(),
        "exact unit or batch requests and `changed_paths` are mutually exclusive"
    );
    let mut targets = args.targets.clone();
    if let Some(target) = &args.target {
        targets.push(target.clone());
    }
    if !targets.is_empty() {
        targets.sort();
        targets.dedup();
        validate_test_targets(workspace, &targets)?;
        if let Some(test_batch) = &args.test_batch {
            anyhow::ensure!(
                targets.len() == 1,
                "`test_batch` requires exactly one explicit test target"
            );
            return crate::commands::query::explicit_test_batch_plan(
                workspace,
                &targets[0],
                test_batch,
            );
        }
        if let Some(test_unit) = &args.test_unit {
            if targets.len() != 1 {
                anyhow::bail!("`test_unit` requires exactly one explicit test target");
            }
            return crate::commands::query::explicit_test_unit_plan(
                workspace,
                &targets[0],
                test_unit,
            );
        }
        return crate::commands::query::explicit_test_plan(workspace, &targets);
    }

    if args.test_unit.is_some() || args.test_batch.is_some() {
        anyhow::bail!("`test_unit` or `test_batch` requires an explicit `target`");
    }

    let plan = crate::commands::query::test_plan_for_paths(workspace, &args.changed_paths)?;
    if plan.batches.is_empty() {
        anyhow::bail!("no test targets matched the requested inputs");
    }
    let targets = plan
        .batches
        .iter()
        .map(|batch| batch.target.clone())
        .collect::<Vec<_>>();
    validate_test_targets(workspace, &targets)?;
    Ok(plan)
}

pub(super) fn summarize_test_run_results(value: &mut Value) -> Result<()> {
    let runs = value
        .get_mut("runs")
        .and_then(Value::as_array_mut)
        .context("test run report is missing `runs`")?;
    for run in runs {
        let Some(results) = run.get_mut("results") else {
            continue;
        };
        if results.is_null() {
            continue;
        }
        *results = crate::commands::query::test_results_summary_value(results)?;
    }
    Ok(())
}

pub(super) fn validate_test_targets(workspace: &std::path::Path, targets: &[String]) -> Result<()> {
    let graph = struo_frontend::load_graph_workspace(workspace).context("loading graph")?;
    for target_id in targets {
        crate::commands::query::target_id_path(target_id)?;
        let target = graph
            .iter()
            .find(|target| target.label.id == *target_id)
            .with_context(|| format!("no target matches `{target_id}`"))?;
        if !target
            .capabilities
            .iter()
            .any(|capability| capability.name == "test")
        {
            anyhow::bail!("target `{target_id}` does not expose the test capability");
        }
    }
    Ok(())
}

pub(super) fn run_graph_target(
    workspace: &std::path::Path,
    capability: &str,
    target: &str,
    visible: bool,
    arguments: &[String],
    resource_limits: &ResourceLimits,
) -> Result<Value> {
    let exe = std::env::current_exe().context("resolving current struo executable")?;
    run_graph_target_with_exe_and_memory_limit(
        &exe,
        workspace,
        capability,
        target,
        visible,
        arguments,
        resource_limits.memory_bytes,
    )
}

#[cfg(test)]
pub(super) fn run_graph_target_with_exe(
    exe: &std::path::Path,
    workspace: &std::path::Path,
    capability: &str,
    target: &str,
    visible: bool,
    arguments: &[String],
) -> Result<Value> {
    run_graph_target_with_exe_and_memory_limit(
        exe, workspace, capability, target, visible, arguments, 0,
    )
}

pub(super) fn run_graph_target_with_exe_and_memory_limit(
    exe: &std::path::Path,
    workspace: &std::path::Path,
    capability: &str,
    target: &str,
    visible: bool,
    arguments: &[String],
    memory_limit_bytes: u64,
) -> Result<Value> {
    let mut command = std::process::Command::new(exe);
    command
        .current_dir(workspace)
        .arg("-C")
        .arg(workspace)
        .arg("--format")
        .arg("json");
    if memory_limit_bytes > 0 {
        command
            .arg("--memory-limit")
            .arg(memory_limit_bytes.to_string());
    }
    command.arg(capability);
    if visible {
        command.arg("--visible");
    }
    command.arg(target);
    if !arguments.is_empty() {
        command.arg("--").args(arguments);
    }
    let output = crate::commands::util::capture_command_output(&mut command)
        .with_context(|| format!("running `{}` {capability} `{target}`", exe.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let (record, record_parse_error) = parse_json_record(&stdout);
    let error = (!output.status.success())
        .then(|| parse_structured_dispatch_error(&stderr))
        .flatten();
    let captured_stdout = captured_action_log(workspace, &record, "stdout.log");
    let captured_stderr = captured_action_log(workspace, &record, "stderr.log");
    Ok(json!({
        "target": target,
        "capability": capability,
        "exit_code": output.status.code().unwrap_or(-1),
        "success": output.status.success(),
        "record": record,
        "record_parse_error": record_parse_error,
        "stdout_truncated": output.stdout_truncated,
        "stderr_truncated": output.stderr_truncated,
        "error": error,
        "captured_stdout": captured_stdout,
        "captured_stderr": captured_stderr,
        "stderr": stderr,
    }))
}

pub(super) fn parse_structured_dispatch_error(stderr: &str) -> Option<Value> {
    stderr.lines().rev().find_map(|line| {
        serde_json::from_str::<Value>(line)
            .ok()?
            .get("error")
            .cloned()
    })
}

#[derive(Serialize)]
pub(super) struct CapturedActionLog {
    pub(super) path: String,
    pub(super) text: String,
    pub(super) byte_count: usize,
    pub(super) truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) read_error: Option<String>,
}

pub(super) fn captured_action_log(
    workspace: &std::path::Path,
    record: &Value,
    file_name: &str,
) -> Option<CapturedActionLog> {
    const MAX_CAPTURE_BYTES: usize = 64 * 1024;
    let relative = record
        .get("outputs")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .find(|path| {
            Path::new(path).file_name().and_then(|name| name.to_str()) == Some(file_name)
        })?;
    let workspace_path = match struo_core::WorkspacePath::try_from(relative) {
        Ok(path) => path,
        Err(error) => {
            return Some(CapturedActionLog {
                path: relative.to_string(),
                text: String::new(),
                byte_count: 0,
                truncated: false,
                read_error: Some(error.to_string()),
            });
        }
    };
    let path = workspace_path.resolve(workspace);
    let mut bytes = Vec::with_capacity(MAX_CAPTURE_BYTES + 1);
    let read = std::fs::File::open(&path).and_then(|file| {
        file.take((MAX_CAPTURE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
    });
    if let Err(error) = read {
        return Some(CapturedActionLog {
            path: relative.to_string(),
            text: String::new(),
            byte_count: 0,
            truncated: false,
            read_error: Some(error.to_string()),
        });
    }
    let byte_count = std::fs::metadata(&path).map_or(bytes.len(), |metadata| {
        usize::try_from(metadata.len()).unwrap_or(usize::MAX)
    });
    let truncated = bytes.len() > MAX_CAPTURE_BYTES;
    if truncated {
        bytes.truncate(MAX_CAPTURE_BYTES);
    }
    Some(CapturedActionLog {
        path: relative.to_string(),
        text: String::from_utf8_lossy(&bytes).into_owned(),
        byte_count,
        truncated,
        read_error: None,
    })
}

pub(super) fn parse_json_record(stdout: &str) -> (Value, Option<String>) {
    if stdout.is_empty() {
        return (Value::Null, None);
    }
    match serde_json::from_str(stdout) {
        Ok(value) => (value, None),
        Err(err) => (Value::String(stdout.to_string()), Some(err.to_string())),
    }
}
