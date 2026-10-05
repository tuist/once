//! `shellspec_test` reads `ShellSpec`'s `JUnit` report so each example reports
//! its own outcome and failure, while keeping the example identities scanned
//! from the spec sources.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

use once_frontend::analysis::with_active_store;

use super::{action_by_identifier, all_prelude_source, eval_prelude_source_to_repr, store_for};

const MATH_SPEC: &str = r#"Describe 'math'
  It 'adds & "quotes"'
    When call expr 1 + 1
    The output should eq 2
  End

  It 'fails <here>'
    When call expr 1 + 1
    The output should eq 3
  End

  It 'is skipped'
    Skip "not now"
    When call true
  End

  It "uses double quotes"
    When call true
    The status should be success
  End
End
"#;

fn shellspec_path() -> String {
    let output = Command::new("sh")
        .args(["-c", "command -v shellspec"])
        .output()
        .unwrap();
    let path = String::from_utf8(output.stdout).unwrap().trim().to_string();
    assert!(!path.is_empty(), "`shellspec` must be on PATH");
    path
}

fn shellspec_script(workspace: &Path, args: &str) -> String {
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{"package": "", "name": "specs", "id": "specs"}},
    "attr": {{"shellspec": {shellspec:?}, "args": {args}}},
    "deps": [],
    "srcs": ["spec/**/*_spec.sh"],
    "build_dir": ".once/out/specs",
    "capability": "test",
}}
_shellspec_test_impl(ctx)
result = "ok"
"#,
        prelude = all_prelude_source(),
        shellspec = shellspec_path(),
    );
    let store = store_for(workspace, "");
    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    out.unwrap();
    action_by_identifier(&store, "shellspec_test:specs").argv[2].clone()
}

fn run(workspace: &Path, args: &str) -> (std::process::Output, serde_json::Value) {
    let script = shellspec_script(workspace, args);
    let output = Command::new("sh")
        .arg("-c")
        .arg(&script)
        .env("HOME", workspace.join(".once/out/specs/test/home"))
        .current_dir(workspace)
        .output()
        .unwrap();
    let results = std::fs::read_to_string(workspace.join(".once/out/specs/test/test_results.json"))
        .unwrap_or_else(|error| panic!("missing results: {error}\n{output:?}\n{script}"));
    (output, serde_json::from_str(&results).unwrap())
}

fn workspace() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("spec")).unwrap();
    std::fs::create_dir_all(dir.path().join(".once/out/specs/test")).unwrap();
    std::fs::write(dir.path().join(".shellspec"), "").unwrap();
    std::fs::write(dir.path().join("spec/math_spec.sh"), MATH_SPEC).unwrap();
    dir
}

fn case<'a>(report: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    report["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == id)
        .unwrap_or_else(|| panic!("missing case {id}: {report}"))
}

#[test]
fn shellspec_examples_report_their_own_outcomes_and_failures() {
    let dir = workspace();

    let (output, report) = run(dir.path(), "[]");

    assert!(!output.status.success(), "{output:?}");
    assert_eq!(report["status"], "failed");
    assert_eq!(
        report["summary"],
        serde_json::json!({"total": 4, "passed": 2, "failed": 1, "skipped": 1, "flaky": 0}),
        "{report}"
    );
    assert_eq!(
        case(&report, "spec/math_spec.sh::adds & \"quotes\"")["attempts"],
        serde_json::json!([{"status": "passed"}]),
        "{report}"
    );
    let failed = &case(&report, "spec/math_spec.sh::fails <here>")["attempts"][0];
    assert_eq!(failed["status"], "failed");
    assert!(failed.get("duration_ms").is_none(), "{report}");
    let message = failed["failure"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("The output should eq 3\nexpected: 3"),
        "{message}"
    );
    assert!(message.contains("got: 2"), "{message}");
    assert_eq!(
        case(&report, "spec/math_spec.sh::is skipped")["status"],
        "skipped"
    );
    assert_eq!(
        case(&report, "spec/math_spec.sh::math uses double quotes")["status"],
        "passed",
        "report entries the source scan misses are still reported: {report}"
    );
    let log =
        std::fs::read_to_string(dir.path().join(".once/out/specs/test/shellspec.log")).unwrap();
    assert!(log.contains("fails <here>"), "{log}");
}

