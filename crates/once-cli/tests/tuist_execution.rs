#![cfg(unix)]

use std::path::Path;
use std::process::Output;

#[path = "support/tuist_execution.rs"]
mod tuist_execution;

const MODULE: &str = r#"
def _impl(ctx):
    intermediate = declare_output("intermediate.txt")
    result = declare_output("result")
    run_action(
        argv = ["/bin/sh", "-c", "test ! -e secret && test -d scratch && test ! -e prune/stale && cat \"$1\" > \"$2\"", "sh", "input.txt", intermediate],
        inputs = ["input.txt", "prune"],
        outputs = [intermediate],
        clean_paths = ["prune/stale"],
        create_dirs = ["scratch"],
        identifier = "prepare",
    )
    run_action(
        argv = ["/bin/sh", "-c", "mkdir -p \"$2/nested\" && tr a-z A-Z < \"$1\" > \"$2/nested/upper.txt\" && echo remote-log", "sh", intermediate, result],
        inputs = [intermediate],
        outputs = [result],
        identifier = "consume",
    )
    return {}

distributed = target_kind(
    capabilities = [capability("build", ["default"])],
    impl = _impl,
)
"#;

const PARALLEL: &str = r#"
def _piece_impl(ctx):
    stem = ctx["attr"]["stem"]
    out = declare_output(stem + ".txt")
    run_action(
        argv = ["sh", "-c", "tr a-z A-Z < \"$1\" > \"$2\"", "sh", stem + ".in", out],
        inputs = [stem + ".in"],
        outputs = [out],
        identifier = stem,
    )
    return {}

piece = target_kind(
    attrs = [attr("stem", "string", required = True)],
    providers = ["piece"],
    capabilities = [capability("build", ["default"])],
    impl = _piece_impl,
)

distributed = target_kind(
    deps = [dep("pieces", ["piece"])],
    capabilities = [capability("build", ["default"])],
    impl = lambda ctx: {},
)
"#;

/// Independent targets, so their actions can run at the same time.
const PIECES: &str = r#"
[target.dependencies]
pieces = ["./a", "./b", "./c"]
[[target]]
name = "a"
kind = "piece"
[target.attrs]
stem = "a"
[[target]]
name = "b"
kind = "piece"
[target.attrs]
stem = "b"
[[target]]
name = "c"
kind = "piece"
[target.attrs]
stem = "c"
"#;

const ABSENT: &str = r#"
def _impl(ctx):
    out = declare_output("never.txt")
    run_action(
        argv = ["/bin/sh", "-c", "true", "sh", out],
        outputs = [out],
        identifier = "absent",
    )
    return {}

distributed = target_kind(
    capabilities = [capability("build", ["default"])],
    impl = _impl,
)
"#;

fn workspace(url: &str, module: &str, targets: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("modules")).unwrap();
    std::fs::write(root.path().join("modules/distributed.star"), module).unwrap();
    std::fs::write(
        root.path().join("once.toml"),
        manifest(url, tuist_execution::IMAGE) + targets,
    )
    .unwrap();
    root
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    hex::encode(sha2::Sha256::digest(bytes))
}

fn manifest(url: &str, image: &str) -> String {
    format!(
        r#"
[modules]
paths = ["modules/*.star"]
[infrastructure.cache]
provider = "tuist"
[infrastructure.execution]
provider = "tuist"
[infrastructures.tuist]
kind = "tuist"
url = "{url}"
account = "acme"
project = "app"
execution_image = "{image}"
[[target]]
name = "app"
kind = "distributed"
"#
    )
}

