use std::collections::VecDeque;
use std::future::Future;

use tokio::task::JoinSet;
use tracing::{debug, warn};

use super::topology::Topology;
use super::{Plan, PlanError, PlanNode, PlanOutcome};
use crate::Outcome;

pub(super) async fn run<F, Fut>(
    plan: &Plan,
    max_in_flight: usize,
    mut execute: F,
) -> Result<Vec<PlanOutcome>, PlanError>
where
    F: FnMut(&PlanNode) -> Fut,
    Fut: Future<Output = crate::Result<Outcome>> + Send + 'static,
{
    let mut topology = Topology::new(plan)?;
    let count = plan.nodes.len();
    let mut ready: VecDeque<_> = topology
        .remaining
        .iter()
        .enumerate()
        .filter_map(|(index, &remaining)| (remaining == 0).then_some(index))
        .collect();
    let mut running = JoinSet::new();
    let mut outcomes: Vec<Option<PlanOutcome>> = (0..count).map(|_| None).collect();
    let mut aborted = None;
    debug!(node_count = count, max_in_flight, "starting plan execution");

    loop {
        if aborted.is_none() {
            while running.len() < max_in_flight.max(1) {
                let Some(index) = ready.pop_front() else {
                    break;
                };
                let node = &plan.nodes[index];
                debug!(
                    node_index = index,
                    node_label = %node.label,
                    dependency_count = node.deps.len(),
                    "spawning plan node"
                );
                let work = execute(node);
                running.spawn(async move { (index, work.await) });
            }
        }
        let Some(joined) = running.join_next().await else {
            break;
        };
        let result = match joined {
            Ok((index, result)) => complete(
                plan,
                &mut topology,
                &mut ready,
                &mut outcomes,
                index,
                result,
                aborted.is_none(),
            ),
            Err(error) => Err(PlanError::Task(error)),
        };
        if let Err(error) = result {
            if aborted.is_none() {
                aborted = Some(error);
            }
        }
    }

    if let Some(error) = aborted {
        warn!(%error, "plan execution aborted");
        return Err(error);
    }
    Ok(outcomes
        .into_iter()
        .map(|slot| slot.expect("validated DAG must populate every outcome"))
        .collect())
}