/// `JUnit` `time` is measured only under the profiler.
#[test]
fn shellspec_examples_report_durations_under_the_profiler() {
    let dir = workspace();

    let (_, report) = run(dir.path(), r#"["--profile"]"#);

    let passed = &case(&report, "spec/math_spec.sh::adds & \"quotes\"")["attempts"][0];
    assert!(passed["duration_ms"].is_u64(), "{report}");
    assert!(
        case(&report, "spec/math_spec.sh::is skipped")["attempts"][0]
            .get("duration_ms")
            .is_none()
    );
}

/// A project that chooses its own reports keeps them, and the results fall
/// back to the scanned examples with the run's verdict.
#[test]
fn shellspec_keeps_project_report_options() {
    let dir = workspace();
    std::fs::write(
        dir.path().join(".shellspec"),
        "--reportdir custom-reports\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("spec/math_spec.sh"),
        "Describe 'math'\n  It 'adds'\n    When call expr 1 + 1\n    The output should eq 2\n  End\nEnd\n",
    )
    .unwrap();

    let (output, report) = run(dir.path(), "[]");

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        case(&report, "spec/math_spec.sh::adds")["attempts"],
        serde_json::json!([{"status": "passed"}])
    );
    assert!(!dir.path().join(".once/out/specs/test/junit").exists());
}

/// A target whose own arguments request a `JUnit` report has that report read,
/// at the location the arguments chose.
#[test]
fn shellspec_reads_the_report_its_arguments_request() {
    let dir = workspace();

    let (_, report) = run(
        dir.path(),
        r#"["--output", "junit", "--reportdir", "my-reports"]"#,
    );

    assert!(dir.path().join("my-reports/results_junit.xml").exists());
    assert!(!dir.path().join(".once/out/specs/test/junit").exists());
    assert_eq!(
        case(&report, "spec/math_spec.sh::fails <here>")["status"],
        "failed",
        "{report}"
    );
    assert_eq!(report["summary"]["skipped"], 1, "{report}");
}

/// Output an example prints lands in the report as CDATA and is never read
/// as report structure, so a passing example that prints report-shaped text
/// stays passing.
#[test]
fn shellspec_ignores_report_shaped_example_output() {
    let dir = workspace();
    std::fs::write(
        dir.path().join("spec/math_spec.sh"),
        "Describe 'math'\n  It 'prints markup'\n    When call printf '%s\\n' 'first' '<failure message=\"oops\"/>' '<skipped/>'\n    The status should be success\n    The output should include 'first'\n  End\nEnd\n",
    )
    .unwrap();

    let (output, report) = run(dir.path(), "[]");

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        case(&report, "spec/math_spec.sh::prints markup")["attempts"],
        serde_json::json!([{"status": "passed"}]),
        "{report}"
    );
}

/// Examples run in a shuffled order still each take their own report entry.
#[test]
fn shellspec_matches_report_entries_in_random_order() {
    let dir = workspace();

    let (_, report) = run(dir.path(), r#"["--random", "examples:7"]"#);

    assert_eq!(report["summary"]["total"], 4, "{report}");
    assert_eq!(
        case(&report, "spec/math_spec.sh::fails <here>")["status"],
        "failed",
        "{report}"
    );
    assert_eq!(
        case(&report, "spec/math_spec.sh::is skipped")["status"],
        "skipped",
        "{report}"
    );
}

/// The same example name under two groups cannot be matched to its source
/// line by name alone, so those report entries are reported under their full
/// names instead of being swapped.
#[test]
fn shellspec_reports_repeated_example_names_by_full_name() {
    let dir = workspace();
    std::fs::write(
        dir.path().join("spec/math_spec.sh"),
        "Describe 'first'\n  It 'works'\n    When call true\n    The status should be success\n  End\nEnd\nDescribe 'second'\n  It 'works'\n    When call false\n    The status should be success\n  End\nEnd\n",
    )
    .unwrap();

    let (_, report) = run(dir.path(), r#"["--random", "examples:3"]"#);

    assert_eq!(report["summary"]["total"], 2, "{report}");
    assert_eq!(
        case(&report, "spec/math_spec.sh::first works")["status"],
        "passed",
        "{report}"
    );
    assert_eq!(
        case(&report, "spec/math_spec.sh::second works")["status"],
        "failed",
        "{report}"
    );
}

/// An example whose name ends another example's name never takes that
/// example's report entry, whatever order the examples ran in.
#[test]
fn shellspec_matches_the_longest_example_name() {
    let dir = workspace();
    std::fs::write(
        dir.path().join("spec/math_spec.sh"),
        "Describe 'math'\n  It 'works'\n    When call true\n    The status should be success\n  End\n  It 'nested works'\n    When call false\n    The status should be success\n  End\nEnd\n",
    )
    .unwrap();

    for seed in 1..6 {
        let (_, report) = run(dir.path(), &format!(r#"["--random", "examples:{seed}"]"#));

        assert_eq!(report["summary"]["total"], 2, "{report}");
        assert_eq!(
            case(&report, "spec/math_spec.sh::works")["status"],
            "passed",
            "{report}"
        );
        assert_eq!(
            case(&report, "spec/math_spec.sh::nested works")["status"],
            "failed",
            "{report}"
        );
    }
}