async fn build(root: &Path, home: &Path) -> Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_once"));
    command
        .current_dir(root)
        .args(["--format", "json", "build", "--remote", "app"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("TUIST_TOKEN", "test-token")
        .env("ONCE_CHANGE_TRACKER", "0");
    command.output().await.unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn graph_actions_run_on_the_tuist_execution_service_and_restore_from_its_cache() {
    let server = tuist_execution::start().await;
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("modules")).unwrap();
    std::fs::write(root.path().join("modules/distributed.star"), MODULE).unwrap();
    std::fs::write(
        root.path().join("once.toml"),
        manifest(&server.url, tuist_execution::IMAGE),
    )
    .unwrap();
    std::fs::write(root.path().join("input.txt"), b"asymmetric Input").unwrap();
    std::fs::write(root.path().join("secret"), b"undeclared").unwrap();
    std::fs::create_dir(root.path().join("prune")).unwrap();
    std::fs::write(root.path().join("prune/stale"), b"host copy").unwrap();
    // The first action runs, then its operation is lost: Once must submit it
    // again rather than fail or run it locally.
    server
        .service
        .state
        .lock()
        .await
        .faults
        .push_back(tuist_execution::Fault::Lose);

    let first = build(root.path(), home.path()).await;
    assert!(first.status.success(), "{}", stderr(&first));
    let record: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    let result = record["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(serde_json::Value::as_str)
        .find(|path| path.ends_with("/result"))
        .expect("result directory is reported");
    assert_eq!(
        std::fs::read_to_string(root.path().join(result).join("nested/upper.txt")).unwrap(),
        "ASYMMETRIC INPUT"
    );
    assert_eq!(
        std::fs::read(root.path().join("prune/stale")).unwrap(),
        b"host copy"
    );
    assert!(
        !root.path().join("scratch").exists(),
        "setup stays on the worker"
    );
    {
        let state = server.service.state.lock().await;
        assert_eq!(
            state.executed.len(),
            3,
            "the lost action is submitted once more"
        );
        assert_eq!(state.executed[0], state.executed[1]);
        for platform in &state.platforms {
            assert_eq!(
                platform,
                &[
                    ("Arch".to_string(), "amd64".to_string()),
                    ("OSFamily".into(), "Linux".into()),
                    ("tuist-image".into(), tuist_execution::IMAGE.into()),
                ]
            );
        }
    }

    std::fs::remove_dir_all(root.path().join(".once")).unwrap();
    std::fs::remove_dir_all(home.path().join("cache")).unwrap();
    let second = build(root.path(), home.path()).await;
    assert!(second.status.success(), "{}", stderr(&second));
    assert_eq!(
        server.service.state.lock().await.executed.len(),
        3,
        "a clean checkout restores both actions from the project cache"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join(result).join("nested/upper.txt")).unwrap(),
        "ASYMMETRIC INPUT"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mismatched_worker_image_fails_before_anything_runs() {
    let server = tuist_execution::start().await;
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("modules")).unwrap();
    std::fs::write(root.path().join("modules/distributed.star"), MODULE).unwrap();
    std::fs::write(
        root.path().join("once.toml"),
        manifest(&server.url, "sha-stale"),
    )
    .unwrap();
    std::fs::write(root.path().join("input.txt"), b"input").unwrap();
    std::fs::create_dir(root.path().join("prune")).unwrap();

    let output = build(root.path(), home.path()).await;
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("execution_image `sha-stale` does not match the image `sha-test`"),
        "{}",
        stderr(&output)
    );
    assert!(server.service.state.lock().await.executed.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn parallel_actions_respect_the_advertised_concurrency_and_upload_only_changed_inputs() {
    let server = tuist_execution::start_with(None, 1).await;
    let root = workspace(&server.url, PARALLEL, PIECES);
    let home = tempfile::tempdir().unwrap();
    for name in ["a", "b", "c"] {
        std::fs::write(
            root.path().join(format!("{name}.in")),
            format!("input {name}"),
        )
        .unwrap();
    }
    // A full queue is backpressure: Once waits for room instead of failing.
    server.service.state.lock().await.refuse_queue = 2;

    let first = build(root.path(), home.path()).await;
    assert!(first.status.success(), "{}", stderr(&first));
    {
        let state = server.service.state.lock().await;
        assert_eq!(state.executed.len(), 3);
        assert_eq!(state.over_limit, 0, "Once never exceeds max_concurrency");
        assert_eq!(state.max_in_flight, 1);
        assert_eq!(
            state.executed[0][0], "sh",
            "a bare program resolves on the worker PATH"
        );
    }

    let uploaded = server.service.state.lock().await.uploads.len();
    std::fs::write(root.path().join("b.in"), b"changed b").unwrap();
    let second = build(root.path(), home.path()).await;
    assert!(second.status.success(), "{}", stderr(&second));
    let state = server.service.state.lock().await;
    assert_eq!(
        state.executed.len(),
        4,
        "only the changed action runs again: {:?}",
        state.executed
    );
    let fresh = &state.uploads[uploaded..];
    assert!(fresh.contains(&sha256(b"changed b")));
    for unchanged in [b"input a".as_slice(), b"input c", b"input b"] {
        assert!(
            !fresh.contains(&sha256(unchanged)),
            "{} is uploaded again",
            String::from_utf8_lossy(unchanged)
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_broken_stream_reattaches_and_an_evicted_input_is_uploaded_again() {
    let server = tuist_execution::start().await;
    let root = workspace(&server.url, MODULE, "");
    let home = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("input.txt"), b"asymmetric Input").unwrap();
    std::fs::create_dir(root.path().join("prune")).unwrap();
    server
        .service
        .state
        .lock()
        .await
        .faults
        .extend([tuist_execution::Fault::Evict, tuist_execution::Fault::Break]);

    let output = build(root.path(), home.path()).await;
    assert!(output.status.success(), "{}", stderr(&output));
    let state = server.service.state.lock().await;
    assert_eq!(
        state.executed.len(),
        2,
        "a reattached action is not submitted again"
    );
    assert_eq!(
        state
            .uploads
            .iter()
            .filter(|hash| **hash == sha256(b"asymmetric Input"))
            .count(),
        2,
        "the evicted input is uploaded once more"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_timed_out_action_fails_as_a_timeout() {
    let server = tuist_execution::start().await;
    let root = workspace(&server.url, PARALLEL, PIECES);
    let home = tempfile::tempdir().unwrap();
    for name in ["a", "b", "c"] {
        std::fs::write(
            root.path().join(format!("{name}.in")),
            format!("input {name}"),
        )
        .unwrap();
    }
    server
        .service
        .state
        .lock()
        .await
        .faults
        .push_back(tuist_execution::Fault::TimeOut);

    let output = build(root.path(), home.path()).await;
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("exceeded its timeout"),
        "{}",
        stderr(&output)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_declared_output_the_worker_did_not_produce_fails_the_action() {
    let server = tuist_execution::start().await;
    let root = workspace(&server.url, ABSENT, "");
    let home = tempfile::tempdir().unwrap();

    let output = build(root.path(), home.path()).await;
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("was not produced"),
        "{}",
        stderr(&output)
    );
}

fn worker_image() -> String {
    std::env::var("ONCE_TUIST_WORKER_IMAGE").expect(
        "set ONCE_TUIST_WORKER_IMAGE to a linux/amd64 image with /bin/sh, cat, tr and mkdir",
    )
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Docker and ONCE_TUIST_WORKER_IMAGE"]
async fn graph_actions_run_in_a_simulated_linux_tuist_worker() {
    let server = tuist_execution::start_with(Some(worker_image()), 4).await;
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("modules")).unwrap();
    std::fs::write(root.path().join("modules/distributed.star"), MODULE).unwrap();
    std::fs::write(
        root.path().join("once.toml"),
        manifest(&server.url, tuist_execution::IMAGE),
    )
    .unwrap();
    std::fs::write(root.path().join("input.txt"), b"asymmetric Input").unwrap();
    std::fs::write(root.path().join("secret"), b"undeclared").unwrap();
    std::fs::create_dir(root.path().join("prune")).unwrap();
    std::fs::write(root.path().join("prune/stale"), b"host copy").unwrap();

    let output = build(root.path(), home.path()).await;
    assert!(output.status.success(), "{}", stderr(&output));
    let record: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let result = record["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(serde_json::Value::as_str)
        .find(|path| path.ends_with("/result"))
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join(result).join("nested/upper.txt")).unwrap(),
        "ASYMMETRIC INPUT"
    );
    assert_eq!(server.service.state.lock().await.executed.len(), 2);
}