fn complete(
    plan: &Plan,
    topology: &mut Topology,
    ready: &mut VecDeque<usize>,
    outcomes: &mut [Option<PlanOutcome>],
    index: usize,
    result: crate::Result<Outcome>,
    schedule_dependents: bool,
) -> Result<(), PlanError> {
    let node = &plan.nodes[index];
    let outcome = result.map_err(|error| {
        warn!(node_index = index, node_label = %node.label, %error, "plan node errored");
        PlanError::Core(error)
    })?;
    debug!(
        node_index = index,
        node_label = %node.label,
        cache = ?outcome.cache,
        exit_code = outcome.result.exit_code,
        "plan node completed"
    );
    if outcome.result.exit_code != 0 {
        return Err(PlanError::Failed {
            label: node.label.clone(),
            exit_code: outcome.result.exit_code,
        });
    }
    outcomes[index] = Some(PlanOutcome {
        index,
        label: node.label.clone(),
        outcome,
    });
    if schedule_dependents {
        for &dependent in &topology.dependents[index] {
            topology.remaining[dependent] -= 1;
            if topology.remaining[dependent] == 0 {
                ready.push_back(dependent);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use struo_cas::{ActionResult, Digest};
    use tokio::sync::{mpsc, Semaphore};

    use super::*;
    use crate::{Action, CacheState, WorkspacePath};

    fn plan(count: usize) -> Plan {
        Plan {
            nodes: (0..count)
                .map(|index| PlanNode {
                    label: index.to_string(),
                    action: Action::WriteFile {
                        path: WorkspacePath::try_from("out").unwrap(),
                        bytes: Vec::new(),
                        input_digest: None,
                    },
                    deps: Vec::new(),
                })
                .collect(),
        }
    }

    fn outcome(exit_code: i32) -> Outcome {
        Outcome {
            action: Digest::of_bytes(b"test"),
            result: ActionResult {
                exit_code,
                stdout: None,
                stderr: None,
                outputs: BTreeMap::new(),
            },
            cache: CacheState::Miss,
        }
    }

    #[tokio::test]
    async fn a_panicking_worker_returns_an_error_without_hanging() {
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            run(&plan(1), 1, |_| async {
                panic!("worker panic");
            }),
        )
        .await
        .expect("scheduler must observe worker panics");
        assert!(matches!(result, Err(PlanError::Task(error)) if error.is_panic()));
    }

    #[tokio::test]
    async fn a_wide_plan_uses_a_bounded_parallel_window() {
        let gate = Arc::new(Semaphore::new(0));
        let (started, mut starts) = mpsc::unbounded_channel();
        let worker_gate = Arc::clone(&gate);
        let task = tokio::spawn(async move {
            run(&plan(20), 3, |node| {
                let gate = Arc::clone(&worker_gate);
                let started = started.clone();
                let label = node.label.clone();
                async move {
                    started.send(label).unwrap();
                    gate.acquire().await.unwrap().forget();
                    Ok(outcome(0))
                }
            })
            .await
        });
        for _ in 0..3 {
            tokio::time::timeout(Duration::from_secs(5), starts.recv())
                .await
                .unwrap()
                .unwrap();
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(50), starts.recv())
                .await
                .is_err()
        );
        gate.add_permits(1);
        tokio::time::timeout(Duration::from_secs(5), starts.recv())
            .await
            .unwrap()
            .unwrap();
        gate.add_permits(19);
        let outcomes = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(outcomes.len(), 20);
        assert!(outcomes
            .iter()
            .enumerate()
            .all(|(index, outcome)| outcome.index == index));
    }

    #[tokio::test]
    async fn a_host_task_starts_while_a_remote_task_waits_for_admission() {
        use crate::{
            Executor, ExecutorResources, ResourceLimits, ResourcePool, ResourceRequest, RunOpts,
            Runner,
        };
        let workspace = tempfile::tempdir().unwrap();
        let resources =
            ExecutorResources::new(Arc::new(ResourcePool::new(ResourceLimits::new(1, 1 << 30))))
                .with_remote_concurrency(1);
        let held = resources
            .acquire(Executor::E2b, &ResourceRequest::default())
            .await;
        let runner = Runner::with_cache(
            struo_cas::CacheProvider::open_local(workspace.path().join("cache")),
            workspace.path(),
            RunOpts::default(),
        )
        .with_executor_resources(resources.clone());
        assert_eq!(runner.max_parallel_plan_tasks(), 2);
        let (started, mut starts) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            run(&plan(2), runner.max_parallel_plan_tasks(), |node| {
                let resources = resources.clone();
                let started = started.clone();
                let label = node.label.clone();
                async move {
                    let executor = if label == "0" {
                        Executor::E2b
                    } else {
                        Executor::Local
                    };
                    let _permit = resources
                        .acquire(executor, &ResourceRequest::default())
                        .await;
                    started.send(label).unwrap();
                    Ok(outcome(0))
                }
            })
            .await
        });
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), starts.recv())
                .await
                .unwrap()
                .unwrap(),
            "1"
        );
        drop(held);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), starts.recv())
                .await
                .unwrap()
                .unwrap(),
            "0"
        );
        assert_eq!(task.await.unwrap().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn failure_drains_running_work_without_starting_queued_nodes() {
        let finished = Arc::new(AtomicUsize::new(0));
        let starts = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Semaphore::new(0));
        let result = run(&plan(10), 2, |node| {
            starts.fetch_add(1, Ordering::SeqCst);
            let gate = Arc::clone(&gate);
            let finished = Arc::clone(&finished);
            let fail = node.label == "0";
            async move {
                if fail {
                    gate.add_permits(1);
                    Ok(outcome(7))
                } else {
                    gate.acquire().await.unwrap().forget();
                    finished.fetch_add(1, Ordering::SeqCst);
                    Ok(outcome(0))
                }
            }
        })
        .await;
        assert!(matches!(
            result,
            Err(PlanError::Failed { exit_code: 7, .. })
        ));
        assert_eq!(starts.load(Ordering::SeqCst), 2);
        assert_eq!(finished.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_core_error_stops_dependents() {
        let mut plan = plan(2);
        plan.nodes[1].deps = vec![0];
        let mut starts = 0;
        let result = run(&plan, 2, |_| {
            starts += 1;
            async { Err(crate::Error::EmptyArgv) }
        })
        .await;
        assert!(matches!(
            result,
            Err(PlanError::Core(crate::Error::EmptyArgv))
        ));
        assert_eq!(starts, 1);
    }
}
