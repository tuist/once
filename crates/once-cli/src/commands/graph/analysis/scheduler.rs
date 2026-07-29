use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use once_cas::{CacheProvider, Digest};
use once_core::{ResourcePool, SandboxMode};
use once_frontend::analysis::AnalysisEngine;
use once_frontend::GraphTarget;
use serde_json::Value as JsonValue;
use tokio::task::JoinSet;

use super::source_digest_cache::SourceDigestCache;
use super::{build_one, materialize_cached_outputs, AvailableInput, BuildOutcome};

pub(super) struct BuildScheduler<'a> {
    root_id: &'a str,
    workspace: &'a Path,
    cache: &'a CacheProvider,
    targets: &'a HashMap<String, Arc<GraphTarget>>,
    analyzer: &'a AnalysisEngine,
    tool_paths: &'a Arc<BTreeMap<String, String>>,
    source_digest_cache: &'a SourceDigestCache,
    module_source_digest: Digest,
    reachable: &'a HashSet<String>,
    retained: &'a HashSet<String>,
    sandbox: SandboxMode,
    resources: &'a Arc<ResourcePool>,
    /// Ceiling on build tasks in flight at once. A wide graph can have
    /// thousands of independently-ready targets; spawning a task (and its
    /// subprocess) for every one at once is how a build system exhausts
    /// memory and file descriptors. Ready targets past this many wait in
    /// the queue until a slot frees. Action-level CPU concurrency is
    /// bounded separately by the core runner's resource pool.
    max_in_flight: usize,
}

impl<'a> BuildScheduler<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        root_id: &'a str,
        workspace: &'a Path,
        cache: &'a CacheProvider,
        targets: &'a HashMap<String, Arc<GraphTarget>>,
        analyzer: &'a AnalysisEngine,
        tool_paths: &'a Arc<BTreeMap<String, String>>,
        source_digest_cache: &'a SourceDigestCache,
        reachable: &'a HashSet<String>,
        retained: &'a HashSet<String>,
        sandbox: SandboxMode,
        resources: &'a Arc<ResourcePool>,
    ) -> Self {
        let module_source_digest = Digest::of_bytes(analyzer.module_source().as_bytes());
        let max_in_flight = resources.max_parallel_actions();
        Self {
            root_id,
            workspace,
            cache,
            targets,
            analyzer,
            tool_paths,
            source_digest_cache,
            module_source_digest,
            reachable,
            retained,
            sandbox,
            resources,
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
            materialize_cached_outputs(
                &outcome,
                self.workspace,
                self.cache,
                Some(self.source_digest_cache),
            )
            .await
            .with_context(|| format!("materializing outputs for {target_id}"))?;
            tracing::trace!(
                target = %target_id,
                cache = outcome.cache_tag,
                outputs = outcome.outputs.len(),
                completed = state.completed + 1,
                total = self.reachable.len(),
                "completed graph target build task"
            );
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
            tracing::trace!(
                target = %target_id,
                deps = inputs.providers.len(),
                critical_depth,
                running_after_spawn = running.len() + 1,
                "spawning graph target build task"
            );

            running.spawn(build_one(
                self.workspace.to_path_buf(),
                self.cache.clone(),
                self.analyzer.clone(),
                self.module_source_digest,
                target,
                inputs.providers,
                inputs.providers_by_role,
                inputs.action_digests,
                inputs.available_inputs,
                Arc::clone(self.tool_paths),
                self.source_digest_cache.clone(),
                self.sandbox,
                Arc::clone(self.resources),
            ));
        }
        Ok(())
    }
}

struct DependencyInputs {
    providers: Vec<Arc<JsonValue>>,
    providers_by_role: BTreeMap<String, Vec<Arc<JsonValue>>>,
    action_digests: Vec<(String, Digest)>,
    available_inputs: BTreeMap<String, AvailableInput>,
}

struct BuildState {
    remaining_deps: HashMap<String, usize>,
    dependents: HashMap<String, Vec<String>>,
    critical_depths: HashMap<String, usize>,
    remaining_readers: HashMap<String, usize>,
    ready: BinaryHeap<(usize, Reverse<String>)>,
    outcomes: HashMap<String, BuildOutcome>,
    completed: usize,
}

impl BuildState {
    fn new(
        targets: &HashMap<String, Arc<GraphTarget>>,
        reachable: &HashSet<String>,
        retained: &HashSet<String>,
    ) -> Result<Self> {
        let mut remaining_deps = HashMap::new();
        let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
        for target_id in reachable {
            let target = targets
                .get(target_id)
                .with_context(|| format!("target `{target_id}` vanished from graph"))?;
            let mut dep_count = 0;
            for dep_id in target
                .dependency_ids()
                .filter(|dep_id| reachable.contains(*dep_id))
            {
                dep_count += 1;
                dependents
                    .entry(dep_id.clone())
                    .or_default()
                    .push(target_id.clone());
            }
            remaining_deps.insert(target_id.clone(), dep_count);
        }

        let mut remaining_readers = dependents
            .iter()
            .map(|(target_id, deps)| (target_id.clone(), deps.len()))
            .collect::<HashMap<_, _>>();
        for target_id in retained {
            *remaining_readers.entry(target_id.clone()).or_default() += 1;
        }

        let critical_depths = critical_depths(reachable, &dependents)?;
        let ready = remaining_deps
            .iter()
            .filter(|(_, count)| **count == 0)
            .map(|(target_id, _)| {
                (
                    critical_depths.get(target_id).copied().unwrap_or(1),
                    Reverse(target_id.clone()),
                )
            })
            .collect();

        Ok(Self {
            remaining_deps,
            dependents,
            critical_depths,
            remaining_readers,
            ready,
            outcomes: HashMap::new(),
            completed: 0,
        })
    }

