use std::path::{Path, PathBuf};
use std::process::Command;

use super::{all_prelude_source, eval_prelude_string_function_in};

const SAMPLE_TESTS: &str = r#"
#[cfg(test)]
mod tests {
    #[test]
    fn passes_slowly() {
        std::thread::sleep(std::time::Duration::from_millis(40));
    }

    #[test]
    fn fails_with_output() {
        println!("captured line");
        println!("---- tests::passes_slowly stdout ----");
        println!("failures:");
        assert_eq!(1, 2, "numbers differ");
    }

    #[test]
    #[should_panic]
    fn panics_as_expected() {
        panic!("boom");
    }

    #[test]
    #[should_panic]
    fn never_panics() {}

    #[test]
    #[ignore = "slow ... later"]
    fn ignored_case() {}
}
"#;

fn compile_runner(dir: &Path) -> PathBuf {
    let source =
        eval_prelude_string_function_in(all_prelude_source(), "_rust_test_runner_source", "()")
            .unwrap();
    let source_path = dir.join("runner.rs");
    std::fs::write(&source_path, source).unwrap();
    let runner = dir.join(format!("runner{}", std::env::consts::EXE_SUFFIX));
    let output = Command::new("rustc")
        .args(["--edition", "2021"])
        .arg(&source_path)
        .arg("-o")
        .arg(&runner)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    runner
}

fn compile_sample_tests(dir: &Path) -> PathBuf {
    compile_tests(dir, SAMPLE_TESTS)
}

fn compile_tests(dir: &Path, source: &str) -> PathBuf {
    let source_path = dir.join("sample.rs");
    std::fs::write(&source_path, source).unwrap();
    let binary = dir.join(format!("sample{}", std::env::consts::EXE_SUFFIX));
    let output = Command::new("rustc")
        .args(["--edition", "2021", "--test"])
        .arg(&source_path)
        .arg("-o")
        .arg(&binary)
        .env_remove("RUSTC_BOOTSTRAP")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    binary
}

fn run_runner(
    runner: &Path,
    binary: &Path,
    dir: &Path,
    mode: &str,
    configure: impl FnOnce(&mut Command),
) -> (std::process::Output, serde_json::Value) {
    let results = dir.join(format!("{mode}/test_results.json"));
    let mut command = Command::new(runner);
    command
        .arg(binary)
        .arg(&results)
        .arg(dir.join(format!("{mode}/rust-libtest.log")))
        .arg(dir.join(format!("{mode}/native_results.txt")))
        .arg("pkg/tests")
        .arg(dir)
        .arg(mode)
        .env_remove("RUSTC_BOOTSTRAP");
    configure(&mut command);
    let output = command.output().unwrap();
    let report = serde_json::from_slice(&std::fs::read(&results).unwrap()).unwrap();
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

fn attempt<'a>(report: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    let attempts = case(report, name)["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1, "{report}");
    &attempts[0]
}

/// The stable harness prints no per-test timing, so the runner opts into
/// libtest's timing report. Every case that ran carries its measured
/// duration, every failure carries the output libtest captured for that case
/// only, and ignored cases report neither.
#[test]
fn libtest_runner_reports_durations_and_failure_output() {
    let dir = tempfile::TempDir::new().unwrap();
    let runner = compile_runner(dir.path());
    let binary = compile_sample_tests(dir.path());

    let (output, report) = run_runner(&runner, &binary, dir.path(), "report-time", |_| {});

    assert_eq!(output.status.code(), Some(101), "{output:?}");
    assert_eq!(report["status"], "failed");
    assert_eq!(
        report["summary"],
        serde_json::json!({"total": 5, "passed": 2, "failed": 2, "skipped": 1, "flaky": 0})
    );

    let slow = attempt(&report, "tests::passes_slowly");
    assert_eq!(slow["status"], "passed");
    assert!(slow["duration_ms"].as_u64().unwrap() >= 30, "{report}");
    assert!(slow.get("failure").is_none(), "{report}");

    let panicked = attempt(&report, "tests::panics_as_expected");
    assert_eq!(panicked["status"], "passed");
    assert!(panicked["duration_ms"].is_u64(), "{report}");

    let failed = attempt(&report, "tests::fails_with_output");
    assert_eq!(failed["status"], "failed");
    assert!(failed["duration_ms"].is_u64(), "{report}");
    let message = failed["failure"]["message"].as_str().unwrap();
    assert!(message.starts_with("captured line\n"), "{message}");
    assert!(
        message.contains("---- tests::passes_slowly stdout ----\nfailures:\n"),
        "{message}"
    );
    assert!(message.contains("numbers differ"), "{message}");
    assert!(!message.contains("never_panics"), "{message}");
    assert!(!message.contains("test result:"), "{message}");

    let not_panicked = attempt(&report, "tests::never_panics");
    assert_eq!(not_panicked["status"], "failed");
    let message = not_panicked["failure"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("note: test did not panic as expected"),
        "{message}"
    );
    assert!(!message.contains("numbers differ"), "{message}");

    let ignored = attempt(&report, "tests::ignored_case");
    assert_eq!(ignored["status"], "skipped");
    assert!(ignored.get("duration_ms").is_none(), "{report}");
    assert!(ignored.get("failure").is_none(), "{report}");

    let native =
        std::fs::read_to_string(dir.path().join("report-time/native_results.txt")).unwrap();
    assert!(
        native.contains("-Z unstable-options --report-time"),
        "{native}"
    );
}

