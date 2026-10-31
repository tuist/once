#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output};

const MODULE: &str = r#"
def _impl(ctx):
    intermediate = declare_output("intermediate.txt")
    result = declare_output("result.txt")
    run_action(
        argv = ["/bin/sh", "-c", "test ! -e secret && test ! -S /var/run/docker.sock && test -d scratch && test -d .once/tmp/action-cache && test ! -e .once/tmp/action-cache/secret && test ! -e prune/stale && cat \"$1\" > \"$2\"", "sh", "input.txt", intermediate],
        inputs = ["input.txt", "prune"],
        outputs = [intermediate],
        clean_paths = ["prune/stale"],
        create_dirs = ["scratch", ".once/tmp/action-cache"],
        identifier = "prepare",
        cacheable = ctx["attr"].get("cacheable", True),
    )
    run_action(
        argv = ["/bin/sh", "-c", "tr a-z A-Z < \"$1\" > \"$2\"", "sh", intermediate, result],
        inputs = [intermediate],
        outputs = [result],
        identifier = "consume",
        cacheable = ctx["attr"].get("cacheable", True),
    )
    return {}

distributed = target_kind(
    attrs = [attr("cacheable", "bool", default = "true")],
    capabilities = [capability("build", ["default"])],
    impl = _impl,
)
"#;

fn build(root: &Path, home: &Path, target: &str, unavailable: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_once"));
    command
        .current_dir(root)
        .args([
            "--format",
            "json",
            "build",
            "--remote",
            "--compute",
            "workers",
            target,
        ])
        .env("HOME", home)
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("ONCE_CACHE_PROVIDER", "local")
        .env("ONCE_CHANGE_TRACKER", "0");
    if unavailable {
        command.env("ONCE_DOCKER_BIN", root.join("missing-docker"));
    }
    command.output().unwrap()
}

fn successful(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
#[ignore = "requires Docker and ONCE_DOCKER_TEST_IMAGE with sleep, mkdir, tar and /bin/sh"]
fn graph_actions_transfer_dependencies_restore_cache_and_never_fall_back_locally() {
    let image =
        std::env::var("ONCE_DOCKER_TEST_IMAGE").expect("set an immutable sha256 Docker image ID");
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("modules")).unwrap();
    std::fs::write(root.path().join("modules/distributed.star"), MODULE).unwrap();
    std::fs::write(
        root.path().join("once.toml"),
        format!(
            r#"
[modules]
paths = ["modules/*.star"]
[infrastructures.workers]
kind = "docker"
image = "{image}"
[[target]]
name = "cached"
kind = "distributed"
[[target]]
name = "fresh"
kind = "distributed"
[target.attrs]
cacheable = false
"#
        ),
    )
    .unwrap();
    std::fs::write(root.path().join("input.txt"), b"first asymmetric input").unwrap();
    std::fs::write(root.path().join("secret"), b"undeclared").unwrap();
    std::fs::create_dir_all(root.path().join(".once/tmp/action-cache")).unwrap();
    std::fs::write(
        root.path().join(".once/tmp/action-cache/secret"),
        b"private scratch",
    )
    .unwrap();
    std::fs::create_dir(root.path().join("prune")).unwrap();
    std::fs::write(root.path().join("prune/stale"), b"preserve host copy").unwrap();
    let record = successful(&build(root.path(), home.path(), "cached", false));
    assert!(!root.path().join("scratch").exists());
    assert_eq!(
        std::fs::read(root.path().join("prune/stale")).unwrap(),
        b"preserve host copy"
    );
    let output = record["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(serde_json::Value::as_str)
        .find(|path| path.ends_with("/result.txt"))
        .unwrap();
    let output = root.path().join(output);
    assert_eq!(std::fs::read(&output).unwrap(), b"FIRST ASYMMETRIC INPUT");
    std::fs::remove_dir_all(root.path().join(".once/out")).unwrap();
    successful(&build(root.path(), home.path(), "cached", true));
    assert_eq!(std::fs::read(&output).unwrap(), b"FIRST ASYMMETRIC INPUT");

    std::fs::write(root.path().join("input.txt"), b"second value").unwrap();
    assert!(!build(root.path(), home.path(), "cached", true)
        .status
        .success());
    successful(&build(root.path(), home.path(), "cached", false));
    assert_eq!(std::fs::read(&output).unwrap(), b"SECOND VALUE");
    assert!(!build(root.path(), home.path(), "fresh", true)
        .status
        .success());
    successful(&build(root.path(), home.path(), "fresh", false));
}

const SETUP_MODULE: &str = r#"
def _impl(ctx):
    result = declare_output("result.txt")
    args = cmd_args(["response-file-value"], use_arg_file = {"path": "scratch/args.txt", "arg_format": "{}"})
    run_action(
        argv = ["/bin/sh", "-c", "test -d scratch && test ! -e scratch/stale && cat \"$2\" > \"$1\"", "sh", result, args],
        outputs = [result],
        clean_paths = ["scratch"],
        create_dirs = ["scratch"],
        cacheable = ctx["attr"].get("cacheable", True),
    )
    return {}
setup_probe = target_kind(attrs = [attr("cacheable", "bool", default = "true")], capabilities = [capability("build", ["default"])], impl = _impl)
"#;

fn setup_fixture(root: &Path, infrastructure: &str) {
    std::fs::create_dir(root.join("modules")).unwrap();
    std::fs::write(root.join("modules/probe.star"), SETUP_MODULE).unwrap();
    std::fs::write(
        root.join("once.toml"),
        format!(
            r#"
[modules]
paths = ["modules/*.star"]
[infrastructures.workers]
{infrastructure}
[[target]]
name = "cached"
kind = "setup_probe"
[[target]]
name = "fresh"
kind = "setup_probe"
[target.attrs]
cacheable = false
"#
        ),
    )
    .unwrap();
    std::fs::create_dir(root.join("scratch")).unwrap();
    std::fs::write(root.join("scratch/stale"), b"must be removed").unwrap();
}

fn check_setup(root: &Path, home: &Path) {
    for target in ["cached", "fresh", "fresh"] {
        let record = successful(&build(root, home, target, false));
        let output = record["outputs"][0].as_str().unwrap();
        assert_eq!(
            std::fs::read(root.join(output)).unwrap(),
            b"response-file-value\n"
        );
    }
}

#[test]
fn named_native_binding_preserves_setup_and_response_files() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    setup_fixture(root.path(), "kind = \"local\"");
    check_setup(root.path(), home.path());
    assert!(!root.path().join("scratch/stale").exists());
    std::fs::write(root.path().join("scratch/stale"), b"keep on cache hit").unwrap();
    successful(&build(root.path(), home.path(), "cached", true));
    assert_eq!(
        std::fs::read(root.path().join("scratch/stale")).unwrap(),
        b"keep on cache hit"
    );
}

