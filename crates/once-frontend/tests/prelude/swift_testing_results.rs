use super::{apple_prelude_source, eval_prelude_source_to_repr, eval_prelude_string_function};

/// Reading a declaration proves a case exists, never that it ran or how it
/// ended. Source can compile away behind a condition and a runner can filter
/// or never reach a case, so a listing derived from sources reports no
/// verdict, and the counts it summarizes stay at zero.
#[cfg(unix)]
#[test]
fn a_listing_taken_from_sources_reports_no_verdict() {
    let script = eval_prelude_string_function(
        "_apple_test_report_script",
        r#"(["Suite.swift"], "cases.jsonl", "tests/Bundle", "swift_testing")"#,
    )
    .unwrap();

    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("Suite.swift"),
        "import Testing\nstruct MathSuite {\n  @Test func addsNumbers() {}\n}\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("run.log"), "").unwrap();
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "status=0\nlog=run.log\nnative_results=native.txt\nresults=results.json\n{script}"
        ))
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}\n{script}");

    let results = std::fs::read_to_string(dir.path().join("results.json")).unwrap();
    assert!(
        results.contains(r#""id":"tests/Bundle::MathSuite/addsNumbers""#),
        "{results}"
    );
    assert!(results.contains(r#""status":"unknown""#), "{results}");
    assert!(
        results.contains(r#""summary":{"total":1,"passed":0,"failed":0,"skipped":0,"flaky":0}"#),
        "a listing must not credit itself with outcomes: {results}"
    );
    assert!(
        results.contains(r#""status":"passed""#),
        "the run's own outcome is known: {results}"
    );
}

#[cfg(unix)]
#[test]
fn native_test_output_supplies_case_verdicts_and_durations() {
    let script = eval_prelude_string_function(
        "_apple_test_report_script",
        r#"(["Suite.swift"], "cases.jsonl", "tests/Bundle", "swift_testing")"#,
    )
    .unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("Suite.swift"),
        "import XCTest\nclass ParserTests: XCTestCase {\n  func testKnown() {}\n  func testNotReached() {}\n}\nstruct SwiftSuite {\n  @Test func computes() {}\n}\nstruct OtherSuite {\n  @Test func computes() {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("run.log"),
        "Test Case '-[Bundle.ParserTests testKnown]' passed (0.125 seconds).\n◇ Suite SwiftSuite started.\n✘ Test computes() failed after 0.250 seconds.\n✔ Suite SwiftSuite passed after 0.250 seconds.\n◇ Suite OtherSuite started.\n✔ Test computes() passed after 0.040 seconds.\n✔ Suite OtherSuite passed after 0.040 seconds.\n✔ Test generated() passed after 0.010 seconds.\n✔ Test run with 4 tests passed after 0.010 seconds.\n",
    )
    .unwrap();
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "status=1\nlog=run.log\nnative_results=native.txt\nresults=results.json\n{script}"
        ))
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}\n{script}");
    let results: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("results.json")).unwrap()).unwrap();
    assert_eq!(results["status"], "failed");
    assert_eq!(results["summary"]["total"], 5);
    assert_eq!(results["summary"]["passed"], 3, "{results}");
    assert_eq!(results["summary"]["failed"], 1);
    let cases = results["cases"].as_array().unwrap();
    let named = |name: &str| cases.iter().find(|case| case["name"] == name).unwrap();
    assert_eq!(named("testKnown")["status"], "passed");
    assert_eq!(named("testKnown")["attempts"][0]["duration_ms"], 125);
    assert_eq!(named("testNotReached")["status"], "unknown");
    assert_eq!(
        cases
            .iter()
            .find(|case| case["suite"] == "SwiftSuite" && case["name"] == "computes")
            .unwrap()["status"],
        "failed"
    );
    assert_eq!(
        cases
            .iter()
            .find(|case| case["suite"] == "OtherSuite" && case["name"] == "computes")
            .unwrap()["status"],
        "passed"
    );
    assert_eq!(named("generated")["status"], "passed");
}

/// Only the `XCTest` host runs `XCTest` cases, so a bundle that has any is left
/// to it. A bundle without them is free to run through the testing library's own
/// entry point, which reports every test and what became of it.
#[test]
fn a_bundle_holding_xctest_cases_stays_with_the_xctest_host() {
    let result = eval_prelude_source_to_repr(format!(
        "{}\n{}",
        apple_prelude_source(),
        r##"
sources = {
    "/ws/Suite.swift": "import Testing\n@Test func a() {}\n",
    "/ws/Legacy.swift": "import XCTest\nclass Legacy: XCTestCase {}\n",
    "/ws/Bridge.m": "#import <XCTest/XCTest.h>\n",
}
def workspace_root():
    return "/ws"
def host_file_exists(path):
    return path in sources
def host_file_read(path):
    return sources[path]
result = repr([
    _apple_sources_import_xctest(["Suite.swift"]),
    _apple_sources_import_xctest(["Suite.swift", "Legacy.swift"]),
    _apple_sources_import_xctest(["Bridge.m"]),
    _apple_sources_import_xctest(["Missing.swift"]),
    _apple_swift_testing_filter("MathSuite/addsNumbers"),
    _apple_swift_testing_helper("/Toolchains/absent.xctoolchain/usr/bin/swiftc"),
])
"##
    ))
    .unwrap();

    assert_eq!(
        result,
        r#"[False, True, True, False, "MathSuite/addsNumbers\\(", ""]"#
    );
}

