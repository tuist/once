use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use once_frontend::GraphTarget;
use tokio::task::JoinSet;

use super::{build_one, materialize_cached_outputs, AvailableInput, BuildContext, BuildOutcome};
use state::BuildState;

mod ranking;
mod state;

pub(super) use state::DependencyInputs;

pub(super) struct BuildScheduler<'a> {
    root_id: &'a str,
    context: BuildContext,
    targets: &'a HashMap<String, Arc<GraphTarget>>,
    reachable: &'a HashSet<String>,
    retained: &'a HashSet<String>,
    uncached_work: &'a AtomicBool,
    /// Ready targets beyond this window wait without allocating task state.
    /// Action admission separately bounds processes, memory, and named slots.
    max_in_flight: usize,
}

impl<'a> BuildScheduler<'a> {
    pub(super) fn new(
        root_id: &'a str,
        context: BuildContext,
        targets: &'a HashMap<String, Arc<GraphTarget>>,
        reachable: &'a HashSet<String>,
        retained: &'a HashSet<String>,
        uncached_work: &'a AtomicBool,
    ) -> Self {
        let max_in_flight = context.resources.max_parallel_actions();
        Self {
            root_id,
            context,
            targets,
            reachable,
            retained,
            uncached_work,
            max_in_flight,
        }
    }

    pub(super) async fn run(self) -> Result<HashMap<String, BuildOutcome>> {
        let mut state = BuildState::new(self.targets, self.reachable, self.retained)?;
        let mut running = JoinSet::new();

        self.spawn_ready(&mut state, &mut running)?;
        while state.completed < self.reachable.len() {
            if running.is_empty() {
                anyhow::bail!(
                    "cycle detected while building graph target `{}`",
                    self.root_id
                );
            }

            let joined = running
                .join_next()
                .await
                .context("build task set ended unexpectedly")?;
            let (target_id, outcome) = joined.context("joining graph build task")??;
            tracing::trace!(
                target = %target_id,
                cache = outcome.cache_tag,
                outputs = outcome.outputs.len(),
                completed = state.completed + 1,
                total = self.reachable.len(),
                "completed graph target build task"
            );
            if super::has_uncached_actions(outcome.cache_state, &outcome.per_action_outcomes) {
                self.uncached_work.store(true, Ordering::Relaxed);
            }
            state.record_completion(&target_id, outcome)?;
            self.spawn_ready(&mut state, &mut running)?;
        }

        Ok(state.outcomes)
    }

    fn spawn_ready(
        &self,
        state: &mut BuildState,
        running: &mut JoinSet<Result<(String, BuildOutcome)>>,
    ) -> Result<()> {
        while running.len() < self.max_in_flight {
            let Some((critical_depth, Reverse(target_id))) = state.ready.pop() else {
                break;
            };
            let target = Arc::clone(
                self.targets
                    .get(&target_id)
                    .with_context(|| format!("target `{target_id}` vanished from graph"))?,
            );
            let inputs = state.dependency_inputs(&target, self.reachable)?;
            if target_id != self.root_id
                && !self
                    .context
                    .suppressed_target_lifecycle
                    .contains(&target_id)
            {
                if let Some(bus) = &self.context.event_bus {
                    crate::bus_events::target_queued(bus, &target_id);
                }
            }
            tracing::trace!(
                target = %target_id,
                deps = inputs.providers.len(),
                critical_depth,
                running_after_spawn = running.len() + 1,
                "spawning graph target build task"
            );

            let context = self.context.clone();
            let build = build_one(self.context.clone(), target, inputs);
            // Restoration stays in the task so unrelated completions can be joined.
            running.spawn(async move {
                let bus = context.event_bus.clone();
                let finish = Box::pin(async {
                    let (target_id, outcome) = build.await?;
                    materialize_completed(&context, &target_id, &outcome).await?;
                    Ok((target_id, outcome))
                });
                crate::bus_events::observe_target_transfers(bus.as_ref(), &target_id, finish).await
            });
        }
        Ok(())
    }
}

async fn materialize_completed(
    context: &BuildContext,
    target_id: &str,
    outcome: &BuildOutcome,
) -> Result<()> {
    let started_at_ms = crate::bus_events::now_ms();
    let started_at = std::time::Instant::now();
    materialize_cached_outputs(outcome, context.repair())
        .await
        .with_context(|| format!("materializing outputs for {target_id}"))?;
    if let Some(bus) = &context.event_bus {
        // Reporting acquires a lane only after restoration has finished.
        let worker = context.workers.acquire().await;
        crate::bus_events::target_phase_completed(
            bus,
            target_id,
            "materialize_outputs",
            worker.worker_id(),
            started_at_ms,
            u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
        );
    }
    Ok(())
}
