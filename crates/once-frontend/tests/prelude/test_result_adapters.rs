//! Each language runner normalizes its native report into
//! `once.test_results.v1`. These tests drive the embedded runners against
//! canned native output and check that every attempt carries the duration
//! the native runner measured and the failure message it recorded, so the
//! run events report them instead of zero durations and empty failures.
#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output};

use super::{all_prelude_source, eval_prelude_string_function_in, write_executable};

fn runner_source(function: &str) -> String {
    eval_prelude_string_function_in(all_prelude_source(), function, "()").unwrap()
}

fn run(command: &mut Command) -> Output {
    command
        .output()
        .unwrap_or_else(|error| panic!("failed to spawn {command:?}: {error}"))
}

fn read_report(path: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("missing {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap()
}

fn only_attempt<'a>(report: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    let case = report["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .unwrap_or_else(|| panic!("missing case {name}: {report}"));
    let attempts = case["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1, "{report}");
    &attempts[0]
}

#[test]
fn javascript_adapter_reports_assertion_durations_and_failure_messages() {
    let dir = tempfile::TempDir::new().unwrap();
    let adapter = dir.path().join("adapter.mjs");
    std::fs::write(&adapter, runner_source("_javascript_test_adapter")).unwrap();
    let runner = dir.path().join("fake-runner.mjs");
    std::fs::write(
        &runner,
        r#"import fs from "node:fs"
const output = process.argv.find(arg => arg.startsWith("--outputFile=")).slice("--outputFile=".length)
fs.writeFileSync(output, JSON.stringify({ testResults: [{ name: "tests/math.test.js", assertionResults: [
  { title: "adds", fullName: "math adds", ancestorTitles: ["math"], status: "passed", duration: 12.6, failureMessages: [] },
  { title: "divides", fullName: "math divides", ancestorTitles: ["math"], status: "failed", duration: 3, failureMessages: ["\u001b[31mExpected 2\u001b[39m", "Received 3"] },
  { title: "later", fullName: "math later", ancestorTitles: ["math"], status: "pending", duration: null, failureMessages: [] },
  { title: "unmeasured", fullName: "math unmeasured", ancestorTitles: ["math"], status: "passed", failureMessages: [] },
] }] }))
process.exit(1)
"#,
    )
    .unwrap();
    let results = dir.path().join("out/test_results.json");
    std::fs::create_dir_all(results.parent().unwrap()).unwrap();

    let output = run(Command::new("node")
        .arg(&adapter)
        .args(["--once-target", "pkg/tests", "--once-runner-type", "vitest"])
        .arg("--once-runner")
        .arg(&runner)
        .args(["--once-runner-via-node", "true"])
        .arg("--once-native-results")
        .arg(dir.path().join("out/native.json"))
        .arg("--once-results")
        .arg(&results)
        .args(["--once-source", "tests/math.test.js"])
        .current_dir(dir.path()));

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report = read_report(&results);
    assert_eq!(
        only_attempt(&report, "adds"),
        &serde_json::json!({"status": "passed", "duration_ms": 13})
    );
    assert_eq!(
        only_attempt(&report, "divides"),
        &serde_json::json!({"status": "failed", "duration_ms": 3, "failure": {"message": "Expected 2\nReceived 3"}})
    );
    assert_eq!(
        only_attempt(&report, "later"),
        &serde_json::json!({"status": "skipped"})
    );
    assert_eq!(
        only_attempt(&report, "unmeasured"),
        &serde_json::json!({"status": "passed"})
    );
}

#[test]
fn pytest_adapter_reports_one_attempt_per_test_with_failure_text() {
    let dir = tempfile::TempDir::new().unwrap();
    let adapter = dir.path().join("adapter.py");
    std::fs::write(&adapter, runner_source("_python_pytest_adapter")).unwrap();
    let fake = dir.path().join("fake");
    std::fs::create_dir_all(&fake).unwrap();
    std::fs::write(
        fake.join("pytest.py"),
        r#"class Item:
    def __init__(self, nodeid):
        self.nodeid = nodeid
        self.name = nodeid.split("::")[-1]


class Session:
    items = [Item("tests/test_math.py::test_adds"), Item("tests/test_math.py::test_divides"), Item("tests/test_math.py::test_later")]


class Report:
    def __init__(self, nodeid, when, outcome, duration, text=""):
        self.nodeid = nodeid
        self.when = when
        self.passed = outcome == "passed"
        self.skipped = outcome == "skipped"
        self.duration = duration
        self.longreprtext = text


def main(args, plugins):
    plugin = plugins[0]
    plugin.pytest_collection_finish(Session())
    for when, duration in (("setup", 0.001), ("call", 0.0204), ("teardown", 0.0012)):
        plugin.pytest_runtest_logreport(Report("tests/test_math.py::test_adds", when, "passed", duration))
    plugin.pytest_runtest_logreport(Report("tests/test_math.py::test_divides", "setup", "passed", 0.001))
    plugin.pytest_runtest_logreport(Report("tests/test_math.py::test_divides", "call", "failed", 0.004, "assert 3 == 2"))
    plugin.pytest_runtest_logreport(Report("tests/test_math.py::test_divides", "teardown", "passed", 0.001))
    plugin.pytest_runtest_logreport(Report("tests/test_math.py::test_later", "setup", "skipped", 0.0, "skipped"))
    return 1
"#,
    )
    .unwrap();
    let results = dir.path().join("out/test_results.json");

    let output = run(Command::new("python3")
        .arg(&adapter)
        .arg("--once-results")
        .arg(&results)
        .arg("--once-native-results")
        .arg(dir.path().join("out/native.json"))
        .args([
            "--once-target",
            "pkg/tests",
            "--once-source",
            "tests/test_math.py",
        ])
        .env("PYTHONPATH", &fake)
        .current_dir(dir.path()));

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report = read_report(&results);
    assert_eq!(report["summary"]["total"], 3, "{report}");
    assert_eq!(
        only_attempt(&report, "test_adds"),
        &serde_json::json!({"status": "passed", "duration_ms": 22})
    );
    assert_eq!(
        only_attempt(&report, "test_divides"),
        &serde_json::json!({"status": "failed", "duration_ms": 6, "failure": {"message": "assert 3 == 2"}})
    );
    assert_eq!(
        only_attempt(&report, "test_later"),
        &serde_json::json!({"status": "skipped"})
    );
}

#[test]
fn rspec_adapter_reports_example_run_time_and_exception() {
    let dir = tempfile::TempDir::new().unwrap();
    let adapter = dir.path().join("adapter.rb");
    std::fs::write(&adapter, runner_source("_ruby_rspec_adapter")).unwrap();
    let runner = dir.path().join("fake-rspec");
    write_executable(
        &runner,
        r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--out" ]; then out="$2"; fi
  shift
done
cat > "$out" <<'JSON'
{"examples": [
  {"id": "./spec/math_spec.rb[1:1]", "full_description": "math adds", "status": "passed", "file_path": "./spec/math_spec.rb", "line_number": 2, "run_time": 0.0156},
  {"id": "./spec/math_spec.rb[1:2]", "full_description": "math divides", "status": "failed", "file_path": "./spec/math_spec.rb", "line_number": 5, "run_time": 0.002, "exception": {"class": "RSpec::Expectations::ExpectationNotMetError", "message": "expected: 2\n     got: 3"}},
  {"id": "./spec/math_spec.rb[1:3]", "full_description": "math later", "status": "pending", "file_path": "./spec/math_spec.rb", "line_number": 8, "run_time": 0.0}
]}
JSON
exit 1
"#,
    );
    let results = dir.path().join("out/test_results.json");
    std::fs::create_dir_all(results.parent().unwrap()).unwrap();

    let output = run(Command::new("ruby")
        .arg(&adapter)
        .arg("--once-results")
        .arg(&results)
        .arg("--once-native-results")
        .arg(dir.path().join("out/native.json"))
        .arg("--once-runner")
        .arg(&runner)
        .args([
            "--once-target",
            "pkg/specs",
            "--once-source",
            "spec/math_spec.rb",
        ])
        .current_dir(dir.path()));

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report = read_report(&results);
    assert_eq!(
        only_attempt(&report, "math adds"),
        &serde_json::json!({"status": "passed", "duration_ms": 16})
    );
    assert_eq!(
        only_attempt(&report, "math divides"),
        &serde_json::json!({"status": "failed", "duration_ms": 2, "failure": {"message": "RSpec::Expectations::ExpectationNotMetError: expected: 2\n     got: 3"}})
    );
    assert_eq!(
        only_attempt(&report, "math later"),
        &serde_json::json!({"status": "skipped"})
    );
}

#[test]
fn minitest_adapter_times_each_file_and_keeps_failing_output() {
    let dir = tempfile::TempDir::new().unwrap();
    let adapter = dir.path().join("adapter.rb");
    std::fs::write(&adapter, runner_source("_ruby_minitest_adapter")).unwrap();
    std::fs::create_dir_all(dir.path().join("test")).unwrap();
    std::fs::write(dir.path().join("test/pass_test.rb"), "sleep 0.05\n").unwrap();
    std::fs::write(
        dir.path().join("test/fail_test.rb"),
        "warn \"Expected 2, got 3\"\nexit 1\n",
    )
    .unwrap();
    let results = dir.path().join("out/test_results.json");
    std::fs::create_dir_all(results.parent().unwrap()).unwrap();

    let output = run(Command::new("ruby")
        .arg(&adapter)
        .arg("--once-results")
        .arg(&results)
        .arg("--once-native-results")
        .arg(dir.path().join("out/native.txt"))
        .args(["--once-target", "pkg/tests"])
        .args(["--once-source", "test/pass_test.rb"])
        .args(["--once-source", "test/fail_test.rb"])
        .current_dir(dir.path()));

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report = read_report(&results);
    let passed = only_attempt(&report, "test/pass_test.rb");
    assert_eq!(passed["status"], "passed");
    assert!(passed["duration_ms"].as_u64().unwrap() >= 40, "{report}");
    assert!(passed.get("failure").is_none(), "{report}");
    let failed = only_attempt(&report, "test/fail_test.rb");
    assert_eq!(failed["status"], "failed");
    assert!(failed["duration_ms"].is_u64(), "{report}");
    assert_eq!(failed["failure"]["message"], "Expected 2, got 3");
}

#[test]
fn go_runner_reports_test2json_elapsed_and_output() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("runner.go"),
        runner_source("_go_test_runner_source"),
    )
    .unwrap();
    let runner = dir.path().join("runner");
    let build = run(Command::new("go")
        .args(["build", "-o"])
        .arg(&runner)
        .arg("runner.go")
        .env("GO111MODULE", "off")
        .current_dir(dir.path()));
    assert!(build.status.success(), "{build:?}");

    let binary = dir.path().join("pkg.test");
    write_executable(
        &binary,
        "#!/bin/sh\nprintf 'TestAdds\\nTestDivides\\nTestLater\\nTestNotReached\\n'\n",
    );
    let go = dir.path().join("fake-go");
    write_executable(
        &go,
        r#"#!/bin/sh
cat <<'JSON'
{"Action":"run","Test":"TestAdds"}
{"Action":"output","Test":"TestAdds","Output":"=== RUN   TestAdds\n"}
{"Action":"output","Test":"TestAdds","Output":"--- PASS: TestAdds (0.02s)\n"}
{"Action":"pass","Test":"TestAdds","Elapsed":0.021}
{"Action":"run","Test":"TestDivides"}
{"Action":"output","Test":"TestDivides","Output":"=== RUN   TestDivides\n"}
{"Action":"output","Test":"TestDivides","Output":"    math_test.go:9: expected 2, got 3\n"}
{"Action":"output","Test":"TestDivides","Output":"--- FAIL: TestDivides (0.00s)\n"}
{"Action":"fail","Test":"TestDivides","Elapsed":0.004}
{"Action":"output","Test":"TestLater","Output":"--- SKIP: TestLater (0.00s)\n"}
{"Action":"skip","Test":"TestLater","Elapsed":0}
{"Action":"output","Output":"FAIL\n"}
{"Action":"fail","Elapsed":0.03}
JSON
exit 1
"#,
    );
    let results = dir.path().join("out/test_results.json");

    let output = run(Command::new(&runner)
        .arg(&go)
        .arg("example.com/pkg")
        .arg(&binary)
        .arg(&results)
        .arg(dir.path().join("out/go-test.log"))
        .arg(dir.path().join("out/native.jsonl"))
        .arg("")
        .arg("pkg/tests")
        .arg("--")
        .current_dir(dir.path()));

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report = read_report(&results);
    assert_eq!(
        only_attempt(&report, "TestAdds"),
        &serde_json::json!({"status": "passed", "duration_ms": 21})
    );
    assert_eq!(
        only_attempt(&report, "TestDivides"),
        &serde_json::json!({"status": "failed", "duration_ms": 4, "failure": {"message": "    math_test.go:9: expected 2, got 3\n--- FAIL: TestDivides (0.00s)"}})
    );
    assert_eq!(
        only_attempt(&report, "TestLater"),
        &serde_json::json!({"status": "skipped"})
    );
}

