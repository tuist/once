use std::collections::BTreeMap;
use std::sync::Arc;

use super::*;
use crate::{
    Action, Error, ExecutorResources, NetworkPolicy, OutputSymlinkMode, ResourceLimits,
    ResourcePool, ResourceRequest, SandboxMode, WorkspacePath,
};

mod cancellation;
#[cfg(unix)]
mod context;

fn action(argv: &[&str], image: String) -> Action {
    let mut binding = RemoteExecution::provider("docker-test-fleet");
    binding.executor = Some("docker".into());
    binding.environment = Some(image);
    Action::RunCommand {
        argv: argv.iter().map(|arg| (*arg).to_string()).collect(),
        env: BTreeMap::new(),
        cwd: None,
        input_digest: None,
        inputs: vec![],
        outputs: vec![],
        stdout_path: None,
        stderr_path: None,
        output_symlink_mode: OutputSymlinkMode::default(),
        resources: ResourceRequest::default(),
        sandbox: SandboxMode::default(),
        network: NetworkPolicy::default(),
        timeout_ms: Some(30_000),
        success_exit_codes: vec![0],
        remote: Some(Box::new(binding)),
    }
}

fn resources() -> ExecutorResources {
    ExecutorResources::new(Arc::new(ResourcePool::new(ResourceLimits::new(
        1,
        512 * 1024 * 1024,
    ))))
}

#[test]
fn requires_an_explicit_immutable_image_and_shares_host_capacity() {
    assert_eq!(
        Executor::Docker.resource_domain(),
        crate::ResourceDomain::Host
    );
    for image in [
        "",
        "alpine:latest",
        "sha256:short",
        &format!("sha256:{}", "x".repeat(64)),
    ] {
        assert!(Executor::resolve(&action(&["true"], image.into())).is_err());
    }
    assert_eq!(
        Executor::resolve(&action(&["true"], format!("sha256:{}", "a".repeat(64)))).unwrap(),
        Executor::Docker
    );
}

#[test]
fn unsupported_requirements_fail_before_contacting_docker() {
    let mut action = action(&["true"], format!("sha256:{}", "a".repeat(64)));
    if let Action::RunCommand { network, .. } = &mut action {
        *network = NetworkPolicy::Deny;
    }
    assert!(matches!(
        Executor::resolve(&action),
        Err(Error::UnsupportedExecutorRequirement {
            requirement: "network denial",
            ..
        })
    ));
    if let Action::RunCommand {
        network,
        stdout_path,
        ..
    } = &mut action
    {
        *network = NetworkPolicy::default();
        *stdout_path = Some(Box::new(WorkspacePath::try_from("stdout").unwrap()));
    }
    assert!(Executor::resolve(&action).is_err());
}

#[tokio::test]
async fn portable_file_operations_can_use_a_docker_command_lease() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(workspace.path().join("cache"));
    let pool = resources();
    let lease = pool
        .acquire(Executor::Docker, &ResourceRequest::default())
        .await;
    let action = Action::WriteFile {
        path: WorkspacePath::try_from("prepared/input").unwrap(),
        bytes: b"data".to_vec(),
        input_digest: None,
    };
    crate::run_uncached_admitted(&action, workspace.path(), &cache, false, None, lease)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(workspace.path().join("prepared/input")).unwrap(),
        b"data"
    );
}

pub(super) fn real_image() -> String {
    std::env::var("ONCE_DOCKER_TEST_IMAGE")
        .expect("set ONCE_DOCKER_TEST_IMAGE to a warmed immutable sha256 image ID")
}

