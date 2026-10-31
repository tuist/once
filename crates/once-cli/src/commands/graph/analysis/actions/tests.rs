use std::collections::BTreeMap;

use super::*;

#[test]
fn capability_outputs_are_sorted_and_deduplicated() {
    let mut outputs = vec![
        ".once/out/Root/run".to_string(),
        ".once/out/Root/run/stdout.log".to_string(),
        ".once/out/Root/run".to_string(),
    ];

    deduplicate_outputs(&mut outputs);

    assert_eq!(
        outputs,
        [".once/out/Root/run", ".once/out/Root/run/stdout.log"]
    );
}

#[test]
fn tool_execution_wraps_command_actions_only() {
    let command: DeclaredAction = serde_json::from_value(serde_json::json!({
        "argv": ["rustc", "--version"],
        "outputs": []
    }))
    .unwrap();
    let portable: DeclaredAction = serde_json::from_value(serde_json::json!({
        "operation": {
            "kind": "write_file",
            "path": "out.txt",
            "bytes": [111, 107]
        },
        "outputs": ["out.txt"]
    }))
    .unwrap();
    let mut actions = vec![command, portable];

    apply_tool_execution(
        &[
            "/once/mise".to_string(),
            "exec".to_string(),
            "--".to_string(),
        ],
        &BTreeMap::from([
            ("MISE_ENABLE_TOOLS".to_string(), "rust".to_string()),
            ("PATH".to_string(), "/opt/rust/bin:/usr/bin".to_string()),
        ]),
        &mut actions,
    );

    assert_eq!(
        actions[0].argv,
        ["/once/mise", "exec", "--", "rustc", "--version"]
    );
    assert_eq!(
        actions[0].env.get("MISE_ENABLE_TOOLS").map(String::as_str),
        Some("rust")
    );
    assert_eq!(
        actions[0].env.get("PATH").map(String::as_str),
        Some("/opt/rust/bin:/usr/bin")
    );
    assert!(actions[1].argv.is_empty());
    assert!(actions[1].env.is_empty());
}

#[test]
fn tool_execution_merges_action_path_ahead_of_tool_path() {
    let command: DeclaredAction = serde_json::from_value(serde_json::json!({
        "argv": ["rustc", "--version"],
        "env": {"PATH": "/target/runtime:/opt/rust/bin"},
        "outputs": []
    }))
    .unwrap();
    let mut actions = vec![command];

    apply_tool_execution(
        &[
            "/once/mise".to_string(),
            "exec".to_string(),
            "--".to_string(),
        ],
        &BTreeMap::from([("PATH".to_string(), "/opt/rust/bin:/usr/bin".to_string())]),
        &mut actions,
    );

    // The action's own directory survives, the shared entry is not
    // duplicated, and the mise directory is appended.
    assert_eq!(
        actions[0].env.get("PATH").map(String::as_str),
        Some("/target/runtime:/opt/rust/bin:/usr/bin")
    );
}

#[test]
fn host_executable_fallback_resolves_the_relevant_path_entry() {
    let directory = tempfile::tempdir().unwrap();
    let filename = if cfg!(windows) {
        "fallback-tool.exe"
    } else {
        "fallback-tool"
    };
    let executable = directory.path().join(filename);
    std::fs::write(&executable, b"tool").unwrap();
    let paths = env::join_paths([directory.path()]).unwrap();

    assert_eq!(
        host_executable_path_in("fallback-tool", &paths, Some(OsStr::new(".COM;.EXE;.CMD"))),
        Some(executable)
    );
}

#[cfg(unix)]
#[test]
fn host_executable_fallback_skips_mise_shims() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let directory = tempfile::tempdir().unwrap();
    let shims = directory.path().join("shims");
    let tools = directory.path().join("tools");
    std::fs::create_dir(&shims).unwrap();
    std::fs::create_dir(&tools).unwrap();
    let mise = directory.path().join("mise");
    let tool = tools.join("probe");
    for (path, script) in [
        (&mise, b"#!/bin/sh\nprintf shim".as_slice()),
        (&tool, b"#!/bin/sh\nprintf real-tool".as_slice()),
    ] {
        std::fs::write(path, script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    symlink(&mise, shims.join("probe")).unwrap();
    let paths = env::join_paths([shims, tools]).unwrap();
    let resolved = host_executable_path_in("probe", &paths, None).unwrap();
    let output = std::process::Command::new(resolved).output().unwrap();

    assert!(output.status.success());
    assert_eq!(output.stdout, b"real-tool");
}

fn module_digest() -> Digest {
    Digest::of_bytes(b"modules")
}

fn test_resources() -> &'static Arc<ResourcePool> {
    static RESOURCES: std::sync::LazyLock<Arc<ResourcePool>> = std::sync::LazyLock::new(|| {
        Arc::new(ResourcePool::new(once_core::ResourceLimits::new(256, 0)))
    });
    &RESOURCES
}

fn test_workers() -> &'static crate::bus_events::WorkerSlots {
    static WORKERS: std::sync::LazyLock<crate::bus_events::WorkerSlots> =
        std::sync::LazyLock::new(|| crate::bus_events::WorkerSlots::new(256));
    &WORKERS
}

#[test]
fn missing_declared_inputs_reports_only_absent_paths() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("present.rs"), b"fn main() {}").unwrap();
    let declared = DeclaredAction {
        resources: ResourceRequest::default(),
        history: None,
        presentation: None,
        operation: None,
        argv: vec!["tool".to_string()],
        arg_files: Vec::new(),
        inputs: vec![
            "present.rs".to_string(),
            ".once/out/x/generated.rlib".to_string(),
        ],
        outputs: Vec::new(),
        stdout: None,
        stderr: None,
        clean_paths: Vec::new(),
        create_dirs: Vec::new(),
        cwd: None,
        env: BTreeMap::new(),
        sandbox: None,
        network: None,
        success_exit_codes: vec![0],
        cacheable: true,
        inherit_parent_env: false,
        depends_on_prior_actions: true,
        intermediate: false,
        toolchain_identity: None,
        identifier: None,
        display_name: None,
        source_files: Vec::new(),
    };

    assert_eq!(
        missing_declared_inputs(workspace.path(), &declared),
        vec![".once/out/x/generated.rlib".to_string()]
    );
}

#[tokio::test]
async fn same_target_outputs_materialize_before_a_later_miss() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let blob = cache.put_blob(b"generated").await.unwrap();
    let path = ".once/tmp/analysis/generated.txt";
    let result = ActionResult {
        exit_code: 0,
        stdout: None,
        stderr: None,
        outputs: BTreeMap::from([(path.to_string(), blob)]),
    };

    let resources = once_core::ExecutorResources::new(Arc::new(ResourcePool::new(
        once_core::ResourceLimits::default(),
    )));
    restore_outputs(
        workspace.path(),
        &cache,
        &result,
        None,
        resources.acquire_materialization().await,
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read(workspace.path().join(path)).unwrap(),
        b"generated"
    );
}

#[test]
fn declared_action_uses_direct_argv_and_creates_output_parents() {
    let workspace = tempfile::tempdir().unwrap();
    let declared = DeclaredAction {
        resources: ResourceRequest::default(),
        operation: None,
        history: None,
        presentation: None,
        argv: vec!["tool".to_string(), "--version".to_string()],
        arg_files: Vec::new(),
        inputs: Vec::new(),
        outputs: vec![
            ".once/out/x/A.out".to_string(),
            ".once/out/x/sub/B.meta".to_string(),
        ],
        stdout: None,
        stderr: None,
        clean_paths: Vec::new(),
        create_dirs: vec![".once/tmp/work".to_string()],
        cwd: None,
        env: BTreeMap::new(),
        sandbox: None,
        network: None,
        success_exit_codes: vec![0],
        cacheable: true,
        inherit_parent_env: false,
        depends_on_prior_actions: true,
        intermediate: false,
        toolchain_identity: None,
        identifier: None,
        display_name: None,
        source_files: Vec::new(),
    };

    let action = declared_to_action(
        workspace.path(),
        &declared,
        module_digest(),
        &[],
        SandboxMode::default(),
    )
    .unwrap();

    assert!(workspace.path().join(".once/out/x").is_dir());
    assert!(workspace.path().join(".once/out/x/sub").is_dir());
    let Action::RunCommand { argv, inputs, .. } = action else {
        panic!("command declaration should lower to RunCommand");
    };
    assert_eq!(argv, vec!["tool".to_string(), "--version".to_string()]);
    assert!(inputs
        .iter()
        .any(|input| input.as_str() == ".once/tmp/work"));
}

#[test]
fn declared_action_replaces_stale_file_that_became_an_output_directory() {
    let workspace = tempfile::tempdir().unwrap();
    let stale = workspace
        .path()
        .join(".once/out/Framework/Modules/Feature.swiftmodule");
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, b"old flat module").unwrap();
    let output = WorkspacePath::try_from(
        ".once/out/Framework/Modules/Feature.swiftmodule/arm64-apple-ios-simulator.swiftmodule",
    )
    .unwrap();

    ensure_output_parent_dirs(workspace.path(), &[output]).unwrap();

    assert!(stale.is_dir());
}

#[test]
fn concurrent_actions_can_create_shared_output_parents() {
    let workspace = tempfile::tempdir().unwrap();
    let workspace = workspace.path().to_path_buf();
    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        let mut tasks = Vec::new();
        for index in 0..8 {
            let workspace = &workspace;
            let barrier = &barrier;
            tasks.push(scope.spawn(move || {
                let output = WorkspacePath::try_from(format!(
                    ".once/out/tests/test/batches/{index}/test_results.json"
                ))
                .unwrap();
                barrier.wait();
                ensure_output_parent_dirs(workspace, &[output])
            }));
        }
        for task in tasks {
            task.join().unwrap().unwrap();
        }
    });
}

