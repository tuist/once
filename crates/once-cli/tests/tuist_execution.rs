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
    directory = declare_output("tree")
    out = declare_output("tree/never.txt")
    run_action(
        argv = ["mkdir", "-p", directory],
        outputs = [directory, out],
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
    let server = tuist_execution::start_with(None, 4, true).await;
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
async fn redirected_streams_and_outputs_outside_cwd_are_restored_and_cached() {
    let server = tuist_execution::start().await;
    let root = workspace(
        &server.url,
        r#"
def _impl(ctx):
    logs = declare_output("logs")
    out = declare_output("logs/stdout 'quoted'.txt")
    err = declare_output("logs/stderr.txt")
    merged = declare_output("logs/merged.txt")
    result = declare_output("result.txt")
    run_action(
        argv = ["/bin/sh", "-c", "cat input.txt > \"$1\"; printf '%s' \"$2\"; printf diagnostic >&2; exit 23", "sh", execution_path(result), "literal $HOME; $(touch injected)"],
        inputs = ["nested work/input.txt"],
        outputs = [result, logs, out, err],
        stdout = out,
        stderr = err,
        cwd = "nested work",
        success_exit_codes = [23],
        identifier = "separate",
    )
    run_action(
        argv = ["/bin/sh", "-c", "printf first >&2; printf second"],
        outputs = [merged],
        stdout = merged,
        stderr = merged,
        cwd = "empty work",
        create_dirs = ["empty work"],
        identifier = "merged",
    )
    return {}

distributed = target_kind(
    tools = [tool("sh", executables = ["sh"])],
    capabilities = [capability("build", ["default"])],
    impl = _impl,
)
"#,
        "",
    );
    let home = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("mise.toml"), "[tools]\n").unwrap();
    let mise = home.path().join(format!(
        ".local/share/once/tools/mise/{}/mise",
        once_core::MANAGED_MISE_VERSION
    ));
    std::fs::create_dir_all(mise.parent().unwrap()).unwrap();
    std::fs::write(
        &mise,
        "#!/bin/sh\ncase \"$1\" in\nwhich) printf /bin/sh;;\nexec) exit 97;;\n*) exit 0;;\nesac\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&mise, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::create_dir(root.path().join("nested work")).unwrap();
    std::fs::write(
        root.path().join("nested work/input.txt"),
        "from working directory",
    )
    .unwrap();

    for cold_client in [false, true] {
        if cold_client {
            std::fs::remove_dir_all(root.path().join(".once")).unwrap();
            std::fs::remove_dir_all(home.path().join("cache")).unwrap();
        }
        let output = build(root.path(), home.path()).await;
        assert!(output.status.success(), "{}", stderr(&output));
        let record: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let outputs = record["outputs"].as_array().unwrap();
        for (suffix, expected) in [
            ("/result.txt", "from working directory"),
            ("/stdout 'quoted'.txt", "literal $HOME; $(touch injected)"),
            ("/stderr.txt", "diagnostic"),
            ("/merged.txt", "firstsecond"),
        ] {
            let path = outputs
                .iter()
                .filter_map(serde_json::Value::as_str)
                .find(|path| path.ends_with(suffix))
                .expect("declared output is reported");
            assert_eq!(
                std::fs::read_to_string(root.path().join(path)).unwrap(),
                expected
            );
        }
        assert_eq!(server.service.state.lock().await.executed.len(), 2);
        assert!(
            !root.path().join("empty work").exists(),
            "setup stays remote"
        );
    }

    let module = root.path().join("modules/distributed.star");
    let source = std::fs::read_to_string(&module).unwrap();
    std::fs::write(
        &module,
        source.replace("success_exit_codes = [23]", "success_exit_codes = [0]"),
    )
    .unwrap();
    std::fs::remove_dir_all(root.path().join(".once")).unwrap();
    let failed = build(root.path(), home.path()).await;
    assert!(!failed.status.success());
    assert!(stderr(&failed).contains("23"), "{}", stderr(&failed));
    let diagnostic = walkdir::WalkDir::new(root.path().join(".once/out"))
        .into_iter()
        .filter_map(Result::ok)
        .find(|entry| entry.file_name() == "stderr.txt")
        .expect("failed command diagnostics are restored");
    assert_eq!(
        std::fs::read_to_string(diagnostic.path()).unwrap(),
        "diagnostic"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn parallel_actions_respect_the_advertised_concurrency_and_upload_only_changed_inputs() {
    let server = tuist_execution::start_with(None, 1, false).await;
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

/// A workspace for `MODULE` whose first action hits `faults` in order.
async fn faulted(
    faults: impl IntoIterator<Item = tuist_execution::Fault>,
) -> (tuist_execution::Server, tempfile::TempDir) {
    let server = tuist_execution::start().await;
    let root = workspace(&server.url, MODULE, "");
    std::fs::write(root.path().join("input.txt"), b"asymmetric Input").unwrap();
    std::fs::create_dir(root.path().join("prune")).unwrap();
    server.service.state.lock().await.faults.extend(faults);
    (server, root)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_credential_rejected_on_an_open_stream_is_refreshed_and_the_operation_reattached() {
    let (server, root) = faulted([tuist_execution::Fault::RejectToken]).await;
    let home = tempfile::tempdir().unwrap();

    let output = build(root.path(), home.path()).await;
    assert!(output.status.success(), "{}", stderr(&output));
    let state = server.service.state.lock().await;
    assert_eq!(
        state.executed.len(),
        2,
        "the accepted action is not submitted again"
    );
    assert_eq!(state.waits, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_worker_is_resubmitted_and_a_busy_gateway_is_waited_out() {
    let (server, root) = faulted([
        tuist_execution::Fault::WorkerFailure,
        tuist_execution::Fault::Break,
    ])
    .await;
    server.service.state.lock().await.busy_waits = 2;
    let home = tempfile::tempdir().unwrap();

    let output = build(root.path(), home.path()).await;
    assert!(output.status.success(), "{}", stderr(&output));
    let state = server.service.state.lock().await;
    assert_eq!(state.executed.len(), 2);
    assert_eq!(state.waits, 3, "two busy answers, then the reattach");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_operation_lost_on_every_submission_fails_without_uploading_again() {
    let (server, root) = faulted([
        tuist_execution::Fault::Lose,
        tuist_execution::Fault::Lose,
        tuist_execution::Fault::Lose,
    ])
    .await;
    let home = tempfile::tempdir().unwrap();

    let output = build(root.path(), home.path()).await;
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("lost the operation"),
        "{}",
        stderr(&output)
    );
    let state = server.service.state.lock().await;
    assert_eq!(state.executed.len(), 3);
    assert_eq!(
        state
            .uploads
            .iter()
            .filter(|hash| **hash == sha256(b"asymmetric Input"))
            .count(),
        1,
        "a lost operation is not missing inputs"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_output_larger_than_the_budget_is_refused_before_it_is_downloaded() {
    let (_server, root) = faulted([tuist_execution::Fault::Oversize]).await;
    let home = tempfile::tempdir().unwrap();

    let output = build(root.path(), home.path()).await;
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("output bytes limit"),
        "{}",
        stderr(&output)
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
    let server = tuist_execution::start_with(Some(worker_image()), 4, false).await;
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