    fn record_completion(&mut self, target_id: &str, outcome: BuildOutcome) -> Result<()> {
        self.outcomes.insert(target_id.to_string(), outcome);
        self.completed += 1;

        if let Some(next_targets) = self.dependents.get(target_id) {
            for next_id in next_targets {
                let remaining = self
                    .remaining_deps
                    .get_mut(next_id)
                    .with_context(|| format!("missing dependency count for `{next_id}`"))?;
                *remaining -= 1;
                if *remaining == 0 {
                    self.ready.push((
                        self.critical_depths.get(next_id).copied().unwrap_or(1),
                        Reverse(next_id.clone()),
                    ));
                }
            }
        }

        Ok(())
    }

    fn dependency_inputs(
        &mut self,
        target: &GraphTarget,
        reachable: &HashSet<String>,
    ) -> Result<DependencyInputs> {
        let mut providers = Vec::new();
        let mut providers_by_role = BTreeMap::new();
        let mut action_digests = Vec::new();
        let mut available_inputs = BTreeMap::new();
        for dep_id in target
            .deps
            .iter()
            .filter(|dep_id| reachable.contains(*dep_id))
        {
            let (provider, action_digest, inputs) = self.read_dependency(dep_id)?;
            providers.push(provider);
            action_digests.push((dep_id.clone(), action_digest));
            available_inputs.extend(inputs.into_iter().map(|(path, mut input)| {
                input.same_target = false;
                (path, input)
            }));
        }
        for (role, dep_ids) in &target.dependency_edges {
            let mut role_providers = Vec::new();
            for dep_id in dep_ids.iter().filter(|dep_id| reachable.contains(*dep_id)) {
                let (provider, action_digest, inputs) = self.read_dependency(dep_id)?;
                role_providers.push(provider);
                action_digests.push((dep_id.clone(), action_digest));
                available_inputs.extend(inputs.into_iter().map(|(path, mut input)| {
                    input.same_target = false;
                    (path, input)
                }));
            }
            providers_by_role.insert(role.clone(), role_providers);
        }
        Ok(DependencyInputs {
            providers,
            providers_by_role,
            action_digests,
            available_inputs,
        })
    }

    fn read_dependency(
        &mut self,
        dep_id: &str,
    ) -> Result<(Arc<JsonValue>, Digest, BTreeMap<String, AvailableInput>)> {
        let remaining = self
            .remaining_readers
            .get_mut(dep_id)
            .with_context(|| format!("missing reader count for `{dep_id}`"))?;
        *remaining = remaining
            .checked_sub(1)
            .with_context(|| format!("dependency `{dep_id}` had no remaining readers"))?;

        if *remaining == 0 {
            let outcome = self
                .outcomes
                .remove(dep_id)
                .with_context(|| format!("missing build outcome for dependency `{dep_id}`"))?;
            Ok((
                outcome.provider,
                outcome.action_digest,
                outcome.available_inputs,
            ))
        } else {
            let outcome = self
                .outcomes
                .get(dep_id)
                .with_context(|| format!("missing build outcome for dependency `{dep_id}`"))?;
            Ok((
                Arc::clone(&outcome.provider),
                outcome.action_digest,
                outcome.available_inputs.clone(),
            ))
        }
    }
}

fn critical_depths(
    reachable: &HashSet<String>,
    dependents: &HashMap<String, Vec<String>>,
) -> Result<HashMap<String, usize>> {
    let mut depths = HashMap::with_capacity(reachable.len());
    let mut visiting = HashSet::new();
    for target_id in reachable {
        critical_depth(target_id, reachable, dependents, &mut depths, &mut visiting)?;
    }
    Ok(depths)
}

fn critical_depth(
    target_id: &str,
    reachable: &HashSet<String>,
    dependents: &HashMap<String, Vec<String>>,
    depths: &mut HashMap<String, usize>,
    visiting: &mut HashSet<String>,
) -> Result<usize> {
    if let Some(depth) = depths.get(target_id) {
        return Ok(*depth);
    }
    if !visiting.insert(target_id.to_string()) {
        anyhow::bail!("cycle detected while ranking graph target `{target_id}`");
    }
    let mut depth = 1;
    if let Some(next_targets) = dependents.get(target_id) {
        for next_id in next_targets
            .iter()
            .filter(|next_id| reachable.contains(*next_id))
        {
            depth = depth.max(
                critical_depth(next_id, reachable, dependents, depths, visiting)?.saturating_add(1),
            );
        }
    }
    visiting.remove(target_id);
    depths.insert(target_id.to_string(), depth);
    Ok(depth)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn critical_depth_prioritizes_the_longest_remaining_chain() {
        let reachable = ["root", "short", "long-a", "long-b"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let dependents = HashMap::from([
            ("short".to_string(), vec!["root".to_string()]),
            ("long-a".to_string(), vec!["long-b".to_string()]),
            ("long-b".to_string(), vec!["root".to_string()]),
        ]);

        let depths = critical_depths(&reachable, &dependents).unwrap();

        assert_eq!(depths["root"], 1);
        assert_eq!(depths["short"], 2);
        assert_eq!(depths["long-b"], 2);
        assert_eq!(depths["long-a"], 3);
    }

    #[test]
    fn critical_depth_rejects_cycles() {
        let reachable = ["a", "b"].into_iter().map(str::to_string).collect();
        let dependents = HashMap::from([
            ("a".to_string(), vec!["b".to_string()]),
            ("b".to_string(), vec!["a".to_string()]),
        ]);

        assert!(critical_depths(&reachable, &dependents).is_err());
    }
}