#[tokio::test]
async fn declared_command_setup_cleans_paths_and_creates_dirs() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join(".once/out/tree/sub")).unwrap();
    std::fs::write(
        workspace.path().join(".once/out/tree/sub/stale.txt"),
        b"stale",
    )
    .unwrap();
    std::fs::write(workspace.path().join(".once/out/stale-file.txt"), b"stale").unwrap();
    let declared = DeclaredAction {
        resources: ResourceRequest::default(),
        operation: None,
        argv: vec!["tool".to_string()],
        arg_files: Vec::new(),
        inputs: Vec::new(),
        history: None,
        presentation: None,
        outputs: vec![".once/out/tree".to_string()],
        stdout: None,
        stderr: None,
        clean_paths: vec![
            ".once/out/tree".to_string(),
            ".once/out/stale-file.txt".to_string(),
        ],
        create_dirs: vec![".once/out/tree".to_string(), ".once/tmp/home".to_string()],
        cwd: None,
        env: BTreeMap::new(),
        sandbox: None,
        network: None,
        success_exit_codes: vec![0],
        cacheable: true,
        inherit_parent_env: false,
        depends_on_prior_actions: true,
        intermediate: false,
        toolchain_identity: None,
        identifier: None,
        display_name: None,
        source_files: Vec::new(),
    };

    prepare_declared_command_paths(workspace.path(), &declared)
        .await
        .unwrap();

    assert!(workspace.path().join(".once/out/tree").is_dir());
    assert!(!workspace.path().join(".once/out/tree/sub").exists());
    assert!(!workspace.path().join(".once/out/stale-file.txt").exists());
    assert!(workspace.path().join(".once/tmp/home").is_dir());
}

#[test]
fn materialize_declared_arg_files_writes_line_delimited_args() {
    let workspace = tempfile::tempdir().unwrap();
    let arg_files = vec![DeclaredArgFile {
        path: ".once/out/tool/args.txt".to_string(),
        format: DeclaredArgFileFormat::LineDelimited,
        args: vec!["--flag".to_string(), "value with spaces".to_string()],
    }];

    materialize_declared_arg_files(workspace.path(), &arg_files).unwrap();

    assert_eq!(
        std::fs::read_to_string(workspace.path().join(".once/out/tool/args.txt")).unwrap(),
        "--flag\nvalue with spaces\n"
    );
}

#[test]
fn materialize_declared_arg_files_rejects_newline_args() {
    let workspace = tempfile::tempdir().unwrap();
    let arg_files = vec![DeclaredArgFile {
        path: ".once/out/tool/args.txt".to_string(),
        format: DeclaredArgFileFormat::LineDelimited,
        args: vec!["value\n--flag".to_string()],
    }];

    let err = materialize_declared_arg_files(workspace.path(), &arg_files)
        .unwrap_err()
        .to_string();

    assert!(err.contains("contains an argument with a newline"), "{err}");
}

#[tokio::test]
async fn declared_action_failure_message_appends_captured_output() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let stdout = cache.put_blob(b"visible stdout").await.unwrap();
    let stderr = cache.put_blob(b"visible stderr").await.unwrap();
    let result = ActionResult {
        exit_code: 7,
        stdout: Some(stdout),
        stderr: Some(stderr),
        outputs: BTreeMap::new(),
    };

    let message = declared_action_failure_message(DeclaredActionFailure {
        cache: &cache,
        identifier: "target:action",
        index: 2,
        target: "target",
        exit_code: 7,
        argv: &[],
        arg_files: &[],
        result: &result,
    })
    .await;

    assert!(message.contains("target:action (2) failed for target with exit code 7"));
    assert!(message.contains("stdout:\nvisible stdout"));
    assert!(message.contains("stderr:\nvisible stderr"));
}

#[tokio::test]
async fn declared_action_failure_message_truncates_large_output() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let mut bytes = b"drop-me".to_vec();
    bytes.extend(std::iter::repeat_n(b'x', FAILURE_OUTPUT_LIMIT));
    let stdout = cache.put_blob(&bytes).await.unwrap();
    let result = ActionResult {
        exit_code: 1,
        stdout: Some(stdout),
        stderr: None,
        outputs: BTreeMap::new(),
    };

    let message = declared_action_failure_message(DeclaredActionFailure {
        cache: &cache,
        identifier: "id",
        index: 0,
        target: "target",
        exit_code: 1,
        argv: &[],
        arg_files: &[],
        result: &result,
    })
    .await;

    assert!(message.contains("last 16384 bytes of stdout:\n"));
    assert!(!message.contains("drop-me"));
    assert!(message.ends_with(&"x".repeat(FAILURE_OUTPUT_LIMIT)));
}

#[tokio::test]
async fn declared_action_failure_message_appends_arg_file_context() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let result = ActionResult {
        exit_code: 1,
        stdout: None,
        stderr: None,
        outputs: BTreeMap::new(),
    };
    let mut response_args = vec![
        "--name".to_string(),
        "app".to_string(),
        "--input".to_string(),
        ".once/out/deps/dep.bin".to_string(),
        "--search".to_string(),
        ".once/out/deps".to_string(),
    ];
    response_args.extend((0..60).map(|index| format!("arg-{index}")));
    response_args.push("src/input.txt".to_string());
    let arg_files = vec![DeclaredArgFile {
        path: ".once/tmp/analysis/app/tool.args".to_string(),
        format: DeclaredArgFileFormat::LineDelimited,
        args: response_args,
    }];

    let command_argv = vec![
        "tool".to_string(),
        "--config".to_string(),
        ".once/out/deps/config.json".to_string(),
        "@.once/tmp/analysis/app/tool.args".to_string(),
    ];
    let message = declared_action_failure_message(DeclaredActionFailure {
        cache: &cache,
        identifier: "id",
        index: 0,
        target: "target",
        exit_code: 1,
        argv: &command_argv,
        arg_files: &arg_files,
        result: &result,
    })
    .await;

    assert!(message.contains("argv:"));
    assert!(message.contains("first args:\n  tool"));
    assert!(message.contains(".once/out/deps/config.json"));
    assert!(message.contains("arg files:"));
    assert!(message.contains(".once/tmp/analysis/app/tool.args [line-delimited]"));
    assert!(message.contains("first args:\n  --name"));
    assert!(message.contains("last args:"));
    assert!(message.contains("src/input.txt"));
    assert!(!message.contains("extern args:"));
    assert!(!message.contains("dependency search dirs:"));
}

#[tokio::test]
async fn append_captured_output_ignores_missing_digest_and_missing_blob() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let missing = Digest::of_bytes(b"missing");
    let mut message = "base".to_string();

    append_captured_output(&cache, &mut message, "stdout", None).await;
    append_captured_output(&cache, &mut message, "stdout", Some(&missing)).await;

    assert_eq!(message, "base");
}

#[cfg(unix)]
#[tokio::test]
async fn uncached_exit_policies_reject_zero_in_every_local_execution_path() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    for sandbox in [
        SandboxMode::Off,
        SandboxMode::Inputs,
        SandboxMode::CopiedInputs,
    ] {
        for exit_code in [0, 7] {
            let action = Action::RunCommand {
                argv: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    format!("printf policy-detail >&2; exit {exit_code}"),
                ],
                env: BTreeMap::new(),
                cwd: None,
                input_digest: None,
                inputs: vec![],
                outputs: vec![],
                stdout_path: None,
                stderr_path: None,
                output_symlink_mode: OutputSymlinkMode::default(),
                resources: ResourceRequest::default(),
                sandbox,
                network: NetworkPolicy::default(),
                timeout_ms: Some(5_000),
                success_exit_codes: vec![7],
                remote: None,
            };
            let result = run_uncached_action(&action, workspace.path(), &cache, false).await;
            if exit_code == 0 {
                let error = result.unwrap_err();
                assert!(format!("{error:#}").contains("policy-detail"));
                assert!(matches!(
                    error.downcast_ref::<once_core::Error>(),
                    Some(once_core::Error::RejectedZeroExitCode { .. })
                ));
            } else {
                assert_eq!(result.unwrap().exit_code, 0);
            }
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn uncached_action_executes_each_time() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let action = Action::RunCommand {
        argv: vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf x >> counter".to_string(),
        ],
        env: BTreeMap::new(),
        cwd: None,
        input_digest: None,
        inputs: vec![],
        outputs: Vec::new(),
        stdout_path: None,
        stderr_path: None,
        output_symlink_mode: OutputSymlinkMode::default(),
        resources: ResourceRequest::default(),
        sandbox: SandboxMode::default(),
        network: NetworkPolicy::default(),
        timeout_ms: None,
        success_exit_codes: vec![0],
        remote: None,
    };

    run_uncached_action(&action, workspace.path(), &cache, false)
        .await
        .unwrap();
    run_uncached_action(&action, workspace.path(), &cache, false)
        .await
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(workspace.path().join("counter")).unwrap(),
        "xx"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn uncached_action_can_inherit_parent_environment() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let output = WorkspacePath::try_from(".once/out/home.txt").unwrap();
    let action = Action::RunCommand {
        argv: vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf %s \"$HOME\" > .once/out/home.txt".to_string(),
        ],
        env: BTreeMap::new(),
        cwd: None,
        input_digest: None,
        inputs: vec![],
        outputs: vec![output],
        stdout_path: None,
        stderr_path: None,
        output_symlink_mode: OutputSymlinkMode::default(),
        resources: ResourceRequest::default(),
        sandbox: SandboxMode::default(),
        network: NetworkPolicy::default(),
        timeout_ms: None,
        success_exit_codes: vec![0],
        remote: None,
    };
    std::fs::create_dir_all(workspace.path().join(".once/out")).unwrap();

    run_uncached_action(&action, workspace.path(), &cache, true)
        .await
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(workspace.path().join(".once/out/home.txt")).unwrap(),
        std::env::var("HOME").unwrap()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn uncached_action_succeeds_when_declared_output_exists() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let action = Action::RunCommand {
        argv: vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf ok > .once/out/result.txt".to_string(),
        ],
        env: BTreeMap::new(),
        cwd: None,
        input_digest: None,
        inputs: vec![],
        outputs: vec![WorkspacePath::try_from(".once/out/result.txt").unwrap()],
        stdout_path: None,
        stderr_path: None,
        output_symlink_mode: OutputSymlinkMode::default(),
        resources: ResourceRequest::default(),
        sandbox: SandboxMode::default(),
        network: NetworkPolicy::default(),
        timeout_ms: None,
        success_exit_codes: vec![0],
        remote: None,
    };
    std::fs::create_dir_all(workspace.path().join(".once/out")).unwrap();

    let result = run_uncached_action(&action, workspace.path(), &cache, false)
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0);
    assert!(result.outputs.contains_key(".once/out/result.txt"));
    assert_eq!(
        std::fs::read_to_string(workspace.path().join(".once/out/result.txt")).unwrap(),
        "ok"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn uncached_action_discards_stdout_without_buffering_it() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let action = Action::RunCommand {
        argv: vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf noisy-stdout".to_string(),
        ],
        env: BTreeMap::new(),
        cwd: None,
        input_digest: None,
        inputs: vec![],
        outputs: Vec::new(),
        stdout_path: None,
        stderr_path: None,
        output_symlink_mode: OutputSymlinkMode::default(),
        resources: ResourceRequest::default(),
        sandbox: SandboxMode::default(),
        network: NetworkPolicy::default(),
        timeout_ms: None,
        success_exit_codes: vec![0],
        remote: None,
    };

    let result = run_uncached_action(&action, workspace.path(), &cache, false)
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, None);
}

