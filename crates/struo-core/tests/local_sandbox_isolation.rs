//! Per-execution sandboxes for local actions: concurrent runs of one action
//! never share an execroot, and every exit path removes the sandbox.
#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use struo_cas::{CacheProvider, Cas};
use struo_core::{
    run_uncached, Action, Error, NetworkPolicy, OutputSymlinkMode, ResourceRequest, SandboxMode,
    WorkspacePath,
};
use tempfile::TempDir;

fn sandboxed(script: &str, timeout_ms: Option<u64>) -> Action {
    Action::RunCommand {
        argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
        env: BTreeMap::new(),
        cwd: None,
        input_digest: None,
        inputs: vec![WorkspacePath::try_from("in.txt".to_string()).unwrap()],
        outputs: vec![WorkspacePath::try_from("out.txt".to_string()).unwrap()],
        stdout_path: None,
        stderr_path: None,
        output_symlink_mode: OutputSymlinkMode::default(),
        resources: ResourceRequest::default(),
        sandbox: SandboxMode::Inputs,
        network: NetworkPolicy::Unrestricted,
        timeout_ms,
        success_exit_codes: vec![0],
        remote: None,
    }
}

fn setup() -> (TempDir, CacheProvider) {
    let workspace = TempDir::new().unwrap();
    std::fs::write(workspace.path().join("in.txt"), "input").unwrap();
    let cache = CacheProvider::Local(Cas::open(workspace.path().join(".struo/cas")));
    (workspace, cache)
}

fn sandboxes(workspace: &Path) -> Vec<String> {
    match std::fs::read_dir(workspace.join(".struo/sandboxes")) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    }
}

async fn wait_for_no_sandboxes(workspace: &Path) {
    for _ in 0..100 {
        if sandboxes(workspace).is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("sandboxes left behind: {:?}", sandboxes(workspace));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_runs_of_one_action_get_separate_sandboxes() {
    let (workspace, cache) = setup();
    // Each run reads its staged input after the others have staged theirs. A
    // shared execroot would be cleared by a later run before the read.
    let mut action = sandboxed("pwd; sleep 1; cat in.txt", Some(30_000));
    if let Action::RunCommand { outputs, .. } = &mut action {
        outputs.clear();
    }
    let runs = (0..4).map(|_| run_uncached(&action, workspace.path(), &cache, false));
    let results = futures::future::join_all(runs).await;

    let mut execroots = BTreeSet::new();
    for result in results {
        let result = result.expect("every concurrent run completes");
        assert_eq!(result.exit_code, 0);
        let stdout = cache.get_blob(&result.stdout.unwrap()).await.unwrap();
        let stdout = String::from_utf8(stdout).unwrap();
        let (execroot, input) = stdout.split_once('\n').unwrap();
        assert_eq!(input, "input");
        assert!(
            execroot.contains(&action.digest().to_string()),
            "sandbox is still named after the action: {execroot}"
        );
        execroots.insert(execroot.to_string());
    }
    assert_eq!(execroots.len(), 4, "runs shared an execroot: {execroots:?}");
    wait_for_no_sandboxes(workspace.path()).await;
}

#[tokio::test]
async fn a_failed_run_removes_its_sandbox() {
    let (workspace, cache) = setup();
    let action = sandboxed("cat in.txt > out.txt; exit 7", None);
    let result = run_uncached(&action, workspace.path(), &cache, false)
        .await
        .unwrap();
    assert_eq!(result.exit_code, 7);
    assert!(!workspace.path().join("out.txt").exists());
    wait_for_no_sandboxes(workspace.path()).await;
}

#[tokio::test]
async fn a_timed_out_run_removes_its_sandbox() {
    let (workspace, cache) = setup();
    let action = sandboxed("sleep 30 & sleep 30", Some(200));
    let result = run_uncached(&action, workspace.path(), &cache, false).await;
    assert!(matches!(result, Err(Error::Timeout(_))), "{result:?}");
    wait_for_no_sandboxes(workspace.path()).await;
}

#[tokio::test]
async fn a_cancelled_run_removes_its_sandbox() {
    let (workspace, cache) = setup();
    let action = sandboxed("echo started > started; sleep 30", None);
    let run = run_uncached(&action, workspace.path(), &cache, false);
    let cancelled = tokio::time::timeout(Duration::from_millis(500), run).await;
    assert!(cancelled.is_err(), "the run finished before cancellation");
    wait_for_no_sandboxes(workspace.path()).await;
}

#[tokio::test]
async fn a_successful_run_copies_outputs_then_removes_its_sandbox() {
    let (workspace, cache) = setup();
    let action = sandboxed("cat in.txt > out.txt", None);
    let result = run_uncached(&action, workspace.path(), &cache, false)
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("out.txt")).unwrap(),
        "input"
    );
    assert!(sandboxes(workspace.path()).is_empty());
}

#[tokio::test]
async fn a_missing_sandbox_output_cannot_reuse_a_stale_workspace_output() {
    for mode in [SandboxMode::Inputs, SandboxMode::CopiedInputs] {
        let (workspace, cache) = setup();
        std::fs::write(workspace.path().join("out.txt"), "stale").unwrap();
        let mut action = sandboxed("test ! -e out.txt && cat in.txt > fresh.txt", None);
        if let Action::RunCommand {
            sandbox, outputs, ..
        } = &mut action
        {
            *sandbox = mode;
            outputs.insert(0, WorkspacePath::try_from("fresh.txt").unwrap());
        }

        let result = run_uncached(&action, workspace.path(), &cache, false).await;

        assert!(
            matches!(&result, Err(Error::MissingOutput { path }) if path == "out.txt"),
            "{mode:?}: {result:?}"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("out.txt")).unwrap(),
            "stale"
        );
        assert!(!workspace.path().join("fresh.txt").exists());
        wait_for_no_sandboxes(workspace.path()).await;
    }
}