/// Captured output is printed after every result line, so a test that
/// prints result-shaped lines or another failure's header cannot change a
/// verdict, and ambiguous failure output stays in the log instead of being
/// attributed to the wrong case.
#[test]
fn libtest_runner_ignores_result_and_header_lines_printed_by_tests() {
    let dir = tempfile::TempDir::new().unwrap();
    let runner = compile_runner(dir.path());
    let binary = compile_tests(
        dir.path(),
        r#"
#[cfg(test)]
mod tests {
    #[test]
    fn works() {}

    #[test]
    fn first_failure() {
        println!("test tests::works ... FAILED");
        println!("---- tests::second_failure stdout ----");
        panic!("first");
    }

    #[test]
    fn second_failure() {
        panic!("second");
    }
}
"#,
    );

    let (output, report) = run_runner(&runner, &binary, dir.path(), "report-time", |_| {});

    assert_eq!(output.status.code(), Some(101), "{output:?}");
    assert_eq!(attempt(&report, "tests::works")["status"], "passed");
    assert_eq!(report["summary"]["failed"], 2, "{report}");
    for name in ["tests::first_failure", "tests::second_failure"] {
        let failed = attempt(&report, name);
        assert_eq!(failed["status"], "failed");
        assert!(failed.get("failure").is_none(), "{report}");
    }
}

/// Under `--nocapture` a test's output reaches the run log while results are
/// still being printed. A bare section header printed by a test must not end
/// result parsing early.
#[test]
fn libtest_runner_keeps_reading_results_after_uncaptured_section_headers() {
    let dir = tempfile::TempDir::new().unwrap();
    let runner = compile_runner(dir.path());
    let binary = compile_tests(
        dir.path(),
        r#"
#[cfg(test)]
mod tests {
    #[test]
    fn a_prints_a_header() {
        println!();
        println!("failures:");
    }

    #[test]
    #[ignore]
    fn b_ignored() {}

    #[test]
    fn c_works() {
        eprintln!("test tests::a_prints_a_header ... FAILED");
    }
}
"#,
    );

    let (output, report) = run_runner(&runner, &binary, dir.path(), "report-time", |command| {
        command.args(["--nocapture", "--test-threads=1"]);
    });

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        attempt(&report, "tests::a_prints_a_header")["status"],
        "passed",
        "{report}"
    );
    assert_eq!(attempt(&report, "tests::b_ignored")["status"], "skipped");
    assert!(
        attempt(&report, "tests::c_works")["duration_ms"].is_u64(),
        "{report}"
    );
    assert_eq!(report["summary"]["skipped"], 1, "{report}");
}

/// Opting out keeps the plain invocation: no timing flags, no durations, and
/// failure output still attributed to each failed case.
#[test]
fn libtest_runner_without_report_time_omits_durations() {
    let dir = tempfile::TempDir::new().unwrap();
    let runner = compile_runner(dir.path());
    let binary = compile_sample_tests(dir.path());

    let (output, report) = run_runner(&runner, &binary, dir.path(), "no-report-time", |_| {});

    assert_eq!(output.status.code(), Some(101), "{output:?}");
    for case in report["cases"].as_array().unwrap() {
        assert!(case["attempts"][0].get("duration_ms").is_none(), "{report}");
    }
    let failed = attempt(&report, "tests::fails_with_output");
    assert!(failed["failure"]["message"]
        .as_str()
        .unwrap()
        .contains("numbers differ"));
    let native =
        std::fs::read_to_string(dir.path().join("no-report-time/native_results.txt")).unwrap();
    assert!(!native.contains("--report-time"), "{native}");
}