#[cfg(unix)]
#[tokio::test]
async fn uncached_action_streams_large_stderr_into_the_cache() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let action = Action::RunCommand {
        argv: vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "dd if=/dev/zero bs=1048576 count=8 1>&2 2>/dev/null".to_string(),
        ],
        env: BTreeMap::new(),
        cwd: None,
        input_digest: None,
        inputs: vec![],
        outputs: Vec::new(),
        stdout_path: None,
        stderr_path: None,
        output_symlink_mode: OutputSymlinkMode::default(),
        resources: ResourceRequest::default(),
        sandbox: SandboxMode::default(),
        network: NetworkPolicy::default(),
        timeout_ms: Some(30_000),
        success_exit_codes: vec![0],
        remote: None,
    };

    let result = run_uncached_action(&action, workspace.path(), &cache, false)
        .await
        .unwrap();
    let stderr = cache
        .get_blob(result.stderr.as_ref().expect("stderr digest"))
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0);
    assert_eq!(stderr.len(), 8 * 1024 * 1024);
}

#[cfg(unix)]
#[tokio::test]
async fn uncached_action_redirects_merged_streams_to_declared_file() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let log = WorkspacePath::try_from(".once/out/run/log.txt").unwrap();
    let action = Action::RunCommand {
        argv: vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf out; printf err >&2".to_string(),
        ],
        env: BTreeMap::new(),
        cwd: None,
        input_digest: None,
        inputs: vec![],
        outputs: vec![log.clone()],
        stdout_path: Some(Box::new(log.clone())),
        stderr_path: Some(Box::new(log)),
        output_symlink_mode: OutputSymlinkMode::default(),
        resources: ResourceRequest::default(),
        sandbox: SandboxMode::default(),
        network: NetworkPolicy::default(),
        timeout_ms: None,
        success_exit_codes: vec![0],
        remote: None,
    };

    let result = run_uncached_action(&action, workspace.path(), &cache, false)
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0);
    // Both streams landed in the declared file, not in the result.
    assert_eq!(result.stdout, None);
    assert_eq!(result.stderr, None);
    assert!(result.outputs.contains_key(".once/out/run/log.txt"));
    let on_disk = std::fs::read_to_string(workspace.path().join(".once/out/run/log.txt")).unwrap();
    assert!(on_disk.contains("out"), "log missing stdout: {on_disk:?}");
    assert!(on_disk.contains("err"), "log missing stderr: {on_disk:?}");
}

#[tokio::test]
async fn capture_uncached_outputs_records_directory_tree_manifest() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    std::fs::create_dir_all(workspace.path().join(".once/out/tree/sub")).unwrap();
    std::fs::write(workspace.path().join(".once/out/tree/a.txt"), b"a").unwrap();
    std::fs::write(workspace.path().join(".once/out/tree/sub/b.txt"), b"b").unwrap();

    let outputs = capture_uncached_outputs(
        &[WorkspacePath::try_from(".once/out/tree").unwrap()],
        workspace.path(),
        &cache,
    )
    .await
    .unwrap();
    let digest = outputs.get(".once/out/tree").unwrap();
    let manifest = cache.get_blob(digest).await.unwrap();
    let manifest = String::from_utf8(manifest).unwrap();

    assert!(manifest.starts_with("once.directory_output.v1\n"));
    assert!(manifest.contains("dir\tsub\n"));
    assert!(manifest.contains("file\ta.txt\t"));
    assert!(manifest.contains("file\tsub/b.txt\t"));
}

#[cfg(unix)]
#[tokio::test]
async fn capture_uncached_outputs_records_top_level_symlink_without_following_it() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    std::fs::create_dir_all(workspace.path().join(".once/out")).unwrap();
    std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();
    std::os::unix::fs::symlink(outside.path(), workspace.path().join(".once/out/link")).unwrap();

    let outputs = capture_uncached_outputs(
        &[WorkspacePath::try_from(".once/out/link").unwrap()],
        workspace.path(),
        &cache,
    )
    .await
    .unwrap();
    let digest = outputs.get(".once/out/link").unwrap();
    let manifest = cache.get_blob(digest).await.unwrap();
    let manifest = String::from_utf8(manifest).unwrap();

    assert!(manifest.starts_with("once.symlink_output.v1\n"));
    assert!(manifest.contains(&outside.path().to_string_lossy().to_string()));
    assert!(!manifest.contains("secret.txt"));
}

#[cfg(unix)]
#[tokio::test]
async fn single_action_outcome_preserves_streams_for_capability_evidence() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let target = GraphTarget {
        label: once_frontend::TargetLabel {
            package: "tools".to_string(),
            name: "single".to_string(),
            id: "tools/single".to_string(),
        },
        kind: "demo_kind".to_string(),
        deps: Vec::new(),
        dependency_edges: BTreeMap::new(),
        srcs: Vec::new(),
        visibility: Vec::new(),
        attrs: BTreeMap::new(),
        capabilities: Vec::new(),
        providers: Vec::new(),
        tools: Vec::new(),
        diagnostics: Vec::new(),
    };
    let analysis = AnalysisResult {
        actions: vec![DeclaredAction {
            resources: ResourceRequest::default(),
            operation: None,
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf visible-stdout; printf visible-stderr >&2; printf ok > .once/out/one.txt"
                    .to_string(),
            ],
            arg_files: Vec::new(),
            inputs: Vec::new(),
            outputs: vec![".once/out/one.txt".to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            network: None,
            success_exit_codes: vec![0],
            cacheable: true,
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            intermediate: false,
            toolchain_identity: None,
            identifier: Some("one".to_string()),
            history: None,
            presentation: None,
            display_name: None,
            source_files: Vec::new(),
        }],
        provider: serde_json::json!({}),
        declared_outputs: Vec::new(),
        observations: once_frontend::analysis::AnalysisObservations::default(),
    };

    let outcome = run_declared_actions(
        None,
        workspace.path(),
        &cache,
        module_digest(),
        &target,
        "build",
        analysis,
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        SandboxMode::default(),
        test_resources(),
        None,
        None,
        test_workers(),
    )
    .await
    .unwrap();

    let stdout = cache
        .get_blob(&outcome.result.stdout.unwrap())
        .await
        .unwrap();
    let stderr = cache
        .get_blob(&outcome.result.stderr.unwrap())
        .await
        .unwrap();
    assert_eq!(stdout, b"visible-stdout");
    assert_eq!(stderr, b"visible-stderr");
    assert!(outcome.result.outputs.contains_key(".once/out/one.txt"));
    let records = once_core::EvidenceStore::open_workspace(workspace.path())
        .load()
        .await
        .unwrap();
    assert_eq!(records.len(), 0);
}

#[test]
fn uncacheable_action_makes_whole_target_bypass() {
    use EvidenceCacheState::{Bypass, Hit, Miss};

    // A single uncacheable action forces the target to Bypass so a
    // target that must re-run work is never reported as a cache hit.
    assert_eq!(aggregate_declared_action_cache_state(Bypass, Hit), Bypass);
    assert_eq!(aggregate_declared_action_cache_state(Hit, Bypass), Bypass);
    assert_eq!(aggregate_declared_action_cache_state(Bypass, Miss), Bypass);
    assert_eq!(
        aggregate_declared_action_cache_state(Bypass, Bypass),
        Bypass
    );
    // Without a Bypass action, Miss dominates Hit.
    assert_eq!(aggregate_declared_action_cache_state(Hit, Miss), Miss);
    assert_eq!(aggregate_declared_action_cache_state(Hit, Hit), Hit);
}

fn cached_test_target() -> GraphTarget {
    GraphTarget {
        label: once_frontend::TargetLabel {
            package: "tools".to_string(),
            name: "cached".to_string(),
            id: "tools/cached".to_string(),
        },
        kind: "demo_kind".to_string(),
        deps: Vec::new(),
        dependency_edges: BTreeMap::new(),
        srcs: Vec::new(),
        visibility: Vec::new(),
        attrs: BTreeMap::new(),
        capabilities: Vec::new(),
        providers: Vec::new(),
        tools: Vec::new(),
        diagnostics: Vec::new(),
    }
}

#[tokio::test]
async fn deferred_actions_restore_cached_planning_inputs_before_expansion() {
    let workspace = tempfile::TempDir::new().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cache")));
    let engine = once_frontend::analysis::AnalysisEngine::from_source(r#"
def finish(ctx):
    content = host_file_read(workspace_root() + "/scan.json")
    write_path(ctx["outputs"][0], content)
def impl(ctx):
    write_path("scan.json", "planned result")
    expand_actions(implementation = "finish", inputs = ["scan.json"], outputs = ["result.txt"], args = {})
    return {}
demo_kind = {"_once_target_kind": True, "kind": "demo_kind", "impl": impl}
"#).unwrap();
    let target = cached_test_target();
    for expected in [EvidenceCacheState::Miss, EvidenceCacheState::Hit] {
        let analysis = engine
            .analyze_target(&target, workspace.path(), &[])
            .unwrap();
        let outcome = run_declared_actions(
            Some(&engine),
            workspace.path(),
            &cache,
            module_digest(),
            &target,
            "build",
            analysis,
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
            SandboxMode::default(),
            test_resources(),
            None,
            None,
            test_workers(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.cache_state, expected);
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("scan.json")).unwrap(),
            "planned result"
        );
        assert!(outcome.result.outputs.contains_key("result.txt"));
        std::fs::remove_file(workspace.path().join("scan.json")).unwrap();
    }
}