/// The testing library's event stream is the record of what ran. Compile the
/// reporting half of the generated entry point on its own and feed it a stream
/// to confirm the normalized results follow the stream rather than the exit
/// status: a known issue is not a failure, an unreached test claims no pass,
/// and a recorded failure is reported as one.
#[cfg(target_os = "macos")]
#[test]
fn normalized_results_follow_the_event_stream() {
    let source =
        eval_prelude_string_function("_apple_swift_testing_entry_point_source", "()").unwrap();
    let reporting = source
        .split_once("@main")
        .expect("the generated source declares an entry point")
        .0
        .to_string();

    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("Report.swift"), &reporting).unwrap();
    std::fs::write(
        dir.path().join("main.swift"),
        "OnceTestReport.write(exitCode: 0)\n",
    )
    .unwrap();

    std::fs::write(dir.path().join("events.jsonl"), event_stream()).unwrap();

    let compiled = std::process::Command::new("xcrun")
        .args([
            "swiftc",
            "-Onone",
            "-o",
            "report",
            "Report.swift",
            "main.swift",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    let run = std::process::Command::new("./report")
        .current_dir(dir.path())
        .env("ONCE_TEST_RESULTS", "results.json")
        .env("ONCE_TEST_EVENT_STREAM", "events.jsonl")
        .env("ONCE_TEST_TARGET", "tests/Bundle")
        .env("ONCE_TEST_LOG", "run.log")
        .env("ONCE_TEST_NATIVE_RESULTS", "native.txt")
        .output()
        .unwrap();
    assert!(run.status.success(), "{run:?}");

    let results = std::fs::read_to_string(dir.path().join("results.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&results).unwrap();
    assert_eq!(parsed["status"], "failed", "{results}");
    assert_eq!(parsed["summary"]["total"], 4, "{results}");
    assert_eq!(
        parsed["summary"]["passed"], 2,
        "a known issue is not a failure: {results}"
    );
    assert_eq!(parsed["summary"]["failed"], 1, "{results}");
    assert_eq!(
        parsed["summary"]["skipped"], 1,
        "a test the run never reached claims no pass: {results}"
    );
    let by_name = |name: &str| {
        parsed["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == name)
            .unwrap_or_else(|| panic!("{name} missing from {results}"))
            .clone()
    };
    assert_eq!(by_name("passes")["status"], "passed");
    assert_eq!(by_name("failsForReal")["status"], "failed");
    assert_eq!(by_name("hasKnownIssue")["status"], "passed");
    assert_eq!(by_name("neverRuns")["status"], "skipped");
    assert_eq!(by_name("passes")["id"], "tests/Bundle::Suite/passes");
    assert_eq!(by_name("passes")["suite"], "Suite");
}

/// One run's worth of records, in the shape the testing library writes them:
/// a test that passes, one that records a real failure, one whose only issue
/// the test itself marks as known, and one the run never reaches.
#[cfg(target_os = "macos")]
fn event_stream() -> String {
    let suite = r#"{"kind":"test","payload":{"kind":"suite","id":"M.Suite","name":"Suite","sourceLocation":{"_filePath":"Tests/Suite.swift","line":1,"column":1}},"version":0}"#;
    let test = |name: &str| {
        format!(
            r#"{{"kind":"test","payload":{{"kind":"function","id":"M.Suite/{name}()/Tests/Suite.swift:2:3","name":"{name}()","isParameterized":false,"sourceLocation":{{"_filePath":"Tests/Suite.swift","line":2,"column":3}}}},"version":0}}"#
        )
    };
    let event = |kind: &str, name: &str, extra: &str| {
        format!(
            r#"{{"kind":"event","payload":{{"kind":"{kind}","testID":"M.Suite/{name}()/Tests/Suite.swift:2:3","messages":[]{extra}}},"version":0}}"#
        )
    };
    [
        suite.to_string(),
        test("passes"),
        test("failsForReal"),
        test("hasKnownIssue"),
        test("neverRuns"),
        event("testStarted", "passes", ""),
        event("testEnded", "passes", ""),
        event("testStarted", "failsForReal", ""),
        event(
            "issueRecorded",
            "failsForReal",
            r#","issue":{"isKnown":false,"isFailure":true,"severity":"error"}"#,
        ),
        event("testEnded", "failsForReal", ""),
        event("testStarted", "hasKnownIssue", ""),
        event(
            "issueRecorded",
            "hasKnownIssue",
            r#","issue":{"isKnown":true,"isFailure":true,"severity":"error"}"#,
        ),
        event("testEnded", "hasKnownIssue", ""),
    ]
    .join("\n")
}
