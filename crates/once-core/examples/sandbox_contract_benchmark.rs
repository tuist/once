use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use once_cas::{CacheProvider, Cas, Digest};
use once_core::{
    run_uncached, validate_action_contract, Action, OutputSymlinkMode, ResourceRequest,
    SandboxMode, WorkspacePath,
};
use tempfile::TempDir;

struct Case {
    name: &'static str,
    should_report: bool,
    setup: fn(&Path),
    script: fn(&Path) -> String,
    inputs: &'static [&'static str],
    outputs: &'static [&'static str],
}

#[tokio::main]
async fn main() {
    let cases = benchmark_cases();
    let mut true_positives = 0_u64;
    let mut false_positives = 0_u64;
    let mut false_negatives = 0_u64;
    let mut actionable = 0_u64;
    let mut positives = 0_u64;
    let started = Instant::now();

    for case in &cases {
        if case.should_report {
            positives += 1;
        }
        let (reported, has_actionable_diagnostic) = run_case(case).await;
        if case.should_report && has_actionable_diagnostic {
            actionable += 1;
        }
        match (case.should_report, reported) {
            (true, true) => true_positives += 1,
            (false, true) => false_positives += 1,
            (true, false) => false_negatives += 1,
            (false, false) => {}
        }
        eprintln!(
            "case={} expected={} reported={reported}",
            case.name, case.should_report
        );
    }

    let precision = ratio(true_positives, true_positives + false_positives);
    let recall = ratio(true_positives, true_positives + false_negatives);
    let f1 = if precision + recall == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    };
    let actionable_rate = ratio(actionable, positives);
    let negative_count = cases.len() as u64 - positives;
    let false_positive_rate = ratio(false_positives, negative_count);
    let overhead = runtime_overhead().await;

    println!("METRIC detection_f1_pct={:.2}", f1 * 100.0);
    println!("METRIC precision_pct={:.2}", precision * 100.0);
    println!("METRIC recall_pct={:.2}", recall * 100.0);
    println!(
        "METRIC actionable_diagnostic_rate_pct={:.2}",
        actionable_rate * 100.0
    );
    println!(
        "METRIC false_positive_rate_pct={:.2}",
        false_positive_rate * 100.0
    );
    println!("METRIC runtime_overhead_pct={overhead:.2}");
    println!(
        "METRIC benchmark_ms={:.2}",
        started.elapsed().as_secs_f64() * 1000.0
    );
}

async fn run_case(case: &Case) -> (bool, bool) {
    let workspace = TempDir::new().expect("temporary workspace");
    (case.setup)(workspace.path());
    let cache_dir = TempDir::new().expect("temporary cache");
    let cache = CacheProvider::Local(Cas::open(cache_dir.path()));
    let action = command_action(
        (case.script)(workspace.path()),
        case.inputs,
        case.outputs,
        SandboxMode::Inputs,
    );
    validate_action_contract(&action, workspace.path(), &cache)
        .await
        .map_or((true, false), |report| {
            let actionable = report
                .diagnostics
                .iter()
                .any(|diagnostic| !diagnostic.path.is_empty() && !diagnostic.repairs.is_empty());
            (!report.valid, actionable)
        })
}

async fn runtime_overhead() -> f64 {
    let mut sandboxed = Vec::new();
    let mut validated = Vec::new();
    for _ in 0..9 {
        sandboxed.push(time_compliant_action(false).await);
        validated.push(time_compliant_action(true).await);
    }
    sandboxed.sort_unstable();
    validated.sort_unstable();
    let baseline = sandboxed[sandboxed.len() / 2].as_secs_f64();
    let validated = validated[validated.len() / 2].as_secs_f64();
    if baseline == 0.0 {
        0.0
    } else {
        ((validated - baseline) / baseline) * 100.0
    }
}

async fn time_compliant_action(validate: bool) -> Duration {
    let workspace = TempDir::new().expect("temporary workspace");
    std::fs::create_dir_all(workspace.path().join("src")).unwrap();
    std::fs::write(workspace.path().join("src/input.txt"), "hello\n").unwrap();
    let cache_dir = TempDir::new().expect("temporary cache");
    let cache = CacheProvider::Local(Cas::open(cache_dir.path()));
    let action = command_action(
        "mkdir -p out && cat src/input.txt > out/result.txt".to_string(),
        &["src/input.txt"],
        &["out/result.txt"],
        SandboxMode::Inputs,
    );
    let started = Instant::now();
    if validate {
        let report = validate_action_contract(&action, workspace.path(), &cache)
            .await
            .unwrap();
        assert!(report.valid, "{:?}", report.diagnostics);
    } else {
        let result = run_uncached(&action, workspace.path(), &cache, false)
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
    }
    started.elapsed()
}

fn command_action(
    script: String,
    inputs: &[&str],
    outputs: &[&str],
    sandbox: SandboxMode,
) -> Action {
    Action::RunCommand {
        argv: vec!["/bin/sh".to_string(), "-c".to_string(), script],
        env: BTreeMap::new(),
        cwd: None,
        input_digest: Some(Digest::of_bytes(b"contract-benchmark")),
        inputs: inputs.iter().map(|path| workspace_path(path)).collect(),
        outputs: outputs.iter().map(|path| workspace_path(path)).collect(),
        stdout_path: None,
        stderr_path: None,
        output_symlink_mode: OutputSymlinkMode::default(),
        resources: ResourceRequest::default(),
        sandbox,
        timeout_ms: None,
        remote: None,
    }
}

