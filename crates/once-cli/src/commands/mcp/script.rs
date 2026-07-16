use std::path::Path;

use anyhow::{Context, Result};
use once_core::WorkspacePath;
use serde::Deserialize;
use serde_json::{json, Value};

use super::tool_args;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptPathArgs {
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecuteScriptArgs {
    path: String,
    #[serde(default)]
    args: Vec<String>,
}

pub(super) fn validate(workspace: &Path, args: &Value) -> Result<Value> {
    let args: ScriptPathArgs = serde_json::from_value(tool_args(args))?;
    crate::commands::query::script_validation_value(workspace, &args.path)
}

pub(super) fn execute(workspace: &Path, args: &Value) -> Result<Value> {
    let args: ExecuteScriptArgs = serde_json::from_value(tool_args(args))?;
    let executable = std::env::current_exe().context("resolving current once executable")?;
    execute_with_executable(&executable, workspace, &args)
}

fn execute_with_executable(
    executable: &Path,
    workspace: &Path,
    args: &ExecuteScriptArgs,
) -> Result<Value> {
    let script_path = WorkspacePath::try_from(args.path.as_str())?;
    let absolute = script_path.resolve(workspace);
    let annotations = once_frontend::parse_script_annotations(&absolute, &args.path)
        .with_context(|| format!("validating annotated script `{}`", args.path))?;

    let mut command = std::process::Command::new(executable);
    command
        .arg("-C")
        .arg(workspace)
        .arg("--format")
        .arg("json")
        .arg("exec")
        .arg("--script")
        .arg("--")
        .arg(&annotations.runtime)
        .args(&annotations.runtime_args)
        .arg(script_path.as_str())
        .args(&args.args);
    let output = command
        .output()
        .with_context(|| format!("executing annotated script `{}`", args.path))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let raw_stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let (stderr, record) = split_exec_trailer(&raw_stderr);
    let evidence_subject = record
        .get("action_digest")
        .and_then(Value::as_str)
        .map(str::to_string);
    let evidence = evidence_subject.as_deref().map_or_else(
        || Ok(Vec::new()),
        |subject| {
            let workspace = workspace.to_path_buf();
            let subject = subject.to_string();
            super::run_async_result(async move {
                crate::commands::query::evidence_records(&workspace, Some(&subject), None).await
            })
        },
    )?;

    Ok(json!({
        "path": script_path.as_str(),
        "success": output.status.success(),
        "exit_code": output.status.code().unwrap_or(-1),
        "stdout": stdout,
        "stderr": stderr,
        "record": record,
        "evidence_subject": evidence_subject,
        "evidence": evidence,
    }))
}

fn split_exec_trailer(stderr: &str) -> (String, Value) {
    for (index, _) in stderr.match_indices('{').rev() {
        let candidate = stderr[index..].trim();
        let Ok(value) = serde_json::from_str::<Value>(candidate) else {
            continue;
        };
        if value.get("action_digest").and_then(Value::as_str).is_some()
            && value.get("cache").and_then(Value::as_str).is_some()
        {
            return (stderr[..index].trim_end().to_string(), value);
        }
    }
    (stderr.trim_end().to_string(), Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_script_stderr_from_the_structured_trailer() {
        let (stderr, trailer) = split_exec_trailer(
            "script warning\n{\"action_digest\":\"abc\",\"cache\":\"hit\",\"exit_code\":0}\n",
        );

        assert_eq!(stderr, "script warning");
        assert_eq!(trailer["action_digest"], "abc");
        assert_eq!(trailer["cache"], "hit");
    }

    #[test]
    fn validate_returns_structured_script_diagnostics() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("bad.sh"),
            "#!/bin/sh\n# once ouput \"result.txt\"\ntrue\n",
        )
        .unwrap();

        let value = validate(tmp.path(), &json!({ "path": "bad.sh" })).unwrap();

        assert_eq!(value["valid"], false);
        assert_eq!(value["diagnostics"][0]["code"], "invalid_script_contract");
    }
}
