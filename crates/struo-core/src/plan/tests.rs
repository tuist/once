use std::collections::BTreeMap;

use super::*;
use crate::{
    NetworkPolicy, OutputSymlinkMode, ResourceRequest, RunOpts, SandboxMode, WorkspacePath,
};
use struo_cas::Cas;
use tempfile::TempDir;

fn ws() -> (TempDir, Runner) {
    let tmp = TempDir::new().unwrap();
    let cas = Cas::open(tmp.path());
    let runner = Runner::new(cas, tmp.path().to_path_buf(), RunOpts::default());
    (tmp, runner)
}

fn cmd(script: &str) -> Action {
    Action::RunCommand {
        argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
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
        timeout_ms: Some(10_000),
        success_exit_codes: vec![0],
        remote: None,
    }
}

#[test]
fn validate_rejects_out_of_bounds_dep() {
    let mut plan = Plan::new();
    plan.push(PlanNode {
        label: "a".into(),
        action: cmd("true"),
        deps: vec![5],
    });
    assert!(matches!(plan.validate(), Err(PlanError::BadDep(_, _))));
}

#[test]
fn validate_rejects_cycle() {
    let mut plan = Plan::new();
    plan.push(PlanNode {
        label: "a".into(),
        action: cmd("true"),
        deps: vec![1],
    });
    plan.push(PlanNode {
        label: "b".into(),
        action: cmd("true"),
        deps: vec![0],
    });
    assert!(matches!(plan.validate(), Err(PlanError::Cycle)));
}

#[tokio::test]
async fn duplicate_dependencies_run_once() {
    let (tmp, runner) = ws();
    let plan = Plan {
        nodes: vec![
            PlanNode {
                label: "producer".into(),
                action: cmd("printf producer"),
                deps: vec![],
            },
            PlanNode {
                label: "consumer".into(),
                action: cmd("printf x >> marker"),
                deps: vec![0, 0],
            },
        ],
    };
    let outcomes = runner.run_plan(&plan).await.unwrap();
    assert_eq!(outcomes.len(), 2);
    assert_eq!(std::fs::read(tmp.path().join("marker")).unwrap(), b"x");
}