fn cached_test_analysis(dependency_path: &str) -> AnalysisResult {
    AnalysisResult {
        actions: vec![DeclaredAction {
            resources: ResourceRequest::default(),
            operation: None,
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf ok > .once/out/out.txt".to_string(),
            ],
            arg_files: Vec::new(),
            inputs: vec![dependency_path.to_string()],
            outputs: vec![".once/out/out.txt".to_string()],
            stdout: None,
            stderr: None,
            clean_paths: vec![".once/out/side.txt".to_string()],
            create_dirs: vec![".once/out".to_string()],
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            network: None,
            success_exit_codes: vec![0],
            cacheable: true,
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            intermediate: false,
            toolchain_identity: None,
            identifier: Some("cached".to_string()),
            history: None,
            presentation: None,
            display_name: None,
            source_files: Vec::new(),
        }],
        provider: serde_json::json!({}),
        declared_outputs: Vec::new(),
        observations: once_frontend::analysis::AnalysisObservations::default(),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn cache_hit_skips_command_setup_and_defers_output_materialization() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let source_digest_cache = SourceDigestCache::open(workspace.path());
    let dependency_path = ".once/out/dependency/input.txt";
    let dependency_blob = cache.put_blob(b"dependency").await.unwrap();
    let available_inputs = BTreeMap::from([(
        dependency_path.to_string(),
        AvailableInput {
            blob_digest: dependency_blob,
            same_target: false,
            materialized: false,
        },
    )]);
    let target = cached_test_target();
    let analysis = || cached_test_analysis(dependency_path);

    // First run misses the cache and executes the command.
    let first = run_declared_actions(
        None,
        workspace.path(),
        &cache,
        module_digest(),
        &target,
        "build",
        analysis(),
        &[],
        &available_inputs,
        &BTreeMap::new(),
        Some(&source_digest_cache),
        SandboxMode::default(),
        test_resources(),
        None,
        None,
        test_workers(),
    )
    .await
    .unwrap();
    assert_eq!(first.cache_tag, "miss");
    assert!(first.cached_results.is_empty());
    assert_eq!(
        first.available_inputs[dependency_path].blob_digest,
        dependency_blob
    );
    assert!(!first.available_inputs[dependency_path].same_target);
    assert!(
        first.available_inputs[".once/out/out.txt"].same_target,
        "the target's own output stays marked as same-target until a dependent consumes it"
    );
    let dependency = workspace.path().join(dependency_path);
    assert_eq!(std::fs::read(&dependency).unwrap(), b"dependency");
    std::fs::remove_file(&dependency).unwrap();

    // Stand in for content a prior uncached run left behind: it lives
    // under a clean path but is not one of the action's outputs, so a
    // cache hit would not restore it.
    let side = workspace.path().join(".once/out/side.txt");
    std::fs::write(&side, b"precious").unwrap();
    let output = workspace.path().join(".once/out/out.txt");
    std::fs::write(&output, b"stale").unwrap();

    // Second run hits the cache. The command never executes, so its
    // clean_paths must not delete the untracked file.
    let second = run_declared_actions(
        None,
        workspace.path(),
        &cache,
        module_digest(),
        &target,
        "build",
        analysis(),
        &[],
        &available_inputs,
        &BTreeMap::new(),
        Some(&source_digest_cache),
        SandboxMode::default(),
        test_resources(),
        None,
        None,
        test_workers(),
    )
    .await
    .unwrap();
    assert_eq!(second.cache_tag, "hit");
    assert_eq!(
        second
            .cached_results
            .iter()
            .flat_map(|result| result.outputs.keys())
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [".once/out/out.txt"]
    );
    assert_eq!(std::fs::read(&side).unwrap(), b"precious");
    assert_eq!(std::fs::read(&output).unwrap(), b"stale");
    assert!(!dependency.exists());

    for result in &second.cached_results {
        source_digest_cache
            .materialize_outputs(result, workspace.path(), &cache, None)
            .await
            .unwrap();
    }
    assert_eq!(std::fs::read(&output).unwrap(), b"ok");
}

#[cfg(unix)]
#[tokio::test]
async fn uncacheable_action_materializes_dependency_inputs() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let dependency_path = ".once/out/dependency/input.txt";
    let dependency_blob = cache.put_blob(b"dependency").await.unwrap();
    let available_inputs = BTreeMap::from([(
        dependency_path.to_string(),
        AvailableInput {
            blob_digest: dependency_blob,
            same_target: false,
            materialized: false,
        },
    )]);
    let target = cached_test_target();
    let mut analysis = cached_test_analysis(dependency_path);
    analysis.actions[0].cacheable = false;
    analysis.actions[0].argv = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        format!("cat {dependency_path} > .once/out/out.txt"),
    ];

    let outcome = run_declared_actions(
        None,
        workspace.path(),
        &cache,
        module_digest(),
        &target,
        "build",
        analysis,
        &[],
        &available_inputs,
        &BTreeMap::new(),
        None,
        SandboxMode::default(),
        test_resources(),
        None,
        None,
        test_workers(),
    )
    .await
    .unwrap();

    assert_eq!(outcome.cache_tag, "bypass");
    assert_eq!(
        std::fs::read(workspace.path().join(".once/out/out.txt")).unwrap(),
        b"dependency"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn materialized_dependency_inputs_are_not_restored_again() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let dependency_path = ".once/out/dependency/input.txt";
    let dependency = workspace.path().join(dependency_path);
    std::fs::create_dir_all(dependency.parent().unwrap()).unwrap();
    std::fs::write(&dependency, b"ready").unwrap();
    let dependency_blob = cache.put_blob(b"stale").await.unwrap();
    let available_inputs = BTreeMap::from([(
        dependency_path.to_string(),
        AvailableInput {
            blob_digest: dependency_blob,
            same_target: false,
            materialized: true,
        },
    )]);
    let target = cached_test_target();
    let mut analysis = cached_test_analysis(dependency_path);
    analysis.actions[0].cacheable = false;
    analysis.actions[0].argv = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        format!("cat {dependency_path} > .once/out/out.txt"),
    ];

    run_declared_actions(
        None,
        workspace.path(),
        &cache,
        module_digest(),
        &target,
        "build",
        analysis,
        &[],
        &available_inputs,
        &BTreeMap::new(),
        None,
        SandboxMode::default(),
        test_resources(),
        None,
        None,
        test_workers(),
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read(&dependency).unwrap(), b"ready");
    assert_eq!(
        std::fs::read(workspace.path().join(".once/out/out.txt")).unwrap(),
        b"ready"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn incomplete_direct_cache_hit_falls_back_to_execution() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cache")));
    let target = cached_test_target();
    let analysis = || {
        let mut analysis = cached_test_analysis("unused");
        analysis.actions[0].inputs.clear();
        analysis
    };

    let first = run_declared_actions(
        None,
        workspace.path(),
        &cache,
        module_digest(),
        &target,
        "build",
        analysis(),
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        SandboxMode::default(),
        test_resources(),
        None,
        None,
        test_workers(),
    )
    .await
    .unwrap();
    assert_eq!(first.cache_tag, "miss");

    std::fs::remove_dir_all(cache.root().join("cas")).unwrap();
    std::fs::remove_file(workspace.path().join(".once/out/out.txt")).unwrap();

    let second = run_declared_actions(
        None,
        workspace.path(),
        &cache,
        module_digest(),
        &target,
        "build",
        analysis(),
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        SandboxMode::default(),
        test_resources(),
        None,
        None,
        test_workers(),
    )
    .await
    .unwrap();
    assert_eq!(second.cache_tag, "miss");
    assert_eq!(
        std::fs::read(workspace.path().join(".once/out/out.txt")).unwrap(),
        b"ok"
    );

    let third = run_declared_actions(
        None,
        workspace.path(),
        &cache,
        module_digest(),
        &target,
        "build",
        analysis(),
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        SandboxMode::default(),
        test_resources(),
        None,
        None,
        test_workers(),
    )
    .await
    .unwrap();
    assert_eq!(third.cache_tag, "hit");
}

#[tokio::test]
async fn action_result_completeness_checks_stream_and_output_blobs() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cache")));
    let present = cache.put_blob(b"present").await.unwrap();
    let missing = Digest::of_bytes(b"missing");
    let mut result = ActionResult {
        exit_code: 0,
        stdout: Some(present),
        stderr: None,
        outputs: BTreeMap::from([("out.txt".to_string(), present)]),
    };

    assert!(
        action_result_blobs_present(&result, workspace.path(), &cache, None)
            .await
            .unwrap()
    );
    result.stderr = Some(missing);
    assert!(
        !action_result_blobs_present(&result, workspace.path(), &cache, None)
            .await
            .unwrap()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn matching_indexed_output_does_not_require_cached_blob() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cache")));
    let output = workspace.path().join("out.txt");
    std::fs::write(&output, b"present").unwrap();
    let result = ActionResult {
        exit_code: 0,
        stdout: None,
        stderr: None,
        outputs: BTreeMap::from([(
            "out.txt".to_string(),
            Digest::of_bytes(b"absent from cache"),
        )]),
    };
    let digests = SourceDigestCache::open(workspace.path());
    digests.record_outputs(&result, workspace.path());

    assert!(
        action_result_blobs_present(&result, workspace.path(), &cache, Some(&digests))
            .await
            .unwrap()
    );
}

