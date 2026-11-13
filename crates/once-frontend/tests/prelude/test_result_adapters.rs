//! Each language runner normalizes its native report into
//! `once.test_results.v1`. These tests drive the embedded runners against
//! canned native output and check that every attempt carries the duration
//! the native runner measured and the failure message it recorded, so the
//! run events report them instead of zero durations and empty failures.
#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output};

use super::{all_prelude_source, eval_prelude_string_function_in, write_executable};

#[test]
fn dynamic_runner_home_resolves_at_execution_and_preserves_overrides() {
    let prelude = all_prelude_source();
    for kind in [
        "pytest_test",
        "rspec_test",
        "minitest_test",
        "vitest_test",
        "jest_test",
    ] {
        for override_home in [false, true] {
            let source = format!(
                r#"{prelude}
def _resolve_host_executable(value):
    return value
def host_command(argv, cwd = None, merge_stderr = False):
    return "1.0"
ctx = {{
    "label": {{"id": "pkg/tests", "name": "tests", "package": "pkg"}},
    "attr": {{"python": "python", "env": {env}}},
    "srcs": [],
    "build_dir": ".once/out/pkg/tests",
    "capability": "query",
    "test": {{"batch_id": "batch-a"}},
}}
result = _json_encode({kind}["impl"](ctx)["test_info"]["command"]["env"])
"#,
                env = if override_home {
                    r#"{"HOME": "/custom/home"}"#
                } else {
                    "{}"
                }
            );
            let env: std::collections::BTreeMap<String, String> =
                serde_json::from_str(&super::eval_prelude_source_to_repr(source).unwrap()).unwrap();
            for root in ["/first-checkout", "/second-checkout"] {
                let resolved = once_core::resolve_execution_env(&env, Path::new(root));
                let expected = if override_home {
                    "/custom/home".to_owned()
                } else {
                    format!("{root}/.once/out/pkg/tests/test/batches/batch-a/home")
                };
                assert_eq!(resolved["HOME"], expected, "{kind}");
            }
        }
    }
}

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
fn shellspec_reports_escape_strings_and_preserve_duplicate_case_identity() {
    let dir = tempfile::TempDir::new().unwrap();
    let spec = "spec/quoted\"\\\t_spec.sh";
    std::fs::create_dir(dir.path().join("spec")).unwrap();
    let name = "does not matches contains \".dot\" file\\path\t\r\u{0001}";
    std::fs::write(
        dir.path().join(spec),
        format!("Describe 'files'\n  It '{name}'\n  It '{name}'\nEnd\n"),
    )
    .unwrap();
    write_executable(
        &dir.path().join("shellspec"),
        "#!/bin/sh\nexit \"${SHELLSPEC_EXIT:-0}\"\n",
    );
    let target = "tests\"\\\t\n";
    let build_dir = "out\"\\\t\n";
    let source = format!(
        r#"{}
def host_which(name):
    return "/bin/sh" if name == "sh" else "/usr/bin/" + name
ctx = {{
    "label": {{"id": {target}, "name": "tests", "package": ""}},
    "attr": {{"shellspec": "./shellspec"}},
    "srcs": ["spec/**/*_spec.sh"],
    "build_dir": {build_dir},
    "capability": "test",
}}
result = repr(_shellspec_test_impl(ctx))
"#,
        all_prelude_source(),
        target = serde_json::to_string(target).unwrap(),
        build_dir = serde_json::to_string(build_dir).unwrap(),
    );
    let mut reports = Vec::new();
    for exit in [0, 0, 7] {
        let store =
            super::AnalysisStore::new(dir.path().to_path_buf(), String::new(), "out".into());
        let (store, result) =
            super::with_active_store(store, || super::eval_prelude_source_to_repr(source.clone()));
        result.unwrap();
        let action = super::action_by_identifier(&store, &format!("shellspec_test:{target}"));
        std::fs::create_dir_all(dir.path().join(build_dir).join("test")).unwrap();
        let output = run(Command::new(&action.argv[0])
            .args(&action.argv[1..])
            .env("HOME", dir.path().join(build_dir).join("test/home"))
            .env("SHELLSPEC_EXIT", exit.to_string())
            .current_dir(dir.path()));
        assert_eq!(output.status.code(), Some(exit), "{output:?}");
        reports.push(read_report(
            &dir.path().join(build_dir).join("test/test_results.json"),
        ));
    }
    assert_eq!(reports[0], reports[1]);
    assert_eq!(reports[0]["target"], target);
    assert_eq!(
        reports[0]["artifacts"]["logs"][0],
        format!("{build_dir}/test/shellspec.log")
    );
    assert_eq!(
        reports[0]["artifacts"]["native_results"][0],
        format!("{build_dir}/test/native_results.txt")
    );
    assert_eq!(reports[2]["status"], "failed");
    let cases = reports[0]["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 2);
    for (index, case) in cases.iter().enumerate() {
        assert_eq!(case["name"], name);
        assert_eq!(case["file"], spec);
        assert_eq!(case["suite"], "quoted\"\\\t");
        assert_eq!(case["status"], "passed");
        assert_eq!(case["id"], format!("{spec}:{}", index + 2));
        assert_eq!(reports[2]["cases"][index]["id"], case["id"]);
        assert_eq!(reports[2]["cases"][index]["status"], "unknown");
    }
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
fn javascript_suite_units_preserve_duplicate_assertions_and_propagate_failures() {
    let dir = tempfile::TempDir::new().unwrap();
    let adapter = dir.path().join("adapter.mjs");
    std::fs::write(&adapter, runner_source("_javascript_test_adapter")).unwrap();
    let runner = dir.path().join("runner.mjs");
    std::fs::write(&runner, r"import fs from 'node:fs'
const output = process.argv.find(arg => arg.startsWith('--outputFile=')).slice('--outputFile='.length)
const failed = process.env.FAIL === 'true'
fs.writeFileSync(output, JSON.stringify({success: !failed, testResults: [{name: 'duplicate.test.js', assertionResults: [
  {title: 'same', fullName: 'same', status: 'passed'},
  {title: 'same', fullName: 'same', status: failed ? 'failed' : 'passed', failureMessages: failed ? ['intentional failure'] : []},
  {title: 'later', fullName: 'later', status: 'pending'}
]}]}))
fs.writeSync(1, 'x'.repeat(1100000))
process.exit(failed ? 1 : 0)
").unwrap();
    let results = dir.path().join("results.json");
    let native = dir.path().join("native.json");
    for runner_type in ["vitest", "jest"] {
        for unit in ["case", "suite"] {
            for failed in [false, true] {
                let output = run(Command::new("node")
                    .arg(&adapter)
                    .args([
                        "--once-target",
                        "pkg/tests",
                        "--once-runner-type",
                        runner_type,
                    ])
                    .args(if unit == "suite" {
                        &["--once-unit", "suite"][..]
                    } else {
                        &[]
                    })
                    .args(["--once-runner-via-node", "true"])
                    .arg("--once-runner")
                    .arg(&runner)
                    .arg("--once-native-results")
                    .arg(&native)
                    .arg("--once-results")
                    .arg(&results)
                    .env("FAIL", failed.to_string())
                    .current_dir(dir.path()));
                assert_eq!(output.status.success(), !failed);
                assert_eq!(output.stdout.len(), 1_100_000);
                let report = read_report(&results);
                let native_report = read_report(&native);
                let assertions = native_report["testResults"][0]["assertionResults"]
                    .as_array()
                    .unwrap();
                assert_eq!(assertions.len(), 3);
                assert_eq!(assertions[0]["fullName"], assertions[1]["fullName"]);
                assert_eq!(
                    assertions[1]["status"],
                    if failed { "failed" } else { "passed" }
                );
                once_core::validate_test_results(&report, "pkg/tests").unwrap();
                assert_eq!(report["selection_supported"], false);
                assert_eq!(report["summary"]["total"], 1);
                assert_eq!(report["cases"][0]["id"], "pkg/tests::suite");
                assert_eq!(
                    report["cases"][0]["status"],
                    if failed { "failed" } else { "passed" }
                );
                assert_eq!(
                    report["runner"]["metadata"]["native_summary"],
                    serde_json::json!({"total":3,"passed":2-u8::from(failed),"failed":u8::from(failed),"skipped":1})
                );
            }
        }
        let output = run(Command::new("node")
            .arg(&adapter)
            .args([
                "--once-target",
                "pkg/tests",
                "--once-runner-type",
                runner_type,
            ])
            .args(["--once-runner-via-node", "true"])
            .arg("--once-runner")
            .arg(&runner)
            .arg("--once-native-results")
            .arg(&native)
            .arg("--once-results")
            .arg(&results)
            .args([
                "--once-test-unit",
                "pkg/tests::%5B%22duplicate.test.js%22%2C%22same%22%5D",
            ])
            .current_dir(dir.path()));
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains("selected full test names are ambiguous; run the whole target"));
    }
}

#[test]
fn javascript_suite_units_declare_unsupported_case_selection() {
    let dir = tempfile::TempDir::new().unwrap();
    let adapter = dir.path().join("adapter.mjs");
    std::fs::write(&adapter, runner_source("_javascript_test_adapter")).unwrap();
    let output = run(Command::new("node").arg(&adapter).args([
        "--once-unit",
        "suite",
        "--once-test-unit",
        "pkg/tests::suite",
    ]));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("suite units do not support test-unit filtering"));
    for runner_type in ["vitest", "jest"] {
        let source = format!(
            r#"{}
def _resolve_host_executable(value):
    return value
ctx = {{"label": {{"id": "pkg/tests", "name": "tests", "package": "pkg"}}, "attr": {{"unit": "suite"}}, "srcs": [], "build_dir": ".once/out/pkg/tests", "capability": "query"}}
result = _json_encode({runner_type}_test["impl"](ctx)["test_info"])
"#,
            all_prelude_source()
        );
        let info: serde_json::Value =
            serde_json::from_str(&super::eval_prelude_source_to_repr(source).unwrap()).unwrap();
        assert_eq!(info["listing"]["supported"], false);
        assert_eq!(info["filtering"]["case_filtering"], "unsupported");
        assert_eq!(
            info["sharding"],
            serde_json::json!({"supported":false,"granularity":"target"})
        );
    }
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


def hookimpl(**kwargs):
    return lambda function: function


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
fn pytest_adapter_preserves_native_ignores_source_boundaries_and_exact_selection() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("adapter.py"),
        runner_source("_python_pytest_adapter"),
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("tests")).unwrap();
    std::fs::create_dir(dir.path().join("tests/ignored_dir")).unwrap();
    std::fs::write(
        dir.path().join("tests/ignored_dir/test_nested.py"),
        "raise RuntimeError('ignored directory imported')\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("tests/conftest.py"),
        "collect_ignore = ['test_ignored.py', 'ignored_dir']\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("tests/test_ignored.py"),
        "raise RuntimeError('ignored module imported')\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("tests/test_outside.py"),
        "raise RuntimeError('outside target imported')\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("tests/test_kept.py"), "def test_pass():\n    assert True\ndef test_fail():\n    assert False, 'intentional failure'\n").unwrap();
    std::fs::write(dir.path().join("check_collection.py"), r"import json
import subprocess
import sys

def test_collection():
    native = subprocess.run([sys.executable, '-m', 'pytest', '-q', 'tests', '--ignore=tests/test_outside.py'], capture_output=True, text=True)
    assert native.returncode == 1, native.stdout + native.stderr
    assert '1 failed, 1 passed' in native.stdout
    explicit = subprocess.run([sys.executable, '-m', 'pytest', '-q', 'tests/test_ignored.py'], capture_output=True, text=True)
    assert explicit.returncode == 2, explicit.stdout + explicit.stderr
    assert 'ignored module imported' in explicit.stdout
    command = [sys.executable, 'adapter.py', '--once-target', 'tests', '--once-source', 'tests/test_ignored.py', '--once-source', 'tests/ignored_dir/test_nested.py', '--once-source', 'tests/test_kept.py', '--once-results', 'out/results.json', '--once-native-results', 'out/native.json']
    for selected, expected_code, expected_names in [
        (None, 1, ['test_fail', 'test_pass']),
        ('tests/test_kept.py::test_pass', 0, ['test_pass']),
        ('tests/test_kept.py::test_fail', 1, ['test_fail']),
        ('tests/test_ignored.py::test_any', 4, []),
    ]:
        args = command + (['--once-test-unit', 'tests::' + selected] if selected else []) + ['--', '-q']
        result = subprocess.run(args, capture_output=True, text=True)
        assert result.returncode == expected_code, result.stdout + result.stderr
        report = json.load(open('out/results.json'))
        assert sorted(case['name'] for case in report['cases']) == expected_names, report
        assert 'ignored module imported' not in result.stdout
        assert 'ignored directory imported' not in result.stdout
        assert 'outside target imported' not in result.stdout

    for source in ['tests/test_ignored.py', 'tests/ignored_dir/test_nested.py']:
        ignored = [sys.executable, 'adapter.py', '--once-target', 'tests', '--once-source', source, '--once-results', 'out/ignored.json', '--once-native-results', 'out/ignored-native.json', '--', '-q']
        result = subprocess.run(ignored, capture_output=True, text=True)
        assert result.returncode == 5, result.stdout + result.stderr
        assert json.load(open('out/ignored.json'))['cases'] == []

    with open('tests/conftest.py', 'a') as file:
        file.write('\ndef pytest_ignore_collect(collection_path):\n    if collection_path.name == \'test_outside.py\':\n        return False\n')
    native = subprocess.run([sys.executable, '-m', 'pytest', '-q', 'tests/test_kept.py::test_pass'], capture_output=True, text=True)
    assert native.returncode == 0, native.stdout + native.stderr
    result = subprocess.run(command + ['--once-test-unit', 'tests::tests/test_kept.py::test_pass', '--', '-q'], capture_output=True, text=True)
    assert result.returncode == 0, result.stdout + result.stderr
    assert [case['name'] for case in json.load(open('out/results.json'))['cases']] == ['test_pass']
    assert 'outside target imported' not in result.stdout

def test_package_configuration():
    from pathlib import Path
    Path('package/src').mkdir(parents=True)
    Path('package/tests').mkdir()
    Path('package/pytest.ini').write_text('[pytest]\npythonpath = src\n')
    Path('package/src/owned_module.py').write_text('VALUE = 42\n')
    Path('package/tests/test_config.py').write_text('from owned_module import VALUE\ndef test_import():\n    assert VALUE == 42\n')
    native = subprocess.run([sys.executable, '-m', 'pytest', '-q', 'package/tests/test_config.py'], capture_output=True, text=True)
    assert native.returncode == 0, native.stdout + native.stderr
    command = [sys.executable, 'adapter.py', '--once-target', 'tests', '--once-source', 'package/tests/test_config.py', '--once-results', 'out/package.json', '--once-native-results', 'out/package-native.json']
    for selection in [[], ['--once-test-unit', 'tests::tests/test_config.py::test_import']]:
        result = subprocess.run(command + selection + ['--', '-q'], capture_output=True, text=True)
        assert result.returncode == 0, result.stdout + result.stderr
        report = json.load(open('out/package.json'))
        assert [case['name'] for case in report['cases']] == ['test_import']
").unwrap();
    let output = run(Command::new("pytest")
        .args(["-q", "check_collection.py"])
        .env("PYTEST_DISABLE_PLUGIN_AUTOLOAD", "1")
        .current_dir(dir.path()));
    assert!(output.status.success(), "{output:?}");
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
fn rspec_reporting_preserves_application_cli_format_detection() {
    let dir = tempfile::TempDir::new().unwrap();
    let adapter = dir.path().join("adapter.rb");
    std::fs::write(&adapter, runner_source("_ruby_rspec_adapter")).unwrap();
    std::fs::write(
        dir.path().join("cli_spec.rb"),
        r"RSpec.describe 'application CLI' do
  it 'keeps text output' do
    index = ARGV.index('--format')
    json_output = index && ['json', 'j'].include?(ARGV[index + 1])
    expect(json_output).to be_falsey
  end
  it 'propagates failures' do
    expect(ENV['INTENTIONAL_FAILURE']).not_to eq('true')
  end
end
",
    )
    .unwrap();
    let results = dir.path().join("results.json");
    for failed in [false, true] {
        let output = run(Command::new("ruby")
            .arg(&adapter)
            .arg("--once-results")
            .arg(&results)
            .arg("--once-native-results")
            .arg(dir.path().join("native.json"))
            .args([
                "--once-runner",
                "rspec",
                "--once-target",
                "tests",
                "--once-source",
                "cli_spec.rb",
            ])
            .env("INTENTIONAL_FAILURE", failed.to_string())
            .current_dir(dir.path()));
        assert_eq!(output.status.success(), !failed, "{output:?}");
        let report = read_report(&results);
        assert_eq!(report["summary"]["total"], 2);
        assert_eq!(
            only_attempt(&report, "application CLI keeps text output")["status"],
            "passed"
        );
        assert_eq!(report["summary"]["failed"], u8::from(failed));
    }
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
