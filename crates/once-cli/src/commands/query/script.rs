use std::fmt::Write as _;
use std::path::Path;

use anyhow::Result;
use once_core::WorkspacePath;
use once_frontend::{Diagnostic, ScriptAnnotations};
use serde::Serialize;
use serde_json::Value;

use crate::cli::Output;

#[derive(Debug, Serialize)]
struct ScriptContract {
    path: String,
    runtime: String,
    runtime_args: Vec<String>,
    needs: Vec<String>,
    fingerprints: Vec<String>,
    inputs: Vec<String>,
    outputs: Vec<String>,
    env_vars: Vec<String>,
    cwd: Option<String>,
    remote: Option<String>,
    output_symlinks: Option<String>,
}

impl ScriptContract {
    fn new(path: String, annotations: ScriptAnnotations) -> Self {
        Self {
            path,
            runtime: annotations.runtime,
            runtime_args: annotations.runtime_args,
            needs: annotations.needs,
            fingerprints: annotations.fingerprints,
            inputs: annotations.inputs,
            outputs: annotations.outputs,
            env_vars: annotations.env_vars,
            cwd: annotations.cwd,
            remote: annotations.remote,
            output_symlinks: annotations.output_symlinks,
        }
    }
}

#[derive(Debug, Serialize)]
struct ScriptValidation {
    valid: bool,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    contract: Option<ScriptContract>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    diagnostics: Vec<Diagnostic>,
}

pub(super) async fn inspect(workspace: &Path, output: Output, path: &str) -> Result<()> {
    let validation = validate_script(workspace, path);
    super::write_body(
        output,
        || render_script_validation(&validation),
        &validation,
    )
    .await
}

pub(crate) fn script_validation_value(workspace: &Path, path: &str) -> Result<Value> {
    Ok(serde_json::to_value(validate_script(workspace, path))?)
}

fn validate_script(workspace: &Path, path: &str) -> ScriptValidation {
    let workspace_path = match WorkspacePath::try_from(path) {
        Ok(path) => path,
        Err(error) => return invalid_script(path, "invalid_script_path", error.to_string()),
    };
    let absolute = workspace_path.resolve(workspace);
    match once_frontend::parse_script_annotations(&absolute, path) {
        Ok(annotations) => ScriptValidation {
            valid: true,
            path: workspace_path.to_string(),
            contract: Some(ScriptContract::new(workspace_path.to_string(), annotations)),
            diagnostics: Vec::new(),
        },
        Err(error) => invalid_script(path, "invalid_script_contract", error.to_string()),
    }
}

fn invalid_script(path: &str, code: &str, message: String) -> ScriptValidation {
    ScriptValidation {
        valid: false,
        path: path.to_string(),
        contract: None,
        diagnostics: vec![Diagnostic::new(code, message)
            .with_target(path)
            .with_attribute("path")
            .with_repair("Fix the script path, shebang, or `once` directives and validate again")],
    }
}

fn render_script_validation(validation: &ScriptValidation) -> String {
    if let Some(contract) = &validation.contract {
        let mut out = format!(
            "valid script: {}\nruntime: {}\n",
            contract.path, contract.runtime
        );
        if !contract.inputs.is_empty() {
            let _ = writeln!(out, "inputs: {}", contract.inputs.join(", "));
        }
        if !contract.outputs.is_empty() {
            let _ = writeln!(out, "outputs: {}", contract.outputs.join(", "));
        }
        return out;
    }
    let mut out = format!("invalid script: {}\n", validation.path);
    for diagnostic in &validation.diagnostics {
        let _ = writeln!(out, "{}: {}", diagnostic.code, diagnostic.message);
    }
    out
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn returns_the_annotated_script_contract() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("build.sh"),
            "#!/bin/sh\n# once input \"input.txt\"\n# once output \"output.txt\"\ncp input.txt output.txt\n",
        )
        .unwrap();

        let value = script_validation_value(tmp.path(), "build.sh").unwrap();

        assert_eq!(value["valid"], true);
        assert_eq!(value["contract"]["runtime"], "/bin/sh");
        assert_eq!(
            value["contract"]["inputs"],
            serde_json::json!(["input.txt"])
        );
        assert_eq!(
            value["contract"]["outputs"],
            serde_json::json!(["output.txt"])
        );
    }

    #[test]
    fn returns_structured_diagnostics_for_invalid_directives() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("build.sh"),
            "#!/bin/sh\n# once ouput \"output.txt\"\ntrue\n",
        )
        .unwrap();

        let value = script_validation_value(tmp.path(), "build.sh").unwrap();

        assert_eq!(value["valid"], false);
        assert_eq!(value["diagnostics"][0]["code"], "invalid_script_contract");
        assert!(value["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("unknown once directive"));
    }
}