fn assert_declared_action_fingerprints(
    records: &[EvidenceRecord],
    aggregate: Option<InputFingerprintManifest>,
) {
    assert!(records
        .iter()
        .all(|record| record.input_fingerprint.is_some()));
    assert!(records.iter().any(|record| {
        record
            .input_fingerprint
            .as_ref()
            .is_some_and(|fingerprint| {
                fingerprint
                    .components
                    .iter()
                    .any(|component| component.category == "dependency")
            })
    }));
    assert!(records.iter().all(|record| {
        record.input_fingerprint.as_ref().is_none_or(|fingerprint| {
            fingerprint
                .components
                .iter()
                .filter(|component| component.label.starts_with("same-target:"))
                .count()
                <= 1
        })
    }));
    assert!(records.iter().any(|record| {
        record
            .input_fingerprint
            .as_ref()
            .is_some_and(|fingerprint| {
                fingerprint
                    .components
                    .iter()
                    .any(|component| component.label == "same-target:prior-actions")
            })
    }));
    let aggregate = aggregate.expect("multi-action outcome should aggregate fingerprints");
    assert!(aggregate
        .components
        .iter()
        .any(|component| component.label.starts_with("action:1:")));
    let encoded = serde_json::to_string(&records).unwrap();
    assert!(!encoded.contains("printf one"));
    assert!(!encoded.contains("printf two"));
}

#[cfg(unix)]
#[tokio::test]
async fn declared_actions_record_success_evidence_per_action() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let target = GraphTarget {
        label: once_frontend::TargetLabel {
            package: "tools".to_string(),
            name: "demo".to_string(),
            id: "tools/demo".to_string(),
        },
        kind: "demo_kind".to_string(),
        deps: Vec::new(),
        dependency_edges: BTreeMap::new(),
        srcs: Vec::new(),
        visibility: Vec::new(),
        attrs: BTreeMap::new(),
        capabilities: Vec::new(),
        providers: Vec::new(),
        tools: Vec::new(),
        diagnostics: Vec::new(),
    };
    let action = |name: &str| DeclaredAction {
        resources: ResourceRequest::default(),
        history: None,
        presentation: None,
        operation: None,
        argv: vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            format!("printf {name} > .once/out/{name}.txt"),
        ],
        arg_files: Vec::new(),
        inputs: Vec::new(),
        outputs: vec![format!(".once/out/{name}.txt")],
        stdout: None,
        stderr: None,
        clean_paths: Vec::new(),
        create_dirs: Vec::new(),
        cwd: None,
        env: BTreeMap::new(),
        sandbox: None,
        network: None,
        success_exit_codes: vec![0],
        cacheable: false,
        inherit_parent_env: false,
        depends_on_prior_actions: true,
        intermediate: false,
        toolchain_identity: None,
        identifier: Some(name.to_string()),
        display_name: None,
        source_files: Vec::new(),
    };
    let analysis = AnalysisResult {
        actions: vec![action("one"), action("two")],
        provider: serde_json::json!({}),
        declared_outputs: Vec::new(),
        observations: once_frontend::analysis::AnalysisObservations::default(),
    };

    let outcome = run_declared_actions(
        None,
        workspace.path(),
        &cache,
        module_digest(),
        &target,
        "build",
        analysis,
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        SandboxMode::default(),
        test_resources(),
        None,
        None,
        test_workers(),
    )
    .await
    .unwrap();

    assert_eq!(outcome.result.stdout, None);
    assert_eq!(outcome.result.stderr, None);
    assert_eq!(outcome.result.outputs.len(), 2);
    let records = once_core::EvidenceStore::open_workspace(workspace.path())
        .load()
        .await
        .unwrap();
    assert_eq!(records.len(), 2);
    assert!(records
        .iter()
        .all(|record| record.subject.matches("tools/demo:build")));
    assert!(records
        .iter()
        .any(|record| record.outputs.contains_key(".once/out/one.txt")));
    assert!(records
        .iter()
        .any(|record| record.outputs.contains_key(".once/out/two.txt")));
    assert_declared_action_fingerprints(&records, outcome.input_fingerprint);
}

#[cfg(unix)]
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "inline DeclaredAction literals carry many required fields"
)]
async fn declared_action_can_skip_prior_same_target_digests() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let target = GraphTarget {
        label: once_frontend::TargetLabel {
            package: "tools".to_string(),
            name: "split".to_string(),
            id: "tools/split".to_string(),
        },
        kind: "demo_kind".to_string(),
        deps: Vec::new(),
        dependency_edges: BTreeMap::new(),
        srcs: Vec::new(),
        visibility: Vec::new(),
        attrs: BTreeMap::new(),
        capabilities: Vec::new(),
        providers: Vec::new(),
        tools: Vec::new(),
        diagnostics: Vec::new(),
    };

    let analysis = |first_value: &str| AnalysisResult {
        actions: vec![
            DeclaredAction {
                resources: ResourceRequest::default(),
                operation: None,
                argv: vec![
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    format!("printf {first_value} > .once/out/first.txt"),
                ],
                arg_files: Vec::new(),
                inputs: Vec::new(),
                outputs: vec![".once/out/first.txt".to_string()],
                stdout: None,
                stderr: None,
                clean_paths: Vec::new(),
                create_dirs: Vec::new(),
                cwd: None,
                env: BTreeMap::new(),
                sandbox: None,
                network: None,
                success_exit_codes: vec![0],
                cacheable: true,
                inherit_parent_env: false,
                depends_on_prior_actions: true,
                intermediate: false,
                toolchain_identity: None,
                identifier: Some("first".to_string()),
                history: None,
                presentation: None,
                display_name: None,
                source_files: Vec::new(),
            },
            DeclaredAction {
                resources: ResourceRequest::default(),
                operation: None,
                argv: vec![
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "printf second > .once/out/second.txt; printf run >> second_runs".to_string(),
                ],
                arg_files: Vec::new(),
                inputs: Vec::new(),
                outputs: vec![".once/out/second.txt".to_string()],
                stdout: None,
                stderr: None,
                clean_paths: Vec::new(),
                create_dirs: Vec::new(),
                cwd: None,
                env: BTreeMap::new(),
                sandbox: None,
                network: None,
                success_exit_codes: vec![0],
                cacheable: true,
                inherit_parent_env: false,
                depends_on_prior_actions: false,
                intermediate: false,
                toolchain_identity: None,
                identifier: Some("second".to_string()),
                history: None,
                presentation: None,
                display_name: None,
                source_files: Vec::new(),
            },
        ],
        provider: serde_json::json!({}),
        declared_outputs: Vec::new(),
        observations: once_frontend::analysis::AnalysisObservations::default(),
    };

    let first = run_declared_actions(
        None,
        workspace.path(),
        &cache,
        module_digest(),
        &target,
        "build",
        analysis("one"),
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        SandboxMode::default(),
        test_resources(),
        None,
        None,
        test_workers(),
    )
    .await
    .unwrap();
    assert_eq!(first.cache_state, EvidenceCacheState::Miss);

    let second = run_declared_actions(
        None,
        workspace.path(),
        &cache,
        module_digest(),
        &target,
        "build",
        analysis("changed"),
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        SandboxMode::default(),
        test_resources(),
        None,
        None,
        test_workers(),
    )
    .await
    .unwrap();

    assert_eq!(second.cache_state, EvidenceCacheState::Miss);
    assert_eq!(
        std::fs::read_to_string(workspace.path().join(".once/out/first.txt")).unwrap(),
        "changed"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join(".once/out/second.txt")).unwrap(),
        "second"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("second_runs")).unwrap(),
        "run"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn uncached_action_errors_when_declared_output_is_missing() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let action = Action::RunCommand {
        argv: vec!["/bin/sh".to_string(), "-c".to_string(), ":".to_string()],
        env: BTreeMap::new(),
        cwd: None,
        input_digest: None,
        inputs: vec![],
        outputs: vec![WorkspacePath::try_from(".once/out/missing.txt").unwrap()],
        stdout_path: None,
        stderr_path: None,
        output_symlink_mode: OutputSymlinkMode::default(),
        resources: ResourceRequest::default(),
        sandbox: SandboxMode::default(),
        network: NetworkPolicy::default(),
        timeout_ms: None,
        success_exit_codes: vec![0],
        remote: None,
    };

    let err = run_uncached_action(&action, workspace.path(), &cache, false)
        .await
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("declared action completed without producing output `.once/out/missing.txt`"),
        "{err}"
    );
}

#[test]
fn input_digest_changes_with_toolchain_identity() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("input.txt"), b"content").unwrap();
    let declared = DeclaredAction {
        resources: ResourceRequest::default(),
        operation: None,
        argv: vec!["tool".to_string()],
        arg_files: Vec::new(),
        inputs: vec!["input.txt".to_string()],
        outputs: vec![".once/out/A.a".to_string()],
        stdout: None,
        stderr: None,
        clean_paths: Vec::new(),
        create_dirs: Vec::new(),
        cwd: None,
        env: BTreeMap::new(),
        sandbox: None,
        network: None,
        success_exit_codes: vec![0],
        cacheable: true,
        inherit_parent_env: false,
        depends_on_prior_actions: true,
        intermediate: false,
        history: None,
        presentation: None,
        toolchain_identity: Some("id-1".to_string()),
        identifier: None,
        display_name: None,
        source_files: Vec::new(),
    };
    let one = compose_input_digest(workspace.path(), &declared, module_digest(), &[]).unwrap();
    let declared2 = DeclaredAction {
        toolchain_identity: Some("id-2".to_string()),
        ..declared
    };
    let two = compose_input_digest(workspace.path(), &declared2, module_digest(), &[]).unwrap();
    assert_ne!(one, two);
}

#[test]
fn input_digest_frames_optional_identities_and_embedded_separators() {
    let workspace = tempfile::tempdir().unwrap();
    let mut digests = std::collections::HashSet::new();
    for (toolchain, identifier) in [
        (None, None),
        (Some(""), None),
        (None, Some("")),
        (Some("same"), None),
        (None, Some("same")),
        (Some("compiler\0action"), None),
        (Some("compiler"), Some("action")),
    ] {
        let declared: DeclaredAction = serde_json::from_value(serde_json::json!({
            "argv": ["tool"], "outputs": [],
            "toolchain_identity": toolchain, "identifier": identifier,
        }))
        .unwrap();
        let digest =
            compose_input_digest(workspace.path(), &declared, module_digest(), &[]).unwrap();
        assert!(
            digests.insert(digest),
            "identities must not collide: {toolchain:?}, {identifier:?}"
        );
        assert_eq!(
            digest,
            compose_input_digest(workspace.path(), &declared, module_digest(), &[]).unwrap()
        );
    }
}

