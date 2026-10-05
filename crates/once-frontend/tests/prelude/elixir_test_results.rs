//! The Elixir runner loads a formatter into the test VM so every `ExUnit` case
//! reports its own status, measured duration, and failure text. When the
//! formatter cannot be loaded without changing the project's setup, the
//! runner keeps the command untouched and scrapes the console totals.
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

fn write_runner(dir: &Path) {
    let prelude = all_prelude_source();
    std::fs::write(
        dir.join("runner.exs"),
        eval_prelude_string_function_in(&prelude, "_elixir_test_runner_source", "([])").unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.join("formatter.ex"),
        eval_prelude_string_function_in(&prelude, "_elixir_exunit_formatter_source", "()").unwrap(),
    )
    .unwrap();
}

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

fn run_runner(
    dir: &Path,
    runner: &str,
    runner_type: &str,
    args: &[&str],
) -> (Output, serde_json::Value) {
    std::fs::write(dir.join("args.list"), args.join("\n")).unwrap();
    std::fs::create_dir_all(dir.join("home")).unwrap();
    let output = Command::new(find_executable("elixir"))
        .arg("runner.exs")
        .arg(runner)
        .arg("args.list")
        .arg("results.json")
        .arg("test.log")
        .arg("native.txt")
        .arg("pkg/tests")
        .arg(runner_type)
        .arg("formatter.ex")
        .env("HOME", dir.join("home"))
        .env("MIX_HOME", dir.join("home/mix"))
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

fn assert_demo_cases(report: &serde_json::Value) {
    assert_eq!(
        report["summary"],
        serde_json::json!({"total": 4, "passed": 1, "failed": 2, "skipped": 1, "flaky": 0}),
        "{report}"
    );
    let slow = &case(report, "test passes slowly")["attempts"][0];
    assert_eq!(slow["status"], "passed");
    assert!(slow["duration_ms"].as_u64().unwrap() >= 50, "{report}");
    assert!(slow.get("failure").is_none(), "{report}");

    let failed = case(report, "test fails");
    assert_eq!(failed["suite"], "DemoTest");
    assert_eq!(failed["file"], "test/demo_test.exs");
    assert_eq!(failed["id"], "pkg/tests::DemoTest/test fails");
    let attempt = &failed["attempts"][0];
    assert_eq!(attempt["status"], "failed");
    assert!(attempt["duration_ms"].is_u64(), "{report}");
    let message = attempt["failure"]["message"].as_str().unwrap();
    assert!(message.contains("assert 1 + 1 == 3"), "{message}");

    let skipped = &case(report, "test skipped")["attempts"][0];
    assert_eq!(skipped, &serde_json::json!({"status": "skipped"}));

    let invalid = &case(report, "test never runs")["attempts"][0];
    assert_eq!(invalid["status"], "failed");
    assert!(invalid.get("duration_ms").is_none(), "{report}");
    assert!(invalid["failure"]["message"]
        .as_str()
        .unwrap()
        .contains("setup exploded"));
}

#[test]
fn exunit_runner_reports_cases_with_durations_and_failures() {
    let dir = tempfile::TempDir::new().unwrap();
    write_runner(dir.path());
    std::fs::create_dir_all(dir.path().join("test")).unwrap();
    std::fs::write(dir.path().join("test/demo_test.exs"), DEMO_TESTS).unwrap();
    let elixir = find_executable("elixir");

    let (output, report) = run_runner(
        dir.path(),
        &elixir,
        "elixir_exunit",
        &["-e", "ExUnit.start()", "-r", "test/demo_test.exs"],
    );

    assert!(!output.status.success(), "{output:?}");
    assert_eq!(report["status"], "failed");
    assert_demo_cases(&report);
}

fn write_mix_project(dir: &Path, test_helper: &str) {
    std::fs::create_dir_all(dir.join("test")).unwrap();
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    std::fs::write(
        dir.join("mix.exs"),
        "defmodule Demo.MixProject do\n  use Mix.Project\n  def project, do: [app: :demo, version: \"0.1.0\"]\nend\n",
    )
    .unwrap();
    std::fs::write(dir.join("test/test_helper.exs"), test_helper).unwrap();
    std::fs::write(dir.join("test/demo_test.exs"), DEMO_TESTS).unwrap();
}

#[test]
fn mix_runner_loads_the_formatter_past_code_path_pruning() {
    let dir = tempfile::TempDir::new().unwrap();
    write_runner(dir.path());
    write_mix_project(dir.path(), "ExUnit.start()\n");

    let (output, report) = run_runner(dir.path(), &find_executable("mix"), "mix_test", &["test"]);

    assert!(!output.status.success(), "{output:?}");
    assert_demo_cases(&report);
    let log = std::fs::read_to_string(dir.path().join("test.log")).unwrap();
    assert!(log.contains("setup exploded"), "{log}");
}

/// A project that configures its own formatters keeps them: the runner does
/// not pass `--formatter`, reports no cases, and still scrapes the totals.
#[test]
fn mix_runner_keeps_project_formatters_and_scrapes_totals() {
    let dir = tempfile::TempDir::new().unwrap();
    write_runner(dir.path());
    write_mix_project(
        dir.path(),
        "ExUnit.start(formatters: [ExUnit.CLIFormatter])\n",
    );
    std::fs::write(
        dir.path().join("test/demo_test.exs"),
        "defmodule DemoTest do\n  use ExUnit.Case\n  test \"works\", do: :ok\nend\n",
    )
    .unwrap();

    let (output, report) = run_runner(dir.path(), &find_executable("mix"), "mix_test", &["test"]);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(report["status"], "passed");
    assert_eq!(report["cases"], serde_json::json!([]));
    assert_eq!(report["summary"]["total"], 1, "{report}");
    assert_eq!(report["summary"]["passed"], 1, "{report}");
}

/// A formatter that cannot be compiled leaves the run exactly as it was.
#[test]
fn exunit_runner_falls_back_when_the_formatter_cannot_load() {
    let dir = tempfile::TempDir::new().unwrap();
    write_runner(dir.path());
    std::fs::write(dir.path().join("formatter.ex"), "defmodule Broken do\n").unwrap();
    std::fs::create_dir_all(dir.path().join("test")).unwrap();
    std::fs::write(
        dir.path().join("test/demo_test.exs"),
        "defmodule DemoTest do\n  use ExUnit.Case\n  test \"works\", do: :ok\nend\n",
    )
    .unwrap();
    let elixir = find_executable("elixir");

    let (output, report) = run_runner(
        dir.path(),
        &elixir,
        "elixir_exunit",
        &["-e", "ExUnit.start()", "-r", "test/demo_test.exs"],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(report["status"], "passed");
    assert_eq!(report["cases"], serde_json::json!([]));
    assert_eq!(report["summary"]["passed"], 1, "{report}");
}

/// The formatter's variables never reach processes the tests start, and a
/// configured `ERL_AFLAGS` is handed back to them unchanged.
#[test]
fn exunit_runner_keeps_its_variables_out_of_test_processes() {
    let dir = tempfile::TempDir::new().unwrap();
    write_runner(dir.path());
    std::fs::create_dir_all(dir.path().join("test")).unwrap();
    std::fs::write(
        dir.path().join("test/env_test.exs"),
        "defmodule EnvTest do\n  use ExUnit.Case\n  test \"sees the original environment\" do\n    assert System.get_env(\"ONCE_EXUNIT_CASES\") == nil\n    assert System.get_env(\"ONCE_EXUNIT_ERL_AFLAGS\") == nil\n    assert System.get_env(\"ERL_AFLAGS\") == \"+pc unicode\"\n  end\nend\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("args.list"),
        "-e\nExUnit.start()\n-r\ntest/env_test.exs",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("home")).unwrap();
    let elixir = find_executable("elixir");
    let output = Command::new(&elixir)
        .args([
            "runner.exs",
            &elixir,
            "args.list",
            "results.json",
            "test.log",
            "native.txt",
            "pkg/tests",
            "elixir_exunit",
            "formatter.ex",
        ])
        .env("HOME", dir.path().join("home"))
        .env("ERL_AFLAGS", "+pc unicode")
        .current_dir(dir.path())
        .output()
        .unwrap();

    let log = std::fs::read_to_string(dir.path().join("test.log")).unwrap_or_default();
    assert!(output.status.success(), "{output:?}\n{log}");
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("results.json")).unwrap())
            .unwrap();
    assert_eq!(report["cases"][0]["status"], "passed", "{report}");
}
