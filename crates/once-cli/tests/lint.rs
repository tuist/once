#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use serde_json::Value;

#[test]
fn lint_returns_normalized_findings_from_an_accepted_exit_code() {
    let workspace = tempfile::tempdir().unwrap();
    let fake_ruff = workspace.path().join("fake-ruff");
    std::fs::write(
        &fake_ruff,
        r#"#!/usr/bin/env bash
if [[ "$1" == "--version" ]]; then
  printf 'ruff 1.0.0\n'
  exit 0
fi
report=''
previous=''
for argument in "$@"; do
  if [[ "$previous" == "--output-file" ]]; then
    report="$argument"
  fi
  previous="$argument"
done
mkdir -p "$(dirname "$report")"
printf '%s\n' '{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"Ruff","rules":[]}},"results":[{"ruleId":"F401","level":"warning","message":{"text":"imported but unused"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"main.py"},"region":{"startLine":1,"startColumn":1}}}]}]}]}' > "$report"
exit 1
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&fake_ruff).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake_ruff, permissions).unwrap();

    std::fs::write(workspace.path().join("main.py"), "import os\n").unwrap();
    std::fs::write(
        workspace.path().join("once.toml"),
        format!(
            "[[target]]\nname = \"lint\"\nkind = \"ruff_lint\"\nsrcs = [\"*.py\"]\n\n[target.attrs]\nruff = {:?}\n",
            fake_ruff.to_string_lossy()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_once"))
        .arg("-C")
        .arg(workspace.path())
        .arg("--format")
        .arg("json")
        .arg("lint")
        .arg("lint")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let record: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(record["schema"], "once.lint_results.v1");
    assert_eq!(record["summary"]["warnings"], 1);
    assert_eq!(record["findings"][0]["rule_id"], "F401");
    assert_eq!(
        record["findings"][0]["location"]["path"],
        Value::String("main.py".to_string())
    );
}

#[test]
fn invalid_lint_provider_returns_diagnostics_before_actions_run() {
    let workspace = tempfile::tempdir().unwrap();
    let modules = workspace.path().join("modules");
    std::fs::create_dir_all(&modules).unwrap();
    std::fs::write(
        modules.join("lint.star"),
        r#"
def _invalid_lint_impl(ctx):
    write_path(ctx["build_dir"] + "/action-ran.txt", "ran")
    return {
        "lint_info": {
            "outputs": {
                "sarif": ctx["build_dir"] + "/report.sarif",
            },
        },
    }

invalid_lint = target_kind(
    providers = ["once_lint_info"],
    capabilities = [capability("lint", ["default", "lint_results"])],
    impl = _invalid_lint_impl,
)
"#,
    )
    .unwrap();
    std::fs::write(
        workspace.path().join("once.toml"),
        "[modules]\npaths = [\"modules/*.star\"]\n\n[[target]]\nname = \"lint\"\nkind = \"invalid_lint\"\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_once"))
        .arg("-C")
        .arg(workspace.path())
        .arg("--format")
        .arg("json")
        .arg("lint")
        .arg("lint")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let error_line = stderr.lines().last().expect("structured error line");
    let error: Value = serde_json::from_str(error_line).unwrap_or_else(|error| {
        panic!("structured lint error did not decode: {error}; stderr: {stderr}")
    });
    assert_eq!(
        error["error"]["code"],
        Value::String("invalid_lint_provider_output".to_string())
    );
    assert_eq!(
        error["error"]["diagnostics"][0]["attribute"],
        Value::String("lint_info.outputs.results".to_string())
    );
    assert!(!workspace
        .path()
        .join(".once/out/lint/action-ran.txt")
        .exists());
}