#[test]
fn input_fingerprint_explains_inputs_without_exposing_values() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("input.txt"), b"content").unwrap();
    let declared = DeclaredAction {
        resources: ResourceRequest::default(),
        operation: None,
        history: None,
        presentation: None,
        argv: vec!["tool".to_string(), "--token=command-secret".to_string()],
        arg_files: Vec::new(),
        inputs: vec!["input.txt".to_string()],
        outputs: vec![".once/out/A.a".to_string()],
        stdout: None,
        stderr: None,
        clean_paths: Vec::new(),
        create_dirs: Vec::new(),
        cwd: None,
        env: BTreeMap::from([("TOKEN".to_string(), "environment-secret".to_string())]),
        sandbox: None,
        network: None,
        success_exit_codes: vec![0],
        cacheable: true,
        inherit_parent_env: false,
        depends_on_prior_actions: true,
        intermediate: false,
        toolchain_identity: Some("toolchain-secret".to_string()),
        identifier: Some("compile".to_string()),
        display_name: None,
        source_files: Vec::new(),
    };
    let dependency = Digest::of_bytes(b"dependency");
    let fingerprint = compose_input_fingerprint_with_available(
        workspace.path(),
        &declared,
        module_digest(),
        &[("core".to_string(), dependency)],
        &BTreeMap::new(),
        None,
    )
    .unwrap();
    let digest = compose_input_digest(
        workspace.path(),
        &declared,
        module_digest(),
        &[("core".to_string(), dependency)],
    )
    .unwrap();

    assert_eq!(fingerprint.input_digest, digest);
    for (category, label) in [
        ("toolchain", "identity"),
        ("action", "identifier"),
        ("command", "arguments"),
        ("environment", "declared"),
        ("environment", "variable:TOKEN"),
        ("source", "input.txt"),
    ] {
        assert!(fingerprint
            .components
            .iter()
            .any(|component| { component.category == category && component.label == label }));
    }
    let variable = fingerprint
        .components
        .iter()
        .find(|component| {
            component.category == "environment" && component.label == "variable:TOKEN"
        })
        .unwrap();
    assert_eq!(variable.digest, Digest::of_bytes(b"environment-secret"));
    let encoded = serde_json::to_string(&fingerprint).unwrap();
    assert!(!encoded.contains("command-secret"));
    assert!(!encoded.contains("environment-secret"));
    assert!(!encoded.contains("toolchain-secret"));
}

#[test]
fn presentation_and_history_do_not_change_action_cache_identity() {
    let workspace = tempfile::tempdir().unwrap();
    let mut declared: DeclaredAction = serde_json::from_value(serde_json::json!({
        "argv": ["tool"], "outputs": ["output"], "identifier": "compile",
    }))
    .unwrap();
    let original = compose_input_digest(workspace.path(), &declared, module_digest(), &[]).unwrap();
    declared.history = Some(once_core::ActionHistoryKey {
        namespace: "custom.v1".into(),
        key: "stable-step".into(),
    });
    declared.display_name = Some("Compile a friendlier name".to_string());
    declared.source_files = vec!["src/not-an-input.c".to_string()];
    declared.presentation = Some(once_core::ActionPresentation {
        context: vec![once_core::ActionContext {
            key: "custom.mode".into(),
            value: "release".into(),
            ..Default::default()
        }],
        ..Default::default()
    });
    assert_eq!(
        original,
        compose_input_digest(workspace.path(), &declared, module_digest(), &[]).unwrap()
    );
    let first = declared_to_action(
        workspace.path(),
        &declared,
        module_digest(),
        &[],
        SandboxMode::Off,
    )
    .unwrap();
    declared.history = Some(once_core::ActionHistoryKey {
        namespace: "custom.v2".into(),
        key: "different-step".into(),
    });
    declared.display_name = Some("Another display name".to_string());
    declared.source_files.clear();
    declared.presentation = None;
    let second = declared_to_action(
        workspace.path(),
        &declared,
        module_digest(),
        &[],
        SandboxMode::Off,
    )
    .unwrap();
    assert_eq!(first.digest(), second.digest());
}

#[test]
fn input_digest_changes_with_command_setup_paths() {
    let workspace = tempfile::tempdir().unwrap();
    let declared = DeclaredAction {
        resources: ResourceRequest::default(),
        operation: None,
        argv: vec!["tool".to_string()],
        arg_files: Vec::new(),
        inputs: Vec::new(),
        outputs: vec![".once/out/A.a".to_string()],
        stdout: None,
        stderr: None,
        clean_paths: vec![".once/out/A.a".to_string()],
        create_dirs: vec![".once/tmp/home".to_string()],
        cwd: None,
        env: BTreeMap::new(),
        sandbox: None,
        network: None,
        success_exit_codes: vec![0],
        cacheable: true,
        inherit_parent_env: false,
        depends_on_prior_actions: true,
        intermediate: false,
        toolchain_identity: None,
        identifier: None,
        display_name: None,
        source_files: Vec::new(),
        history: None,
        presentation: None,
    };
    let one = compose_input_digest(workspace.path(), &declared, module_digest(), &[]).unwrap();
    let declared2 = DeclaredAction {
        stdout: None,
        stderr: None,
        clean_paths: vec![".once/out/B.a".to_string()],
        ..declared
    };
    let two = compose_input_digest(workspace.path(), &declared2, module_digest(), &[]).unwrap();

    assert_ne!(one, two);
}

#[test]
fn input_digest_changes_with_declared_arg_files() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("input.txt"), b"content").unwrap();
    let declared = DeclaredAction {
        resources: ResourceRequest::default(),
        operation: None,
        history: None,
        presentation: None,
        argv: vec!["tool".to_string(), "@.once/out/args.rsp".to_string()],
        arg_files: vec![DeclaredArgFile {
            path: ".once/out/args.rsp".to_string(),
            format: DeclaredArgFileFormat::LineDelimited,
            args: vec!["--cfg".to_string(), "feature=\"alloc\"".to_string()],
        }],
        inputs: vec!["input.txt".to_string()],
        outputs: vec![".once/out/A.a".to_string()],
        stdout: None,
        stderr: None,
        clean_paths: Vec::new(),
        create_dirs: Vec::new(),
        cwd: None,
        env: BTreeMap::new(),
        sandbox: None,
        network: None,
        success_exit_codes: vec![0],
        cacheable: true,
        inherit_parent_env: false,
        depends_on_prior_actions: true,
        intermediate: false,
        toolchain_identity: None,
        identifier: None,
        display_name: None,
        source_files: Vec::new(),
    };
    let one = compose_input_digest(workspace.path(), &declared, module_digest(), &[]).unwrap();
    let declared2 = DeclaredAction {
        arg_files: vec![DeclaredArgFile {
            path: ".once/out/args.rsp".to_string(),
            format: DeclaredArgFileFormat::LineDelimited,
            args: vec!["--cfg".to_string(), "feature=\"std\"".to_string()],
        }],
        ..declared
    };
    let two = compose_input_digest(workspace.path(), &declared2, module_digest(), &[]).unwrap();

    assert_ne!(one, two);
}

#[test]
fn input_digest_ignores_unrelated_module_source_changes() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("input.txt"), b"content").unwrap();
    let declared = DeclaredAction {
        resources: ResourceRequest::default(),
        operation: None,
        argv: vec!["tool".to_string()],
        arg_files: Vec::new(),
        inputs: vec!["input.txt".to_string()],
        outputs: vec![".once/out/A.a".to_string()],
        stdout: None,
        stderr: None,
        clean_paths: Vec::new(),
        create_dirs: Vec::new(),
        cwd: None,
        env: BTreeMap::new(),
        sandbox: None,
        network: None,
        success_exit_codes: vec![0],
        cacheable: true,
        inherit_parent_env: false,
        depends_on_prior_actions: true,
        intermediate: false,
        toolchain_identity: None,
        identifier: None,
        display_name: None,
        source_files: Vec::new(),
        history: None,
        presentation: None,
    };
    let one = compose_input_digest(
        workspace.path(),
        &declared,
        Digest::of_bytes(b"modules-1"),
        &[],
    )
    .unwrap();
    let two = compose_input_digest(
        workspace.path(),
        &declared,
        Digest::of_bytes(b"modules-2"),
        &[],
    )
    .unwrap();

    assert_eq!(one, two);
}

#[test]
fn same_target_generated_input_uses_its_content_digest() {
    let workspace = tempfile::tempdir().unwrap();
    let input = ".once/out/fingerprint.txt";
    let declared = DeclaredAction {
        resources: ResourceRequest::default(),
        operation: None,
        argv: vec!["tool".to_string()],
        arg_files: Vec::new(),
        inputs: vec![input.to_string()],
        outputs: vec![".once/out/result.txt".to_string()],
        stdout: None,
        stderr: None,
        clean_paths: Vec::new(),
        create_dirs: Vec::new(),
        cwd: None,
        env: BTreeMap::new(),
        sandbox: None,
        network: None,
        success_exit_codes: vec![0],
        cacheable: true,
        inherit_parent_env: false,
        depends_on_prior_actions: true,
        intermediate: false,
        toolchain_identity: None,
        identifier: None,
        display_name: None,
        source_files: Vec::new(),
        history: None,
        presentation: None,
    };
    let available = |content| {
        BTreeMap::from([(
            input.to_string(),
            AvailableInput {
                blob_digest: Digest::of_bytes(content),
                same_target: true,
                materialized: false,
            },
        )])
    };

    let first = compose_input_digest_with_available(
        workspace.path(),
        &declared,
        module_digest(),
        &[],
        &available(b"false"),
        None,
    )
    .unwrap();
    let second = compose_input_digest_with_available(
        workspace.path(),
        &declared,
        module_digest(),
        &[],
        &available(b"true"),
        None,
    )
    .unwrap();

    assert_ne!(first, second);
}