#[test]
fn jvm_runner_times_each_test_method_and_reports_its_failure() {
    let dir = tempfile::TempDir::new().unwrap();
    let runner_dir = dir.path().join("runner");
    let classes = dir.path().join("classes");
    std::fs::create_dir_all(&runner_dir).unwrap();
    std::fs::create_dir_all(&classes).unwrap();
    std::fs::write(
        runner_dir.join("OnceJvmTestRunner.java"),
        runner_source("_jvm_test_runner_source"),
    )
    .unwrap();
    std::fs::write(
        classes.join("MathTest.java"),
        "public class MathTest {\n  public void testAdds() throws Exception { Thread.sleep(30); }\n  public void testDivides() { throw new AssertionError(\"expected 2, got 3\"); }\n}\n",
    )
    .unwrap();
    for (source, out) in [
        (runner_dir.join("OnceJvmTestRunner.java"), &runner_dir),
        (classes.join("MathTest.java"), &classes),
    ] {
        let compile = run(Command::new("javac").arg("-d").arg(out).arg(source));
        assert!(compile.status.success(), "{compile:?}");
    }
    let results = dir.path().join("out/test_results.json");
    let classpath = std::env::join_paths([&runner_dir, &classes]).unwrap();

    let output = run(Command::new("java")
        .arg("-cp")
        .arg(classpath)
        .arg("OnceJvmTestRunner")
        .arg(&classes)
        .arg(&results)
        .arg(dir.path().join("out/jvm.log"))
        .arg(dir.path().join("out/native.txt"))
        .args(["pkg/tests", "kotlin_jvm_test"]));

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report = read_report(&results);
    let adds = only_attempt(&report, "testAdds");
    assert_eq!(adds["status"], "passed");
    assert!(adds["duration_ms"].as_u64().unwrap() >= 25, "{report}");
    assert!(adds.get("failure").is_none(), "{report}");
    let divides = only_attempt(&report, "testDivides");
    assert_eq!(divides["status"], "failed");
    assert!(divides["duration_ms"].is_u64(), "{report}");
    assert!(divides["failure"]["message"]
        .as_str()
        .unwrap()
        .starts_with("java.lang.AssertionError: expected 2, got 3"));
}