#[tokio::test]
async fn accepted_nonzero_exit_code_unblocks_dependents_on_miss_and_hit() {
    let (tmp, runner) = ws();
    let mut action = cmd("exit 7");
    if let Action::RunCommand {
        success_exit_codes, ..
    } = &mut action
    {
        *success_exit_codes = vec![7];
    }
    let plan = Plan {
        nodes: vec![
            PlanNode {
                label: "accepted".into(),
                action,
                deps: vec![],
            },
            PlanNode {
                label: "dependent".into(),
                action: Action::WriteFile {
                    path: WorkspacePath::try_from("marker").unwrap(),
                    bytes: b"dependent ran".to_vec(),
                    input_digest: None,
                },
                deps: vec![0],
            },
        ],
    };
    for expected_cache in [crate::CacheState::Miss, crate::CacheState::Hit] {
        let outcomes = runner.run_plan(&plan).await.unwrap();
        assert_eq!(outcomes[0].outcome.result.exit_code, 0);
        assert_eq!(outcomes[0].outcome.cache, expected_cache);
        assert_eq!(
            std::fs::read(tmp.path().join("marker")).unwrap(),
            b"dependent ran"
        );
        std::fs::remove_file(tmp.path().join("marker")).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn empty_plan_returns_empty_outcomes() {
    let (_tmp, runner) = ws();
    let plan = Plan::new();
    let outcomes = runner.run_plan(&plan).await.unwrap();
    assert!(outcomes.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn linear_chain_runs_in_order() {
    let (tmp, runner) = ws();
    let marker = tmp.path().join("marker");
    let mk = |name: &str| Action::RunCommand {
        argv: vec![
            "/bin/sh".into(),
            "-c".into(),
            format!("printf {name} > {}", marker.display()),
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
        sandbox: SandboxMode::default(),
        network: NetworkPolicy::default(),
        timeout_ms: Some(5_000),
        success_exit_codes: vec![0],
        remote: None,
    };
    let mut plan = Plan::new();
    let a = plan.push(PlanNode {
        label: "a".into(),
        action: mk("AAA"),
        deps: vec![],
    });
    let b = plan.push(PlanNode {
        label: "b".into(),
        action: mk("BBB"),
        deps: vec![a],
    });
    let _c = plan.push(PlanNode {
        label: "c".into(),
        action: mk("CCC"),
        deps: vec![b],
    });
    let outcomes = runner.run_plan(&plan).await.unwrap();
    assert_eq!(outcomes.len(), 3);
    let final_marker = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(final_marker, "CCC");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn independent_leaves_run_in_parallel() {
    let (tmp, runner) = ws();
    let leaf_count = 6usize;
    let runner = runner.with_max_concurrency(leaf_count);
    let started = (0..leaf_count)
        .map(|index| tmp.path().join(format!("parallel/leaf-{index}.started")))
        .collect::<Vec<_>>();
    let wait_for_peers = |name: String| {
        Action::RunCommand {
                argv: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "mkdir -p parallel; : > \"parallel/$1.started\"; while [ \"$(find parallel -name '*.started' -print | wc -l | tr -d '[:space:]')\" -lt \"$2\" ]; do sleep 0.01; done; printf '%s' \"$1\"".into(),
                    "wait-for-peers".into(),
                    name,
                    leaf_count.to_string(),
                ],
                env: BTreeMap::new(),
                cwd: Some(WorkspacePath::root()),
                input_digest: None,
            inputs: vec![],
                outputs: vec![],
                stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
                resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            network: NetworkPolicy::default(),
                timeout_ms: Some(5_000),
                success_exit_codes: vec![0],
                remote: None,
            }
    };
    let mut plan = Plan::new();
    for index in 0..leaf_count {
        let name = format!("leaf-{index}");
        plan.push(PlanNode {
            label: name.clone(),
            action: wait_for_peers(name),
            deps: vec![],
        });
    }

    let outcomes = runner.run_plan(&plan).await.unwrap();
    assert_eq!(outcomes.len(), leaf_count);
    for outcome in &outcomes {
        assert_eq!(outcome.outcome.result.exit_code, 0);
    }
    for marker in started {
        assert!(marker.exists(), "missing marker {}", marker.display());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failure_aborts_the_plan() {
    let (_tmp, runner) = ws();
    let mut plan = Plan::new();
    let bad = plan.push(PlanNode {
        label: "boom".into(),
        action: cmd("exit 7"),
        deps: vec![],
    });
    plan.push(PlanNode {
        label: "downstream".into(),
        action: cmd("printf should-not-run"),
        deps: vec![bad],
    });
    let err = runner.run_plan(&plan).await.unwrap_err();
    match err {
        PlanError::Failed { label, exit_code } => {
            assert_eq!(label, "boom");
            assert_eq!(exit_code, 7);
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn declared_outputs_flow_into_dependents() {
    let (tmp, runner) = ws();
    let producer = Action::RunCommand {
        argv: vec![
            "/bin/sh".into(),
            "-c".into(),
            "mkdir -p out && printf 'hello world' > out/produced.txt".into(),
        ],
        env: BTreeMap::new(),
        cwd: None,
        input_digest: None,
        inputs: vec![],
        outputs: vec![WorkspacePath::try_from("out/produced.txt").unwrap()],
        stdout_path: None,
        stderr_path: None,
        output_symlink_mode: OutputSymlinkMode::default(),
        resources: ResourceRequest::default(),
        sandbox: SandboxMode::default(),
        network: NetworkPolicy::default(),
        timeout_ms: Some(5_000),
        success_exit_codes: vec![0],
        remote: None,
    };
    let consumer = Action::RunCommand {
        argv: vec!["/bin/sh".into(), "-c".into(), "cat out/produced.txt".into()],
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
        timeout_ms: Some(5_000),
        success_exit_codes: vec![0],
        remote: None,
    };
    let mut plan = Plan::new();
    let p = plan.push(PlanNode {
        label: "producer".into(),
        action: producer.clone(),
        deps: vec![],
    });
    plan.push(PlanNode {
        label: "consumer".into(),
        action: consumer,
        deps: vec![p],
    });
    let outcomes = runner.run_plan(&plan).await.unwrap();
    assert_eq!(outcomes.len(), 2);
    let consumer_stdout = runner
        .cache()
        .get_blob(&outcomes[1].outcome.result.stdout.unwrap())
        .await
        .unwrap();
    assert_eq!(consumer_stdout, b"hello world");

    // Restore-on-hit: delete the produced file, run the producer
    // alone; the cache hit must restore the file from CAS.
    std::fs::remove_file(tmp.path().join("out/produced.txt")).unwrap();
    let outcome = runner.run(&producer).await.unwrap();
    assert_eq!(outcome.cache, crate::CacheState::Hit);
    let restored = std::fs::read_to_string(tmp.path().join("out/produced.txt")).unwrap();
    assert_eq!(restored, "hello world");
}
