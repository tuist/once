use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use struo_cas::{ActionResult, CacheProvider, Cas};
use struo_core::{
    Action, CacheState, Error, Executor, ExecutorResources, NetworkPolicy, OutputSymlinkMode,
    RemoteExecution, ResourceLimits, ResourcePool, ResourceRequest, RunOpts, Runner, SandboxMode,
    WorkspacePath,
};
use tempfile::TempDir;

fn command(remote: Option<RemoteExecution>) -> Action {
    Action::RunCommand {
        argv: vec!["/bin/sh".into(), "-c".into(), "printf executed".into()],
        env: BTreeMap::new(),
        cwd: None,
        input_digest: None,
        inputs: vec![],
        outputs: vec![],
        stdout_path: None,
        stderr_path: None,
        output_symlink_mode: OutputSymlinkMode::default(),
        resources: ResourceRequest::default(),
        sandbox: SandboxMode::Off,
        network: NetworkPolicy::Unrestricted,
        timeout_ms: Some(5_000),
        success_exit_codes: vec![0],
        remote: remote.map(Box::new),
    }
}

fn setup() -> (TempDir, CacheProvider, ExecutorResources, Runner) {
    let workspace = TempDir::new().unwrap();
    let cache = CacheProvider::Local(Cas::open(workspace.path().join("cas")));
    let resources = ExecutorResources::new(Arc::new(ResourcePool::new(
        ResourceLimits::new(1, 1_000).with_default_action_memory_bytes(100),
    )))
    .with_remote_concurrency(1);
    let runner = Runner::with_cache(cache.clone(), workspace.path(), RunOpts::default())
        .with_executor_resources(resources.clone());
    (workspace, cache, resources, runner)
}

async fn seed_cache(cache: &CacheProvider, action: &Action) {
    let stdout = cache.put_blob(b"cached").await.unwrap();
    cache
        .put_action_result(
            &action.digest(),
            &ActionResult {
                exit_code: 0,
                stdout: Some(stdout),
                stderr: None,
                outputs: BTreeMap::new(),
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn cached_remote_results_do_not_wait_for_remote_capacity() {
    let (_workspace, cache, resources, runner) = setup();
    let action = command(Some(RemoteExecution::provider("e2b")));
    seed_cache(&cache, &action).await;
    let _held = resources
        .acquire(Executor::E2b, &ResourceRequest::default())
        .await;
    for streaming in [false, true] {
        let outcome = tokio::time::timeout(Duration::from_secs(1), async {
            if streaming {
                runner.run_streaming(&action).await
            } else {
                runner.run(&action).await
            }
        })
        .await
        .expect("cache hit waited for a remote slot")
        .unwrap();
        assert_eq!(outcome.cache, CacheState::Hit);
        assert_eq!(outcome.action, action.digest());
    }
}

#[tokio::test]
async fn cache_probes_without_outputs_do_not_reserve_host_action_memory() {
    let (_workspace, cache, resources, runner) = setup();
    let action = command(Some(RemoteExecution::provider("e2b")));
    seed_cache(&cache, &action).await;
    let _held = resources
        .host()
        .acquire(ResourceRequest::new(1, 1_000))
        .await;
    let hit = tokio::time::timeout(Duration::from_secs(1), runner.run(&action))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(hit.cache, CacheState::Hit);
}

#[tokio::test]
async fn fresh_execution_and_reproducibility_do_not_bypass_admission() {
    let (_workspace, _cache, resources, runner) = setup();
    let action = command(Some(RemoteExecution::provider("e2b")));
    let _held = resources
        .acquire(Executor::E2b, &ResourceRequest::default())
        .await;
    assert!(
        tokio::time::timeout(Duration::from_millis(20), runner.run_fresh(&action, false))
            .await
            .is_err()
    );
    assert!(tokio::time::timeout(
        Duration::from_millis(20),
        runner.verify_reproducible(&action)
    )
    .await
    .is_err());
}

#[tokio::test]
async fn host_budget_builders_preserve_explicit_remote_settings() {
    let (_workspace, _cache, resources, runner) = setup();
    let resources = resources
        .with_remote_concurrency(2)
        .with_client_memory_bytes(10);
    let runner = runner
        .with_executor_resources(resources.clone())
        .with_resource_limits(ResourceLimits::new(1, 100).with_default_action_memory_bytes(100))
        .with_max_concurrency(2);
    let first = resources
        .acquire(Executor::E2b, &ResourceRequest::default())
        .await;
    let second = resources
        .acquire(Executor::E2b, &ResourceRequest::default())
        .await;
    let action = command(Some(RemoteExecution::provider("e2b")));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), runner.run_fresh(&action, false))
            .await
            .is_err()
    );
    drop((first, second));
}