#[test]
#[ignore = "requires Docker and ONCE_DOCKER_TEST_IMAGE with sleep, mkdir, tar and /bin/sh"]
fn docker_response_files_survive_worker_cleanup() {
    let image = std::env::var("ONCE_DOCKER_TEST_IMAGE").unwrap();
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    setup_fixture(
        root.path(),
        &format!("kind = \"docker\"\nimage = \"{image}\""),
    );
    check_setup(root.path(), home.path());
    assert_eq!(
        std::fs::read(root.path().join("scratch/stale")).unwrap(),
        b"must be removed"
    );
}

#[test]
#[ignore = "requires Docker and ONCE_DOCKER_TEST_IMAGE with sleep, mkdir, tar and /bin/sh"]
fn docker_stages_implicit_ordered_outputs_including_cached_producers() {
    let image = std::env::var("ONCE_DOCKER_TEST_IMAGE").unwrap();
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("modules")).unwrap();
    std::fs::write(root.path().join("modules/probe.star"), r#"
def _impl(ctx):
    intermediate = declare_output("intermediate.txt")
    result = declare_output("result.txt")
    run_action(argv = ["/bin/sh", "-c", "printf producer: > \"$1\"", "sh", intermediate], outputs = [intermediate])
    explicit_only = ctx["attr"].get("explicit_only", False)
    script = "test ! -e \"$1\" && cat \"$3\" > \"$2\"" if explicit_only else "cat \"$1\" \"$3\" > \"$2\""
    run_action(
        argv = ["/bin/sh", "-c", script, "sh", intermediate, result, "consumer.txt"],
        inputs = ["consumer.txt"],
        outputs = [result],
        depends_on_prior_actions = not explicit_only,
        cacheable = ctx["attr"].get("cacheable", True),
    )
    return {}
implicit_probe = target_kind(attrs = [attr("explicit_only", "bool", default = "false"), attr("cacheable", "bool", default = "true")], capabilities = [capability("build", ["default"])], impl = _impl)
"#).unwrap();
    std::fs::write(
        root.path().join("once.toml"),
        format!(
            r#"
[modules]
paths = ["modules/*.star"]
[infrastructures.workers]
kind = "docker"
image = "{image}"
[[target]]
name = "cached"
kind = "implicit_probe"
[[target]]
name = "explicit"
kind = "implicit_probe"
[target.attrs]
explicit_only = true
[[target]]
name = "fresh"
kind = "implicit_probe"
[target.attrs]
cacheable = false
"#
        ),
    )
    .unwrap();
    for contents in ["first", "second"] {
        std::fs::write(root.path().join("consumer.txt"), contents).unwrap();
        for (target, prefix) in [
            ("cached", "producer:"),
            ("fresh", "producer:"),
            ("explicit", ""),
        ] {
            let record = successful(&build(root.path(), home.path(), target, false));
            let output = record["outputs"]
                .as_array()
                .unwrap()
                .iter()
                .find_map(|path| path.as_str().filter(|path| path.ends_with("/result.txt")))
                .unwrap();
            assert_eq!(
                std::fs::read(root.path().join(output)).unwrap(),
                format!("{prefix}{contents}").as_bytes()
            );
        }
        std::fs::remove_dir_all(root.path().join(".once/out")).unwrap();
    }
}
