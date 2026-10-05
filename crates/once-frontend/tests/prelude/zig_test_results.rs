//! The Zig test runner script turns the default test runner's
//! `K/N name...OK|SKIP|FAIL (error)` lines into one case per test, and falls
//! back to one case for the whole binary, carrying its measured run time and
//! failure output, whenever that output cannot be trusted.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

use super::{all_prelude_source, eval_prelude_string_function_in, write_executable};

const FAILING_RUN: &str = "1/3 math.test.adds...OK
2/3 math.test.divides...some output
expected 2, found 3
FAIL (TestExpectedEqual)
/zig/lib/std/testing.zig:118:17: 0x1001798bb in expectEqualInner (test)
                return error.TestExpectedEqual;
3/3 math.test.later...SKIP
1 passed; 1 skipped; 1 failed.
";

fn run_script(
    dir: &Path,
    output: &[u8],
    exit_code: i32,
) -> (std::process::Output, serde_json::Value) {
    let script = eval_prelude_string_function_in(
        all_prelude_source(),
        "_zig_test_script",
        r#"({"label": {"package": "pkg", "name": "tests", "id": "pkg/tests"}, "attr": {}}, "tests.bin", "out/test_results.json", "out/zig-test.log", "out/native_results.txt")"#,
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("out")).unwrap();
    std::fs::write(dir.join("canned.txt"), output).unwrap();
    write_executable(
        &dir.join("tests.bin"),
        &format!("#!/bin/sh\ncat canned.txt >&2\nexit {exit_code}\n"),
    );
    let result = Command::new("sh")
        .arg("-c")
        .arg(&script)
        .current_dir(dir)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    let report = serde_json::from_slice(&std::fs::read(dir.join("out/test_results.json")).unwrap())
        .unwrap_or_else(|error| panic!("invalid report: {error}\n{script}"));
    (result, report)
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
fn zig_runner_reports_one_case_per_test_with_failure_output() {
    let dir = tempfile::TempDir::new().unwrap();
    let (output, report) = run_script(dir.path(), FAILING_RUN.as_bytes(), 1);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(report["status"], "failed");
    assert_eq!(
        report["summary"],
        serde_json::json!({"total": 3, "passed": 1, "failed": 1, "skipped": 1, "flaky": 0})
    );
    assert_eq!(
        case(&report, "math.test.adds")["attempts"],
        serde_json::json!([{"status": "passed"}])
    );
    assert_eq!(
        case(&report, "math.test.adds")["id"],
        "pkg/tests::math.test.adds"
    );
    assert_eq!(
        case(&report, "math.test.later")["attempts"],
        serde_json::json!([{"status": "skipped"}])
    );
    let failed = &case(&report, "math.test.divides")["attempts"][0];
    assert_eq!(failed["status"], "failed");
    assert!(failed.get("duration_ms").is_none(), "{report}");
    let message = failed["failure"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("some output\nexpected 2, found 3\nFAIL (TestExpectedEqual)\n"),
        "{message}"
    );
    assert!(
        message.ends_with("return error.TestExpectedEqual;"),
        "{message}"
    );
    assert!(
        report["runner"]["metadata"]["duration_ms"].is_u64(),
        "{report}"
    );
}

#[test]
fn zig_runner_reports_every_passing_test() {
    let dir = tempfile::TempDir::new().unwrap();
    let (output, report) = run_script(
        dir.path(),
        b"1/2 math.test.adds...OK\n2/2 math.test.subtracts...OK\nAll 2 tests passed.\n",
        0,
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(report["status"], "passed");
    assert_eq!(report["summary"]["total"], 2);
    assert_eq!(report["summary"]["passed"], 2);
    for case in report["cases"].as_array().unwrap() {
        assert_eq!(case["attempts"], serde_json::json!([{"status": "passed"}]));
    }
}

/// A custom test runner, a crash, or a run that fails without a failing
/// test (leaked memory, logged errors) does not produce output the per-test
/// parser can trust. The whole binary becomes one case that carries the
/// measured run time and, on failure, the output.
#[test]
fn zig_runner_falls_back_to_one_timed_case_for_untrusted_output() {
    for (output, exit_code) in [
        ("custom runner says hello\n", 0),
        (
            "1/2 math.test.adds...OK\n2/2 math.test.crashes...panic: reached unreachable\n",
            134,
        ),
        (
            "1/1 math.test.leaks...OK\nAll 1 tests passed.\n1 tests leaked memory.\n",
            1,
        ),
    ] {
        let dir = tempfile::TempDir::new().unwrap();
        let (result, report) = run_script(dir.path(), output.as_bytes(), exit_code);

        assert_eq!(result.status.code(), Some(exit_code), "{result:?}");
        let status = if exit_code == 0 { "passed" } else { "failed" };
        assert_eq!(report["status"], status, "{report}");
        assert_eq!(report["summary"]["total"], 1, "{report}");
        let suite = case(&report, "tests");
        assert_eq!(suite["id"], "pkg/tests::suite");
        let attempt = &suite["attempts"][0];
        assert_eq!(attempt["status"], status, "{report}");
        assert!(attempt["duration_ms"].is_u64(), "{report}");
        if exit_code == 0 {
            assert!(attempt.get("failure").is_none(), "{report}");
        } else {
            assert_eq!(attempt["failure"]["message"], output.trim(), "{report}");
        }
    }
}

/// Output a test prints that looks like a runner line, invalid UTF-8, and
/// oversized failures must still produce valid JSON with bounded messages.
#[test]
fn zig_runner_keeps_reports_valid_for_hostile_output() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut bytes =
        b"1/2 math.test.prints...2/2 fake.test...OK\n\x1b[31mquote \" backslash \\ tab\t\xff\xfe "
            .to_vec();
    bytes.extend_from_slice("x".repeat(5000).as_bytes());
    bytes.extend_from_slice(
        b"\nFAIL (TestUnexpectedResult)\n2/2 math.test.ok...OK\n1 passed; 0 skipped; 1 failed.\n",
    );
    let (output, report) = run_script(dir.path(), &bytes, 1);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(report["summary"]["total"], 2, "{report}");
    let failed = &case(&report, "math.test.prints")["attempts"][0];
    let message = failed["failure"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("2/2 fake.test...OK\n\u{1b}[31mquote \" backslash \\ tab\t?? xxx"),
        "{message}"
    );
    assert_eq!(message.chars().count(), 4000);
    assert_eq!(
        case(&report, "math.test.ok")["attempts"],
        serde_json::json!([{"status": "passed"}])
    );
}

/// A test that prints a line shaped like the runner's own cannot take over
/// another test's slot: the reused index sends the run to the single suite
/// case instead of reporting a failure under an invented name.
#[test]
fn zig_runner_does_not_trust_runner_lines_printed_by_tests() {
    let dir = tempfile::TempDir::new().unwrap();
    let output = b"1/3 math.test.first...\n2/3 math.test.invented...OK\nOK\n2/3 math.test.second...FAIL (TestUnexpectedResult)\n3/3 math.test.third...OK\n1 passed; 0 skipped; 1 failed.\n";

    let (_, report) = run_script(dir.path(), output, 1);

    assert_eq!(report["status"], "failed");
    let cases = report["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 1, "{report}");
    assert_eq!(cases[0]["id"], "pkg/tests::suite");
    assert!(!report
        .to_string()
        .contains("\"name\":\"math.test.invented\""));
}