/// A binary that refuses the timing flags must run exactly as before: the
/// probe fails, the run uses the plain invocation, and a passing suite stays
/// passing with no invented durations.
#[cfg(unix)]
#[test]
fn libtest_runner_falls_back_when_the_binary_rejects_report_time() {
    let dir = tempfile::TempDir::new().unwrap();
    let runner = compile_runner(dir.path());
    let binary = dir.path().join("strict-tests");
    super::write_executable(
        &binary,
        r#"#!/bin/sh
for arg in "$@"; do
  if [ "$arg" = "-Z" ]; then
    echo "error: the option \`Z\` is only accepted on the nightly compiler" >&2
    exit 101
  fi
done
for arg in "$@"; do
  if [ "$arg" = "--list" ]; then
    printf 'tests::works: test\n\n1 test, 0 benchmarks\n'
    exit 0
  fi
done
printf '\nrunning 1 test\ntest tests::works ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n\n'
"#,
    );

    let (output, report) = run_runner(&runner, &binary, dir.path(), "report-time", |_| {});

    assert!(output.status.success(), "{output:?}");
    assert_eq!(report["status"], "passed");
    let works = attempt(&report, "tests::works");
    assert_eq!(works["status"], "passed");
    assert!(works.get("duration_ms").is_none(), "{report}");
}

/// The runner sets `RUSTC_BOOTSTRAP` only when the declared test environment
/// leaves it unset, and passes its own flags before the user's arguments so
/// a `--` separator in them cannot turn the flags into test filters.
#[cfg(unix)]
#[test]
fn libtest_runner_keeps_a_configured_rustc_bootstrap() {
    let dir = tempfile::TempDir::new().unwrap();
    let runner = compile_runner(dir.path());
    let binary = dir.path().join("recording-tests");
    let record = dir.path().join("record.txt");
    super::write_executable(
        &binary,
        &format!(
            r#"#!/bin/sh
printf '%s|%s\n' "${{RUSTC_BOOTSTRAP-unset}}" "$*" >> '{}'
for arg in "$@"; do
  if [ "$arg" = "--list" ]; then
    printf 'tests::works: test\n\n1 test, 0 benchmarks\n'
    exit 0
  fi
done
printf '\nrunning 1 test\ntest tests::works ... ok <0.250s>\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.25s\n\n'
"#,
            record.display()
        ),
    );

    let (output, report) = run_runner(&runner, &binary, dir.path(), "report-time", |command| {
        command.env("RUSTC_BOOTSTRAP", "0").arg("--").arg("works");
    });

    assert!(output.status.success(), "{output:?}");
    assert_eq!(attempt(&report, "tests::works")["duration_ms"], 250);
    let record = std::fs::read_to_string(record).unwrap();
    assert_eq!(
        record.lines().collect::<Vec<_>>(),
        vec![
            "0|--list -- works",
            "0|-Z unstable-options --report-time --list -- works",
            "0|-Z unstable-options --report-time -- works",
        ],
    );
}

/// When a test prints a verdict for another case under `--nocapture` and the
/// two verdicts disagree, the real one cannot be told apart, so that case
/// reports no verdict instead of a guessed one.
#[test]
fn libtest_runner_reports_no_verdict_for_conflicting_result_lines() {
    let dir = tempfile::TempDir::new().unwrap();
    let runner = compile_runner(dir.path());
    let binary = compile_tests(
        dir.path(),
        r#"
#[cfg(test)]
mod tests {
    #[test]
    fn a_prints_a_verdict() {
        println!();
        println!("test tests::b_fails ... ok");
    }

    #[test]
    fn b_fails() {
        panic!("real failure");
    }
}
"#,
    );

    let (output, report) = run_runner(&runner, &binary, dir.path(), "report-time", |command| {
        command.args(["--nocapture", "--test-threads=1"]);
    });

    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        attempt(&report, "tests::b_fails")["status"],
        "unknown",
        "{report}"
    );
    assert_ne!(
        attempt(&report, "tests::a_prints_a_verdict")["status"],
        "failed",
        "{report}"
    );
}