#[test]
fn dependency_generated_input_uses_its_content_digest() {
    let workspace = tempfile::tempdir().unwrap();
    let input = ".once/out/dependency.txt";
    let declared = DeclaredAction {
        resources: ResourceRequest::default(),
        operation: None,
        argv: vec!["tool".to_string()],
        arg_files: Vec::new(),
        inputs: vec![input.to_string()],
        outputs: vec![".once/out/result.txt".to_string()],
        stdout: None,
        stderr: None,
        clean_paths: Vec::new(),
        create_dirs: Vec::new(),
        cwd: None,
        env: BTreeMap::new(),
        sandbox: None,
        network: None,
        success_exit_codes: vec![0],
        cacheable: true,
        inherit_parent_env: false,
        depends_on_prior_actions: true,
        intermediate: false,
        toolchain_identity: None,
        identifier: None,
        display_name: None,
        source_files: Vec::new(),
        history: None,
        presentation: None,
    };
    let available = |_producer: &[u8], content: &[u8]| {
        BTreeMap::from([(
            input.to_string(),
            AvailableInput {
                blob_digest: Digest::of_bytes(content),
                same_target: false,
                materialized: false,
            },
        )])
    };

    let first = compose_input_digest_with_available(
        workspace.path(),
        &declared,
        module_digest(),
        &[],
        &available(b"first producer action", b"stable module"),
        None,
    )
    .unwrap();
    let second = compose_input_digest_with_available(
        workspace.path(),
        &declared,
        module_digest(),
        &[],
        &available(b"second producer action", b"stable module"),
        None,
    )
    .unwrap();

    let third = compose_input_digest_with_available(
        workspace.path(),
        &declared,
        module_digest(),
        &[],
        &available(b"second producer action", b"changed module"),
        None,
    )
    .unwrap();

    assert_eq!(first, second);
    assert_ne!(first, third);
}

#[test]
fn input_digest_ignores_unconsumed_dependency_digests() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("input.txt"), b"content").unwrap();
    let declared = DeclaredAction {
        resources: ResourceRequest::default(),
        operation: None,
        argv: vec!["tool".to_string()],
        arg_files: Vec::new(),
        inputs: vec!["input.txt".to_string()],
        outputs: vec![".once/out/A.a".to_string()],
        stdout: None,
        stderr: None,
        clean_paths: Vec::new(),
        create_dirs: Vec::new(),
        cwd: None,
        env: BTreeMap::new(),
        sandbox: None,
        network: None,
        success_exit_codes: vec![0],
        cacheable: true,
        inherit_parent_env: false,
        depends_on_prior_actions: true,
        intermediate: false,
        toolchain_identity: None,
        identifier: None,
        display_name: None,
        source_files: Vec::new(),
        history: None,
        presentation: None,
    };
    let a = compose_input_digest(
        workspace.path(),
        &declared,
        module_digest(),
        &[
            ("dep1".to_string(), Digest::of_bytes(b"d1")),
            ("dep2".to_string(), Digest::of_bytes(b"d2")),
        ],
    )
    .unwrap();
    let b = compose_input_digest(
        workspace.path(),
        &declared,
        module_digest(),
        &[
            ("dep1".to_string(), Digest::of_bytes(b"changed")),
            ("dep3".to_string(), Digest::of_bytes(b"d3")),
        ],
    )
    .unwrap();
    assert_eq!(a, b);
}

#[test]
fn target_action_digest_preserves_single_action_digest() {
    let action = Digest::of_bytes(b"action");

    assert_eq!(compose_target_action_digest("Root", &[action]), action);
}

#[test]
fn target_action_digest_for_empty_actions_is_target_specific() {
    let root = compose_target_action_digest("Root", &[]);
    let same_root = compose_target_action_digest("Root", &[]);
    let other = compose_target_action_digest("Other", &[]);

    assert_eq!(root, same_root);
    assert_ne!(root, other);
}

#[test]
fn target_action_digest_includes_all_declared_actions_in_order() {
    let first = Digest::of_bytes(b"first");
    let second = Digest::of_bytes(b"second");
    let changed_second = Digest::of_bytes(b"changed-second");

    let original = compose_target_action_digest("Root", &[first, second]);
    let changed = compose_target_action_digest("Root", &[first, changed_second]);
    let reordered = compose_target_action_digest("Root", &[second, first]);

    assert_ne!(original, changed);
    assert_ne!(original, reordered);
}

#[test]
fn target_input_digest_handles_empty_single_and_multiple_inputs() {
    let first = Digest::of_bytes(b"first-input");
    let second = Digest::of_bytes(b"second-input");

    assert_eq!(compose_target_input_digest(&[]), None);
    assert_eq!(compose_target_input_digest(&[first]), Some(first));

    let original = compose_target_input_digest(&[first, second]).unwrap();
    let same = compose_target_input_digest(&[first, second]).unwrap();
    let reordered = compose_target_input_digest(&[second, first]).unwrap();

    assert_eq!(original, same);
    assert_ne!(original, reordered);
}
#[cfg(unix)]
#[tokio::test]
async fn emits_each_action_before_a_later_action_fails() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    std::fs::write(workspace.path().join("input.txt"), "input").unwrap();
    let target = cached_test_target();
    let mut analysis = cached_test_analysis("input.txt");
    analysis.actions[0].identifier = Some("prepare".to_string());
    let mut failure = analysis.actions[0].clone();
    failure.identifier = Some("compile".to_string());
    failure.argv = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        "exit 7".to_string(),
    ];
    analysis.actions.push(failure);
    let bus = once_core::RunEventBus::new(8);
    let mut rx = bus.subscribe();
    let error = run_declared_actions(
        None,
        workspace.path(),
        &cache,
        module_digest(),
        &target,
        "build",
        analysis,
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        SandboxMode::default(),
        test_resources(),
        None,
        Some(&bus),
        test_workers(),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("exit code 7"));
    assert!(matches!(
        rx.try_recv().unwrap(),
        once_core::RunEvent::ActionAttemptStarted {
            action_index: 0,
            attempt: 1,
            ..
        }
    ));
    assert!(matches!(
        rx.try_recv().unwrap(),
        once_core::RunEvent::ActionAttemptCompleted {
            action_index: 0,
            attempt: 1,
            exit_code: 0,
            ..
        }
    ));
    assert!(
        matches!(rx.try_recv().unwrap(), once_core::RunEvent::ActionCompleted {
            action_index: 0, identifier: Some(id), exit_code: 0, selected_attempt: 1, ..
        } if id == "prepare")
    );
    assert!(matches!(
        rx.try_recv().unwrap(),
        once_core::RunEvent::ActionAttemptStarted {
            action_index: 1,
            attempt: 1,
            ..
        }
    ));
    assert!(matches!(
        rx.try_recv().unwrap(),
        once_core::RunEvent::ActionAttemptCompleted {
            action_index: 1,
            attempt: 1,
            exit_code: 7,
            ..
        }
    ));
    assert!(
        matches!(rx.try_recv().unwrap(), once_core::RunEvent::ActionCompleted {
            action_index: 1, identifier: Some(id), exit_code: 7, selected_attempt: 1, ..
        } if id == "compile")
    );
    assert!(rx.try_recv().is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn file_plumbing_runs_without_being_reported_as_an_action() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    std::fs::write(workspace.path().join("input.txt"), "input").unwrap();
    let mut analysis = cached_test_analysis("input.txt");
    let mut write = analysis.actions[0].clone();
    write.operation = Some(DeclaredActionOperation::WriteFile {
        path: ".once/out/recipe.txt".to_string(),
        bytes: b"recipe".to_vec(),
    });
    write.argv = Vec::new();
    write.inputs = Vec::new();
    write.outputs = vec![".once/out/recipe.txt".to_string()];
    write.identifier = Some("write_path:.once/out/recipe.txt".to_string());
    analysis.actions.insert(0, write);
    let bus = once_core::RunEventBus::new(16);
    let mut rx = bus.subscribe();

    let outcome = run_declared_actions(
        None,
        workspace.path(),
        &cache,
        module_digest(),
        &cached_test_target(),
        "build",
        analysis,
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        SandboxMode::default(),
        test_resources(),
        None,
        Some(&bus),
        test_workers(),
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(workspace.path().join(".once/out/recipe.txt")).unwrap(),
        "recipe"
    );
    let mut reported = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event {
            once_core::RunEvent::ActionAttemptStarted { action_index, .. }
            | once_core::RunEvent::ActionAttemptCompleted { action_index, .. } => {
                reported.push(action_index);
            }
            once_core::RunEvent::ActionCompleted {
                action_index,
                identifier,
                ..
            } => {
                assert_eq!(identifier.as_deref(), Some("cached"));
                reported.push(action_index);
            }
            _ => {}
        }
    }
    assert_eq!(reported, [1, 1, 1]);
    assert_eq!(
        outcome
            .per_action_outcomes
            .iter()
            .map(|action| action.identifier.as_deref())
            .collect::<Vec<_>>(),
        [Some("cached")]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn failing_file_plumbing_is_still_reported() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let mut analysis = cached_test_analysis("input.txt");
    let mut copy = analysis.actions[0].clone();
    copy.operation = Some(DeclaredActionOperation::CopyPath {
        sources: vec!["missing.txt".to_string()],
        destination: ".once/out/copied.txt".to_string(),
        mode: DeclaredCopyPathMode::File,
    });
    copy.argv = Vec::new();
    copy.inputs = vec!["missing.txt".to_string()];
    copy.outputs = vec![".once/out/copied.txt".to_string()];
    copy.identifier = Some("copy_path:.once/out/copied.txt".to_string());
    analysis.actions = vec![copy];
    let bus = once_core::RunEventBus::new(16);
    let mut rx = bus.subscribe();

    let result = run_declared_actions(
        None,
        workspace.path(),
        &cache,
        module_digest(),
        &cached_test_target(),
        "build",
        analysis,
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        SandboxMode::default(),
        test_resources(),
        None,
        Some(&bus),
        test_workers(),
    )
    .await;

    assert!(result.is_err());
    assert!(matches!(
        rx.try_recv().unwrap(),
        once_core::RunEvent::ActionAttemptStarted {
            action_index: 0,
            ..
        }
    ));
    assert!(matches!(
        rx.try_recv().unwrap(),
        once_core::RunEvent::ActionAttemptCompleted {
            action_index: 0,
            ..
        }
    ));
    assert!(matches!(
        rx.try_recv().unwrap(),
        once_core::RunEvent::ActionCompleted { action_index: 0, identifier: Some(id), .. }
            if id == "copy_path:.once/out/copied.txt"
    ));
}