fn benchmark_cases() -> Vec<Case> {
    vec![
        Case {
            name: "relative_undeclared_read",
            should_report: true,
            setup: setup_secret,
            script: |_| "mkdir -p out && cat secret.txt > out/result.txt".to_string(),
            inputs: &[],
            outputs: &["out/result.txt"],
        },
        Case {
            name: "extra_write",
            should_report: true,
            setup: setup_empty,
            script: |_| {
                "mkdir -p out tmp && printf ok > out/result.txt && printf extra > tmp/log.txt"
                    .to_string()
            },
            inputs: &[],
            outputs: &["out/result.txt"],
        },
        Case {
            name: "input_mutation",
            should_report: true,
            setup: setup_input,
            script: |_| {
                "printf changed >> input.txt && mkdir -p out && printf ok > out/result.txt"
                    .to_string()
            },
            inputs: &["input.txt"],
            outputs: &["out/result.txt"],
        },
        Case {
            name: "input_deletion",
            should_report: true,
            setup: setup_input,
            script: |_| "rm input.txt && mkdir -p out && printf ok > out/result.txt".to_string(),
            inputs: &["input.txt"],
            outputs: &["out/result.txt"],
        },
        Case {
            name: "absolute_workspace_read",
            should_report: true,
            setup: setup_secret,
            script: |workspace| {
                format!(
                    "mkdir -p out && cat '{}' > out/result.txt",
                    workspace.join("secret.txt").display()
                )
            },
            inputs: &[],
            outputs: &["out/result.txt"],
        },
        Case {
            name: "absolute_workspace_write",
            should_report: true,
            setup: setup_empty,
            script: |workspace| {
                format!(
                    "mkdir -p out && printf escape > '{}' && printf ok > out/result.txt",
                    workspace.join("escaped.txt").display()
                )
            },
            inputs: &[],
            outputs: &["out/result.txt"],
        },
        Case {
            name: "declared_symlink_escape",
            should_report: true,
            setup: setup_symlink_escape,
            script: |_| "mkdir -p out && cat links/escape > out/result.txt".to_string(),
            inputs: &["links/escape"],
            outputs: &["out/result.txt"],
        },
        Case {
            name: "output_symlink_escape",
            should_report: true,
            setup: setup_secret,
            script: |workspace| {
                format!(
                    "mkdir -p out && ln -s '{}' out/escape && cat out/escape > out/result.txt",
                    workspace.join("secret.txt").display()
                )
            },
            inputs: &[],
            outputs: &["out"],
        },
        Case {
            name: "rust_source_control",
            should_report: false,
            setup: setup_rust_source,
            script: |_| "mkdir -p out && wc -c < src/lib.rs > out/result.txt".to_string(),
            inputs: &["src/lib.rs"],
            outputs: &["out/result.txt"],
        },
        Case {
            name: "javascript_source_control",
            should_report: false,
            setup: setup_javascript_source,
            script: |_| "mkdir -p out && wc -c < tests/math.test.js > out/result.txt".to_string(),
            inputs: &["tests/math.test.js"],
            outputs: &["out/result.txt"],
        },
        Case {
            name: "declared_directory_control",
            should_report: false,
            setup: setup_directory,
            script: |_| "mkdir -p out && cat src/a src/b > out/result.txt".to_string(),
            inputs: &["src"],
            outputs: &["out/result.txt"],
        },
        Case {
            name: "nested_output_control",
            should_report: false,
            setup: setup_empty,
            script: |_| "mkdir -p out/nested && printf ok > out/nested/result.txt".to_string(),
            inputs: &[],
            outputs: &["out"],
        },
    ]
}

fn setup_empty(_: &Path) {}

fn setup_secret(workspace: &Path) {
    std::fs::write(workspace.join("secret.txt"), "secret\n").unwrap();
}

fn setup_input(workspace: &Path) {
    std::fs::write(workspace.join("input.txt"), "input\n").unwrap();
}

fn setup_directory(workspace: &Path) {
    std::fs::create_dir(workspace.join("src")).unwrap();
    std::fs::write(workspace.join("src/a"), "a\n").unwrap();
    std::fs::write(workspace.join("src/b"), "b\n").unwrap();
}

fn setup_rust_source(workspace: &Path) {
    copy_fixture(
        workspace,
        "crates/once-frontend/prelude/examples/rust-crate-minimal/vendor/itoa-1.0.14/src/lib.rs",
        "src/lib.rs",
    );
}

fn setup_javascript_source(workspace: &Path) {
    copy_fixture(
        workspace,
        "crates/once-frontend/prelude/examples/vitest-test-minimal/tests/math.test.js",
        "tests/math.test.js",
    );
}

fn copy_fixture(workspace: &Path, source: &str, destination: &str) {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let destination = workspace.join(destination);
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::fs::copy(repository.join(source), destination).unwrap();
}

#[cfg(unix)]
fn setup_symlink_escape(workspace: &Path) {
    std::fs::create_dir(workspace.join("links")).unwrap();
    std::fs::write(workspace.join("secret.txt"), "secret\n").unwrap();
    std::os::unix::fs::symlink("../secret.txt", workspace.join("links/escape")).unwrap();
}

#[cfg(not(unix))]
fn setup_symlink_escape(workspace: &Path) {
    std::fs::create_dir(workspace.join("links")).unwrap();
    std::fs::write(workspace.join("links/escape"), "secret\n").unwrap();
}

fn workspace_path(path: &str) -> WorkspacePath {
    WorkspacePath::try_from(path).unwrap()
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}
