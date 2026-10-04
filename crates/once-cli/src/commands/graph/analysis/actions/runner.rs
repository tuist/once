use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context, Result};
use once_cas::{CacheProvider, Digest};
use once_core::{ActionOutputObserver, ResourcePool, ResourceRequest, SandboxMode};
use once_frontend::analysis::{AnalysisResult, DeclaredActionOperation};
use once_frontend::GraphTarget;

use super::{
    expose_target_tools, materialize_available_inputs, run_declared_action, scheduling,
    AvailableInput, BuildOutcome, DeclaredActionRun, DeclaredActionsState, SourceDigestCache,
    EVIDENCE_FLUSH_BATCH_SIZE,
};

/// Materialise each declared action through the action cache, then
/// fold the analysis provider directly into the build outcome.
///
/// Returns a boxed future intentionally because the concrete future
/// captures declared action state and cache execution state. Boxing at
/// this boundary keeps parent graph futures small enough for
/// `clippy::large_futures` and centralizes the allocation.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(in crate::commands::graph::analysis) fn run_declared_actions<'a>(
    analyzer: Option<&'a once_frontend::analysis::AnalysisEngine>,
    workspace: &'a Path,
    cache: &'a CacheProvider,
    module_source_digest: Digest,
    target: &'a GraphTarget,
    capability: &'a str,
    analysis: AnalysisResult,
    dep_action_digests: &'a [(String, Digest)],
    dependency_inputs: &'a BTreeMap<String, AvailableInput>,
    tool_paths: &'a BTreeMap<String, String>,
    source_digest_cache: Option<&'a SourceDigestCache>,
    sandbox: SandboxMode,
    resources: &'a Arc<ResourcePool>,
    output_observer: Option<&'a dyn ActionOutputObserver>,
    event_bus: Option<&'a once_core::RunEventBus>,
    workers: &'a crate::bus_events::WorkerSlots,
) -> Pin<Box<dyn Future<Output = Result<BuildOutcome>> + Send + 'a>> {
    Box::pin(async move {
        let AnalysisResult {
            mut actions,
            provider,
            ..
        } = analysis;
        expose_target_tools(workspace, target, &mut actions, Some(tool_paths)).await?;
        tracing::trace!(
            target = %target.label.id,
            declared_actions = actions.len(),
            dep_action_digests = dep_action_digests.len(),
            "running declared graph actions"
        );
        let mut state = DeclaredActionsState::new(dependency_inputs);
        // A single-action target is fully represented by the caller's
        // capability-level record. Multi-action targets need per-action
        // success evidence so individual streams and outputs stay visible.
        let record_success_evidence = actions.len() > 1
            || actions.iter().any(|action| {
                matches!(
                    action.operation,
                    Some(DeclaredActionOperation::ExpandActions { .. })
                )
            });

        let mut pending = VecDeque::from(actions);
        let mut planner = analyzer.cloned();
        let mut index = 0;
        while let Some(declared) = pending.pop_front() {
            if matches!(
                declared.operation,
                Some(DeclaredActionOperation::ExpandActions { .. })
            ) {
                let permit = resources.acquire(ResourceRequest::default()).await;
                materialize_available_inputs(
                    workspace,
                    cache,
                    &declared,
                    &state.available_inputs,
                    source_digest_cache,
                )
                .await?;
                if planner.is_none() {
                    anyhow::bail!("deferred actions require the target's analysis engine");
                }
                let engine = planner.take().context("missing action planner")?;
                let expansion_target = target.clone();
                let expansion_workspace = workspace.to_path_buf();
                let (engine, expansion) = tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    let result =
                        engine.expand_actions(&expansion_target, &expansion_workspace, &declared);
                    (engine, result)
                })
                .await?;
                planner = Some(engine);
                let expansion = expansion?;
                let mut expanded = expansion.actions;
                expose_target_tools(workspace, target, &mut expanded, Some(tool_paths)).await?;
                for action in expanded.into_iter().rev() {
                    pending.push_front(action);
                }
                continue;
            }
            let batch = scheduling::take_batch(
                declared,
                &mut pending,
                resources.limits().max_parallel_actions(),
            );
            let input_digests = batch
                .iter()
                .map(|action| state.input_action_digests(action, dep_action_digests))
                .collect::<Vec<_>>();
            if batch.len() > 1 {
                let _permit = resources.acquire(ResourceRequest::default()).await;
                for action in &batch {
                    materialize_available_inputs(
                        workspace,
                        cache,
                        action,
                        &state.available_inputs,
                        source_digest_cache,
                    )
                    .await?;
                }
            }
            for action in &batch {
                state.outputs.extend(action.outputs.iter().cloned());
            }
            let count = batch.len();
            let prior_cached_results = if count == 1 {
                state.cached_results.as_slice()
            } else {
                &[]
            };
            let outcomes =
                futures::future::join_all(batch.into_iter().zip(&input_digests).enumerate().map(
                    |(offset, (declared, input_action_digests))| {
                        let available_inputs = &state.available_inputs;
                        Box::pin(async move {
                            let worker = workers.acquire().await;
                            let start_at = crate::bus_events::now_ms();
                            let started = std::time::Instant::now();
                            let identifier = declared.identifier.clone();
                            let action_index = u32::try_from(index + offset).unwrap_or(u32::MAX);
                            if let Some(bus) = event_bus {
                                crate::bus_events::action_attempt_started(
                                    bus,
                                    &target.label.id,
                                    capability,
                                    action_index,
                                    worker.worker_id(),
                                );
                            }
                            let outcome = run_declared_action(DeclaredActionRun {
                                workspace,
                                cache,
                                module_source_digest,
                                target_id: &target.label.id,
                                capability,
                                index: index + offset,
                                declared,
                                input_action_digests,
                                available_inputs,
                                source_digest_cache,
                                prior_cached_results,
                                record_success_evidence,
                                sandbox,
                                resources,
                                output_observer,
                            })
                            .await;
                            let duration_ms =
                                u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                            let (exit_code, was_cached, prepare_ms, execute_ms, cache_key) =
                                match &outcome {
                                    Ok(outcome) => (
                                        outcome.result.exit_code,
                                        outcome.cache_state == once_core::EvidenceCacheState::Hit,
                                        outcome.prepare_ms,
                                        outcome.execute_ms,
                                        outcome.digest.to_string(),
                                    ),
                                    Err(error) => error
                                        .downcast_ref::<super::ActionExecutionFailure>()
                                        .map_or((1, false, 0, 0, String::new()), |failure| {
                                            (
                                                failure.exit_code,
                                                failure.was_cached,
                                                0,
                                                0,
                                                String::new(),
                                            )
                                        }),
                                };
                            if let Some(bus) = event_bus {
                                crate::bus_events::action_attempt_completed(
                                    bus,
                                    &target.label.id,
                                    capability,
                                    action_index,
                                    duration_ms,
                                    was_cached,
                                    exit_code,
                                );
                                crate::bus_events::action_completed(
                                    bus,
                                    &target.label.id,
                                    capability,
                                    action_index,
                                    identifier.as_deref(),
                                    duration_ms,
                                    was_cached,
                                    exit_code,
                                    start_at,
                                    worker.worker_id(),
                                    prepare_ms,
                                    execute_ms,
                                    &cache_key,
                                    1,
                                );
                            }
                            outcome.map(|outcome| {
                                let retained = super::PerActionOutcome {
                                    action_digest: outcome.digest,
                                    identifier,
                                    index: action_index,
                                    cache_state: outcome.cache_state,
                                    duration_ms: i64::try_from(duration_ms).unwrap_or(i64::MAX),
                                    exit_code,
                                };
                                (outcome, retained)
                            })
                        })
                    },
                ))
                .await;
            let mut failure = None;
            for outcome in outcomes {
                match outcome {
                    Ok((outcome, retained)) => {
                        state.record(outcome, !record_success_evidence, retained);
                    }
                    Err(error) if failure.is_none() => failure = Some(error),
                    Err(_) => {}
                }
            }
            if let Some(error) = failure {
                let records = state.take_evidence_records();
                crate::commands::evidence::append_records(workspace, &records).await;
                return Err(error);
            }
            index += count;
            if state.evidence_records.len() >= EVIDENCE_FLUSH_BATCH_SIZE {
                let records = state.take_evidence_records();
                crate::commands::evidence::append_records(workspace, &records).await;
            }
        }

        let records = state.take_evidence_records();
        crate::commands::evidence::append_records(workspace, &records).await;
        Ok(state.finish(&target.label.id, provider))
    })
}
