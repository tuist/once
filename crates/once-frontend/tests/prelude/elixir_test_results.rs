//! The Mix test runner loads a formatter into the test VM so every `ExUnit`
//! case reports its own status, measured duration, and failure text, instead
//! of only the run's totals.
#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output};

use super::{all_prelude_source, eval_prelude_string_function_in};

const DEMO_TESTS: &str = r#"defmodule DemoTest do
  use ExUnit.Case

  test "passes slowly" do
    Process.sleep(60)
  end

  test "fails" do
    assert 1 + 1 == 3
  end

  @tag :skip
  test "skipped" do
  end
end

defmodule BrokenSetupTest do
  use ExUnit.Case

  setup_all do
    raise "setup exploded"
  end

  test "never runs", do: :ok
end
"#;

fn find_executable(name: &str) -> String {
    let output = Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name}"))
        .output()
        .unwrap();
    let path = String::from_utf8(output.stdout).unwrap().trim().to_string();
    assert!(!path.is_empty(), "`{name}` must be on PATH");
    path
}

fn write_mix_project(dir: &Path) {
    std::fs::create_dir_all(dir.join("test")).unwrap();
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    std::fs::write(
        dir.join("mix.exs"),
        "defmodule Demo.MixProject do\n  use Mix.Project\n  def project, do: [app: :demo, version: \"0.1.0\"]\nend\n",
    )
    .unwrap();
    std::fs::write(dir.join("test/test_helper.exs"), "ExUnit.start()\n").unwrap();
    std::fs::write(dir.join("test/demo_test.exs"), DEMO_TESTS).unwrap();
}

/// Runs the Mix test runner the way the `elixir_test` action does: the
/// runner script drives `elixir -e <mix test command> -- <test args>`.
fn run_mix_runner(dir: &Path) -> (Output, serde_json::Value) {
    let prelude = all_prelude_source();
    std::fs::write(
        dir.join("runner.exs"),
        eval_prelude_string_function_in(&prelude, "_elixir_test_runner_source", "([], \"mix\")")
            .unwrap(),
    )
    .unwrap();
    let command =
        eval_prelude_string_function_in(&prelude, "_mix_test_command_source", "()").unwrap();
    std::fs::write(
        dir.join("args.json"),
        serde_json::to_string(&[
            "-e",
            command.as_str(),
            "--",
            "--no-compile",
            "--no-deps-check",
            // The action stages a compiled application; these fixtures have
            // no build, so there is no application to start.
            "--no-start",
        ])
        .unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("home")).unwrap();
    let elixir = find_executable("elixir");
    let output = Command::new(&elixir)
        .args([
            "runner.exs",
            &elixir,
            "args.json",
            "results.json",
            "test.log",
            "native.txt",
            "pkg/tests",
            "mix_test",
            "test.log",
            "native.txt",
        ])
        .env("HOME", dir.join("home"))
        .env("MIX_HOME", dir.join("home/mix"))
        .env("MIX_ENV", "test")
        .current_dir(dir)
        .output()
        .unwrap();
    let report = serde_json::from_str(
        &std::fs::read_to_string(dir.join("results.json"))
            .unwrap_or_else(|error| panic!("missing results: {error}\n{output:?}")),
    )
    .unwrap();
    (output, report)
}

fn case<'a>(report: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    report["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .unwrap_or_else(|| panic!("missing case {name}: {report}"))
}

#[test]
fn mix_runner_reports_cases_with_durations_and_failures() {
    let dir = tempfile::TempDir::new().unwrap();
    write_mix_project(dir.path());

    let (output, report) = run_mix_runner(dir.path());

    let log = std::fs::read_to_string(dir.path().join("test.log")).unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(report["status"], "failed");
    assert_eq!(
        report["summary"],
        serde_json::json!({"total": 4, "passed": 1, "failed": 2, "skipped": 1, "flaky": 0}),
        "{report}\n{log}"
    );
    assert_eq!(report["cases"].as_array().unwrap().len(), 4, "{report}");

    let slow = &case(&report, "test passes slowly")["attempts"][0];
    assert_eq!(slow["status"], "passed");
    assert!(slow["duration_ms"].as_u64().unwrap() >= 50, "{report}");
    assert!(slow.get("failure").is_none(), "{report}");

    let failed = case(&report, "test fails");
    assert_eq!(failed["suite"], "DemoTest");
    assert_eq!(failed["file"], "test/demo_test.exs");
    assert_eq!(failed["id"], "pkg/tests::DemoTest/test fails");
    assert_eq!(failed["runner_metadata"]["line"], 8);
    let attempt = &failed["attempts"][0];
    assert_eq!(attempt["status"], "failed");
    assert!(attempt["duration_ms"].is_u64(), "{report}");
    let message = attempt["failure"]["message"].as_str().unwrap();
    assert!(message.contains("assert 1 + 1 == 3"), "{message}");

    let skipped = &case(&report, "test skipped")["attempts"][0];
    assert_eq!(skipped["status"], "skipped");
    assert!(skipped.get("failure").is_none(), "{report}");

    let invalid = &case(&report, "test never runs")["attempts"][0];
    assert_eq!(invalid["status"], "failed");
    assert!(invalid["failure"]["message"]
        .as_str()
        .unwrap()
        .contains("setup exploded"));

    assert!(log.contains("setup exploded"), "{log}");
}

#[test]
fn a_passing_mix_run_reports_every_case_as_passed() {
    let dir = tempfile::TempDir::new().unwrap();
    write_mix_project(dir.path());
    std::fs::write(
        dir.path().join("test/demo_test.exs"),
        "defmodule DemoTest do\n  use ExUnit.Case\n  test \"works\", do: :ok\n  test \"also works\", do: :ok\nend\n",
    )
    .unwrap();

    let (output, report) = run_mix_runner(dir.path());

    assert!(output.status.success(), "{output:?}");
    assert_eq!(report["status"], "passed");
    assert_eq!(report["summary"]["total"], 2, "{report}");
    assert_eq!(report["summary"]["passed"], 2, "{report}");
    let mut names: Vec<&str> = report["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| case["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["test also works", "test works"], "{report}");
}
