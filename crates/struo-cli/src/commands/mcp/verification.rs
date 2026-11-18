use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use struo_core::ResourceLimits;

pub(super) fn execute(
    workspace: &Path,
    resource_limits: &ResourceLimits,
    arguments: &[String],
) -> Result<Value> {
    let executable = std::env::current_exe().context("resolving current struo executable")?;
    let mut command = std::process::Command::new(executable);
    command
        .current_dir(workspace)
        .arg("-C")
        .arg(workspace)
        .args(["--format", "json"])
        .arg("--memory-limit")
        .arg(resource_limits.memory_bytes.to_string())
        .arg("--cpu-limit")
        .arg(resource_limits.cpu_slots.to_string())
        .args(arguments);
    let output = crate::commands::util::capture_command_output(&mut command)?;
    if output.stdout_truncated {
        anyhow::bail!("verification response exceeded the capture limit; inspect the retained report with struo query verification");
    }
    match serde_json::from_slice::<Value>(&output.stdout) {
        Ok(report)
            if report.get("schema").and_then(Value::as_str) == Some("struo.verification.v1") =>
        {
            Ok(report)
        }
        _ => Ok(
            json!({ "success": false, "exit_code": output.status.code().unwrap_or(-1), "stderr": String::from_utf8_lossy(&output.stderr), "stderr_truncated": output.stderr_truncated }),
        ),
    }
}