#[tokio::test]
async fn explicit_remote_symlink_policy_is_rejected() {
    let (_workspace, _cache, _resources, runner) = setup();
    let mut action = command(Some(RemoteExecution::provider("e2b")));
    let Action::RunCommand {
        output_symlink_mode,
        ..
    } = &mut action
    else {
        unreachable!()
    };
    *output_symlink_mode = OutputSymlinkMode::Preserve;
    assert!(matches!(
        runner.run(&action).await,
        Err(Error::UnsupportedExecutorRequirement { .. })
    ));
}

#[tokio::test]
async fn unknown_adapters_are_rejected_even_with_cached_results() {
    let (_workspace, cache, _resources, runner) = setup();
    let action = command(Some(RemoteExecution::provider("unknown")));
    seed_cache(&cache, &action).await;
    assert!(matches!(
        runner.run(&action).await,
        Err(Error::UnsupportedRemoteProvider { .. })
    ));
}

#[tokio::test]
async fn unsupported_requirements_fail_before_admission_or_provider_access() {
    let (_workspace, _cache, resources, runner) = setup();
    let _held = resources
        .host()
        .acquire(ResourceRequest::new(1, 1_000))
        .await;
    for adapter in ["microsandbox", "e2b", "daytona"] {
        for redirect in [false, true] {
            let mut action = command(Some(RemoteExecution::provider(adapter)));
            let Action::RunCommand {
                network,
                stdout_path,
                ..
            } = &mut action
            else {
                unreachable!()
            };
            if redirect {
                *stdout_path = Some(Box::new(WorkspacePath::try_from("out/log").unwrap()));
            } else {
                *network = NetworkPolicy::Deny;
            }
            for streaming in [false, true] {
                let result = tokio::time::timeout(Duration::from_secs(1), async {
                    if streaming {
                        runner.run_streaming(&action).await
                    } else {
                        runner.run(&action).await
                    }
                })
                .await
                .expect("unsupported requirements queued on admission");
                assert!(matches!(
                    result,
                    Err(Error::UnsupportedExecutorRequirement { .. })
                ));
            }
        }
    }
}

#[tokio::test]
async fn cached_output_restoration_respects_the_host_memory_budget() {
    let (workspace, _cache, resources, runner) = setup();
    let action = Action::WriteFile {
        path: WorkspacePath::try_from("out/value").unwrap(),
        bytes: b"value".to_vec(),
        input_digest: None,
    };
    let first = runner.run(&action).await.unwrap();
    std::fs::remove_file(workspace.path().join("out/value")).unwrap();
    let held = resources
        .host()
        .acquire(ResourceRequest::new(1, 1_000))
        .await;
    let waiting_runner = runner.clone();
    let waiting = tokio::spawn(async move { waiting_runner.run(&action).await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!waiting.is_finished());
    assert!(!workspace.path().join("out/value").exists());
    drop(held);
    let hit = tokio::time::timeout(Duration::from_secs(1), waiting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(hit.action, first.action);
    assert_eq!(hit.cache, CacheState::Hit);
    assert_eq!(
        std::fs::read(workspace.path().join("out/value")).unwrap(),
        b"value"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn explicit_local_binding_uses_native_sandbox_and_redirects() {
    let (workspace, cache, _resources, runner) = setup();
    std::fs::write(workspace.path().join("declared"), b"input").unwrap();
    std::fs::write(workspace.path().join("ambient"), b"hidden").unwrap();
    for streaming in [false, true] {
        let mut binding = RemoteExecution::provider("local-alias");
        binding.executor = Some("local".to_string());
        let mut action = command(Some(binding));
        let Action::RunCommand {
            argv,
            inputs,
            outputs,
            stdout_path,
            sandbox,
            ..
        } = &mut action
        else {
            unreachable!()
        };
        *argv = vec![
            "/bin/sh".into(),
            "-c".into(),
            "test ! -e ambient && cat declared".into(),
        ];
        *inputs = vec![WorkspacePath::try_from("declared").unwrap()];
        let output = WorkspacePath::try_from(if streaming {
            "out/stream"
        } else {
            "out/capture"
        })
        .unwrap();
        *outputs = vec![output.clone()];
        *stdout_path = Some(Box::new(output.clone()));
        *sandbox = SandboxMode::CopiedInputs;
        let outcome = if streaming {
            runner.run_streaming(&action).await
        } else {
            runner.run(&action).await
        }
        .unwrap();
        assert_eq!(outcome.result.exit_code, 0);
        assert_eq!(outcome.action, action.digest());
        assert_eq!(
            std::fs::read(output.resolve(workspace.path())).unwrap(),
            b"input"
        );
        assert!(outcome.result.stdout.is_none());
        assert!(!outcome.result.outputs.is_empty());
        assert!(cache
            .get_action_result(&action.digest())
            .await
            .unwrap()
            .is_some());
    }
}

#[tokio::test]
async fn local_binding_cannot_silently_ignore_an_execution_image() {
    let (_workspace, _cache, _resources, runner) = setup();
    let mut binding = RemoteExecution::provider("local");
    binding.environment = Some("requested-image".to_string());
    assert!(matches!(
        runner.run(&command(Some(binding))).await,
        Err(Error::RemoteProviderConfig { .. })
    ));
}