fn shell_action(
    identifier: &str,
    script: &str,
    inputs: &[&str],
    outputs: &[&str],
    intermediate: bool,
) -> DeclaredAction {
    DeclaredAction {
        resources: ResourceRequest::default(),
        operation: None,
        argv: vec!["/bin/sh".to_string(), "-c".to_string(), script.to_string()],
        arg_files: Vec::new(),
        inputs: inputs.iter().map(ToString::to_string).collect(),
        outputs: outputs.iter().map(ToString::to_string).collect(),
        stdout: None,
        stderr: None,
        clean_paths: Vec::new(),
        create_dirs: vec![".once/out".to_string()],
        cwd: None,
        env: BTreeMap::new(),
        sandbox: None,
        network: None,
        success_exit_codes: vec![0],
        cacheable: true,
        inherit_parent_env: false,
        depends_on_prior_actions: false,
        intermediate,
        toolchain_identity: None,
        identifier: Some(identifier.to_string()),
        history: None,
        presentation: None,
        display_name: None,
        source_files: Vec::new(),
    }
}

/// A snapshot chain: `step` writes an intermediate file and `final`
/// derives the target's output from it.
fn intermediate_chain_analysis(final_suffix: &str) -> AnalysisResult {
    AnalysisResult {
        actions: vec![
            shell_action(
                "step",
                "printf step > .once/out/step.txt",
                &[],
                &[".once/out/step.txt"],
                true,
            ),
            shell_action(
                "final",
                &format!(
                    "{{ cat .once/out/step.txt; printf %s {final_suffix}; }} > .once/out/final.txt"
                ),
                &[".once/out/step.txt"],
                &[".once/out/final.txt"],
                false,
            ),
        ],
        provider: serde_json::json!({}),
        declared_outputs: Vec::new(),
        observations: once_frontend::analysis::AnalysisObservations::default(),
    }
}

async fn run_chain(
    workspace: &Path,
    cache: &CacheProvider,
    analysis: AnalysisResult,
) -> BuildOutcome {
    run_declared_actions(
        None,
        workspace,
        cache,
        module_digest(),
        &cached_test_target(),
        "build",
        analysis,
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        Some(&SourceDigestCache::open(workspace)),
        SandboxMode::default(),
        test_resources(),
        None,
        None,
        test_workers(),
    )
    .await
    .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn cached_intermediate_outputs_are_not_restored_when_the_target_finishes() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    run_chain(workspace.path(), &cache, intermediate_chain_analysis("-a")).await;
    std::fs::remove_dir_all(workspace.path().join(".once/out")).unwrap();

    let second = run_chain(workspace.path(), &cache, intermediate_chain_analysis("-a")).await;

    assert_eq!(second.cache_tag, "hit");
    assert_eq!(
        second
            .cached_results
            .iter()
            .flat_map(|result| result.outputs.keys())
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [".once/out/final.txt"]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn an_action_that_runs_restores_only_the_cached_outputs_it_reads() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let mut analysis = intermediate_chain_analysis("-a");
    analysis.actions.insert(
        0,
        shell_action(
            "unrelated",
            "printf unrelated > .once/out/unrelated.txt",
            &[],
            &[".once/out/unrelated.txt"],
            false,
        ),
    );
    run_chain(workspace.path(), &cache, analysis.clone()).await;
    std::fs::remove_dir_all(workspace.path().join(".once/out")).unwrap();

    analysis.actions[2] = intermediate_chain_analysis("-b").actions.remove(1);
    let second = run_chain(workspace.path(), &cache, analysis).await;

    assert_eq!(second.cache_tag, "miss");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join(".once/out/final.txt")).unwrap(),
        "step-b"
    );
    assert!(!workspace.path().join(".once/out/unrelated.txt").exists());
    assert_eq!(
        second
            .cached_results
            .iter()
            .flat_map(|result| result.outputs.keys())
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [".once/out/unrelated.txt"],
        "the unrelated output still waits to be restored when the target finishes"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_cached_output_an_independent_action_replaces_is_not_restored_over_it() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let analysis = |second: &str| AnalysisResult {
        actions: vec![
            shell_action(
                "first",
                "printf first > .once/out/shared.txt",
                &[],
                &[".once/out/shared.txt"],
                false,
            ),
            shell_action(
                "second",
                &format!("printf {second} > .once/out/shared.txt"),
                &[],
                &[".once/out/shared.txt"],
                false,
            ),
        ],
        ..intermediate_chain_analysis("-a")
    };
    run_chain(workspace.path(), &cache, analysis("second")).await;

    let rerun = run_chain(workspace.path(), &cache, analysis("changed")).await;

    assert!(rerun
        .cached_results
        .iter()
        .all(|result| !result.outputs.contains_key(".once/out/shared.txt")));
    assert_eq!(
        std::fs::read_to_string(workspace.path().join(".once/out/shared.txt")).unwrap(),
        "changed"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_cached_directory_is_restored_before_an_independent_action_writes_into_it() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let analysis = |child: &str| AnalysisResult {
        actions: vec![
            shell_action(
                "tree",
                "mkdir -p .once/out/tree && printf kept > .once/out/tree/kept.txt",
                &[],
                &[".once/out/tree"],
                false,
            ),
            shell_action(
                "child",
                &format!("printf {child} > .once/out/tree/new.txt"),
                &[],
                &[".once/out/tree/new.txt"],
                false,
            ),
        ],
        ..intermediate_chain_analysis("-a")
    };
    run_chain(workspace.path(), &cache, analysis("first")).await;
    std::fs::remove_dir_all(workspace.path().join(".once/out")).unwrap();

    let rerun = run_chain(workspace.path(), &cache, analysis("second")).await;

    let tree = workspace.path().join(".once/out/tree");
    assert_eq!(
        std::fs::read_to_string(tree.join("kept.txt")).unwrap(),
        "kept"
    );
    assert_eq!(
        std::fs::read_to_string(tree.join("new.txt")).unwrap(),
        "second"
    );
    assert!(rerun.cached_results.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn a_cached_output_a_later_action_removes_stays_removed() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let mut remove = shell_action("remove", "", &[], &[], false);
    remove.argv = Vec::new();
    remove.cacheable = false;
    remove.operation = Some(DeclaredActionOperation::PreparePath {
        path: ".once/out/file.txt".to_string(),
        mode: DeclaredPreparePathMode::Remove,
    });
    let analysis = AnalysisResult {
        actions: vec![
            shell_action(
                "file",
                "printf file > .once/out/file.txt",
                &[],
                &[".once/out/file.txt"],
                false,
            ),
            remove,
        ],
        ..intermediate_chain_analysis("-a")
    };
    run_chain(workspace.path(), &cache, analysis.clone()).await;

    let rerun = run_chain(workspace.path(), &cache, analysis).await;

    assert!(!workspace.path().join(".once/out/file.txt").exists());
    assert!(rerun.cached_results.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn intermediate_outputs_are_recorded_with_the_outcome() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));

    let outcome = run_chain(workspace.path(), &cache, intermediate_chain_analysis("-a")).await;

    assert_eq!(
        outcome
            .intermediate_outputs
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [".once/out/step.txt"]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_batch_writing_into_a_cached_directory_keeps_it_for_later_readers() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let analysis = |child: &str| AnalysisResult {
        actions: vec![
            shell_action(
                "tree",
                "mkdir -p .once/out/tree && printf kept > .once/out/tree/kept.txt",
                &[],
                &[".once/out/tree"],
                false,
            ),
            shell_action(
                "child",
                &format!("printf {child} > .once/out/tree/new.txt"),
                &[],
                &[".once/out/tree/new.txt"],
                false,
            ),
            shell_action(
                "unrelated",
                "printf unrelated > .once/out/unrelated.txt",
                &[],
                &[".once/out/unrelated.txt"],
                false,
            ),
            shell_action(
                "listing",
                "ls .once/out/tree > .once/out/listing.txt",
                &[".once/out/tree"],
                &[".once/out/listing.txt"],
                false,
            ),
        ],
        ..intermediate_chain_analysis("-a")
    };
    run_chain(workspace.path(), &cache, analysis("first")).await;
    std::fs::remove_dir_all(workspace.path().join(".once/out")).unwrap();

    let mut changed = analysis("second");
    changed.actions[3].argv[2].push_str(" && true");
    run_chain(workspace.path(), &cache, changed).await;

    let tree = workspace.path().join(".once/out/tree");
    assert_eq!(
        std::fs::read_to_string(tree.join("kept.txt")).unwrap(),
        "kept"
    );
    assert_eq!(
        std::fs::read_to_string(tree.join("new.txt")).unwrap(),
        "second"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join(".once/out/listing.txt")).unwrap(),
        "kept.txt\nnew.txt\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn an_ordered_action_finds_the_cached_intermediate_outputs_it_reads_undeclared() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let analysis = |suffix: &str| {
        let mut analysis = intermediate_chain_analysis(suffix);
        analysis.actions[1].inputs = Vec::new();
        analysis.actions[1].depends_on_prior_actions = true;
        analysis
    };
    run_chain(workspace.path(), &cache, analysis("-a")).await;
    std::fs::remove_dir_all(workspace.path().join(".once/out")).unwrap();

    run_chain(workspace.path(), &cache, analysis("-b")).await;

    assert_eq!(
        std::fs::read_to_string(workspace.path().join(".once/out/final.txt")).unwrap(),
        "step-b"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_cached_output_a_later_action_redirects_into_is_not_restored_over_it() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
    let analysis = |message: &str| {
        let mut redirect = shell_action("redirect", &format!("printf {message}"), &[], &[], false);
        redirect.stdout = Some(".once/out/shared.log".to_string());
        AnalysisResult {
            actions: vec![
                shell_action(
                    "first",
                    "printf first > .once/out/shared.log",
                    &[],
                    &[".once/out/shared.log"],
                    false,
                ),
                redirect,
            ],
            ..intermediate_chain_analysis("-a")
        }
    };
    run_chain(workspace.path(), &cache, analysis("second")).await;

    let rerun = run_chain(workspace.path(), &cache, analysis("changed")).await;

    assert!(rerun.cached_results.is_empty());
    assert_eq!(
        std::fs::read_to_string(workspace.path().join(".once/out/shared.log")).unwrap(),
        "changed"
    );
}