#[tokio::test]
#[ignore = "requires Docker and ONCE_DOCKER_TEST_IMAGE, image must provide sleep, mkdir, tar and /bin/sh"]
async fn real_docker_public_execution_stages_only_inputs_and_installs_outputs() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(workspace.path().join("cache"));
    std::fs::write(workspace.path().join("input"), b"portable input").unwrap();
    std::fs::write(workspace.path().join("secret"), b"not declared").unwrap();
    std::fs::write(workspace.path().join("stale"), b"clean me").unwrap();
    std::fs::create_dir(workspace.path().join("compiler-cache")).unwrap();
    std::fs::write(
        workspace.path().join("compiler-cache/dirty"),
        b"do not stage",
    )
    .unwrap();
    let mut action = action(
        &[
            "/bin/sh",
            "-c",
            "test ! -e /workspace/secret && test ! -e /workspace/stale && test -d /workspace/compiler-cache && test ! -e /workspace/compiler-cache/dirty && test ! -S /var/run/docker.sock && test \"$ROOT\" = /workspace && test \"$PWD\" = /workspace/nested/cwd && cat /workspace/input > /workspace/reports/result && ln /workspace/reports/result /workspace/reports/hardlink && printf captured && printf diagnostic >&2",
        ],
        real_image(),
    );
    if let Action::RunCommand {
        inputs,
        outputs,
        env,
        cwd,
        ..
    } = &mut action
    {
        inputs.push(WorkspacePath::try_from("input").unwrap());
        inputs.push(WorkspacePath::try_from("stale").unwrap());
        outputs.push(WorkspacePath::try_from("reports/result").unwrap());
        outputs.push(WorkspacePath::try_from("reports/hardlink").unwrap());
        env.insert("ROOT".into(), "{{once.execution_root}}".into());
        *cwd = Some(WorkspacePath::try_from("nested/cwd").unwrap());
    }
    let pool = resources();
    let lease = pool
        .acquire(Executor::Docker, action.resource_request())
        .await;
    let result = crate::Runner::run_uncached_admitted_with_setup(
        &action,
        workspace.path(),
        &cache,
        false,
        None,
        lease,
        &[WorkspacePath::try_from("stale").unwrap()],
        &[WorkspacePath::try_from("compiler-cache").unwrap()],
        &[],
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(
        std::fs::read(workspace.path().join("reports/result")).unwrap(),
        b"portable input"
    );
    assert_eq!(
        std::fs::read(workspace.path().join("reports/hardlink")).unwrap(),
        b"portable input"
    );
    assert_eq!(
        cache
            .get_blob(result.stdout.as_ref().unwrap())
            .await
            .unwrap(),
        b"captured"
    );
    assert_eq!(
        cache
            .get_blob(result.stderr.as_ref().unwrap())
            .await
            .unwrap(),
        b"diagnostic"
    );
    assert_no_journal_records(workspace.path());
}

#[tokio::test]
#[ignore = "requires Docker and ONCE_DOCKER_TEST_IMAGE"]
async fn real_docker_failures_preserve_host_outputs_and_clean_up() {
    for script in [
        "printf replaced > /workspace/result; exit 7",
        "exit 0",
        "ln -s /etc/passwd /workspace/result",
    ] {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::open_local(workspace.path().join("cache"));
        std::fs::write(workspace.path().join("result"), b"original").unwrap();
        let mut action = action(&["/bin/sh", "-c", script], real_image());
        if let Action::RunCommand { outputs, .. } = &mut action {
            outputs.push(WorkspacePath::try_from("result").unwrap());
        }
        let pool = resources();
        let lease = pool
            .acquire(Executor::Docker, action.resource_request())
            .await;
        let result =
            crate::run_uncached_admitted(&action, workspace.path(), &cache, false, None, lease)
                .await;
        if script.contains("exit 7") {
            assert_eq!(result.unwrap().exit_code, 7);
        } else {
            assert!(result.is_err());
        }
        assert_eq!(
            std::fs::read(workspace.path().join("result")).unwrap(),
            b"original"
        );
        assert_no_journal_records(workspace.path());
    }
}

pub(super) fn assert_no_journal_records(workspace: &Path) {
    assert!(Journal::new(workspace).scan(100).unwrap().is_empty());
}
