//! The instrumentation runner normalizes `am instrument -r` output. Failed
//! cases carry the `stack=` value of their status block, and every case that
//! ran carries the time the runner observed between its start and end blocks.
#![cfg(unix)]

use std::process::Command;

use super::{android_prelude_source, eval_prelude_string_function_in, write_executable};

const FAKE_ADB: &str = r#"#!/bin/sh
case " $* " in
  *" instrument "*) ;;
  *) exit 0 ;;
esac
block() {
  printf 'INSTRUMENTATION_STATUS: class=com.example.MathTest\r\n'
  printf 'INSTRUMENTATION_STATUS: current=%s\r\n' "$2"
  printf 'INSTRUMENTATION_STATUS: id=AndroidJUnitRunner\r\n'
  printf 'INSTRUMENTATION_STATUS: numtests=3\r\n'
  printf 'INSTRUMENTATION_STATUS: stream=\r\n'
  printf 'INSTRUMENTATION_STATUS: test=%s\r\n' "$1"
}
block adds 1
printf 'INSTRUMENTATION_STATUS_CODE: 1\r\n'
sleep 0.3
block adds 1
printf 'INSTRUMENTATION_STATUS_CODE: 0\r\n'
block divides 2
printf 'INSTRUMENTATION_STATUS_CODE: 1\r\n'
printf 'INSTRUMENTATION_STATUS: class=com.example.MathTest\r\n'
printf 'INSTRUMENTATION_STATUS: current=2\r\n'
printf 'INSTRUMENTATION_STATUS: id=AndroidJUnitRunner\r\n'
printf 'INSTRUMENTATION_STATUS: numtests=3\r\n'
printf 'INSTRUMENTATION_STATUS: stack=java.lang.AssertionError: expected:<2> but was:<3>\r\n'
printf '\tat org.junit.Assert.fail(Assert.java:89)\r\n'
printf '\tat com.example.MathTest.divides(MathTest.java:14)\r\n'
printf '\r\n'
printf 'INSTRUMENTATION_STATUS: stream=\r\n'
printf 'Error in divides(com.example.MathTest):\r\n'
printf 'java.lang.AssertionError: expected:<2> but was:<3>\r\n'
printf 'INSTRUMENTATION_STATUS: test=divides\r\n'
printf 'INSTRUMENTATION_STATUS_CODE: -2\r\n'
block later 3
printf 'INSTRUMENTATION_STATUS_CODE: 1\r\n'
block later 3
printf 'INSTRUMENTATION_STATUS_CODE: -3\r\n'
printf 'INSTRUMENTATION_RESULT: stream=\r\n'
printf 'Tests run: 2,  Failures: 1\r\n'
printf 'INSTRUMENTATION_CODE: -1\r\n'
"#;

#[test]
fn instrumentation_runner_reports_stacks_and_observed_durations() {
    let dir = tempfile::TempDir::new().unwrap();
    let source = eval_prelude_string_function_in(
        android_prelude_source(),
        "_android_instrumentation_runner_source",
        "()",
    )
    .unwrap();
    let source_path = dir.path().join("OnceAndroidInstrumentationRunner.java");
    std::fs::write(&source_path, source).unwrap();
    let classes = dir.path().join("classes");
    let compile = Command::new("javac")
        .arg("-d")
        .arg(&classes)
        .arg(&source_path)
        .output()
        .expect("javac");
    assert!(compile.status.success(), "{compile:?}");
    let adb = dir.path().join("adb");
    write_executable(&adb, FAKE_ADB);
    let results = dir.path().join("out/test_results.json");

    let output = Command::new("java")
        .arg("-cp")
        .arg(&classes)
        .arg("OnceAndroidInstrumentationRunner")
        .arg(&results)
        .arg(dir.path().join("out/instrumentation.log"))
        .arg(dir.path().join("out/native.txt"))
        .arg("tests/device")
        .arg(&adb)
        .args([
            "",
            "com.example",
            "com.example.test",
            "com.example.test/androidx.test.runner.AndroidJUnitRunner",
            "app.apk",
            "test.apk",
            "false",
            "false",
            "true",
            "0",
        ])
        .output()
        .expect("java");

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&results).unwrap()).unwrap();
    assert_eq!(
        report["summary"],
        serde_json::json!({"total": 3, "passed": 1, "failed": 1, "skipped": 1, "flaky": 0}),
        "{report}"
    );
    let attempt = |name: &str| {
        let case = report["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == name)
            .unwrap_or_else(|| panic!("missing {name}: {report}"));
        let attempts = case["attempts"].as_array().unwrap();
        assert_eq!(attempts.len(), 1, "{report}");
        attempts[0].clone()
    };

    let adds = attempt("adds");
    assert_eq!(adds["status"], "passed");
    assert!(adds["duration_ms"].as_u64().unwrap() >= 250, "{report}");
    assert!(adds.get("failure").is_none(), "{report}");

    let divides = attempt("divides");
    assert_eq!(divides["status"], "failed");
    // Its start and end blocks can arrive in one read, and then no duration
    // was observed.
    assert!(
        divides
            .get("duration_ms")
            .is_none_or(serde_json::Value::is_u64),
        "{report}"
    );
    assert_eq!(
        divides["failure"]["message"],
        "java.lang.AssertionError: expected:<2> but was:<3>\n\tat org.junit.Assert.fail(Assert.java:89)\n\tat com.example.MathTest.divides(MathTest.java:14)"
    );

    let later = attempt("later");
    assert_eq!(later, serde_json::json!({"status": "skipped"}));

    let log = std::fs::read_to_string(dir.path().join("out/instrumentation.log")).unwrap();
    assert!(
        log.contains("INSTRUMENTATION_STATUS_CODE: 0\r\n"),
        "the log keeps the raw output"
    );
}
