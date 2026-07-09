use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use once_cas::{CacheProvider, Digest};
use once_core::SandboxMode;
use once_frontend::analysis::AnalysisEngine;
use once_frontend::GraphTarget;
use serde_json::Value as JsonValue;
use tokio::task::JoinSet;

use super::{build_one, BuildOutcome};

pub(super) struct BuildScheduler<'a> {
    root_id: &'a str,
    workspace: &'a Path,
    cache: &'a CacheProvider,
    targets: &'a HashMap<String, Arc<GraphTarget>>,
    analyzer: &'a AnalysisEngine,
    module_source_digest: Digest,
    reachable: &'a HashSet<String>,
    retained: &'a HashSet<String>,
    sandbox: SandboxMode,
}

impl<'a> BuildScheduler<'a> {
    pub(super) fn new(
        root_id: &'a str,
        workspace: &'a Path,
        cache: &'a CacheProvider,
        targets: &'a HashMap<String, Arc<GraphTarget>>,
        analyzer: &'a AnalysisEngine,
        reachable: &'a HashSet<String>,
        retained: &'a HashSet<String>,
        sandbox: SandboxMode,
    ) -> Self {
        let module_source_digest = Digest::of_bytes(analyzer.module_source().as_bytes());
        Self {
            root_id,
            workspace,
            cache,
            targets,
            analyzer,
            module_source_digest,
            reachable,
            retained,
            sandbox,
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
            tracing::debug!(
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
        while let Some(target_id) = state.ready.pop_front() {
            let target = Arc::clone(
                self.targets
                    .get(&target_id)
                    .with_context(|| format!("target `{target_id}` vanished from graph"))?,
            );
            let inputs = state.dependency_inputs(&target, self.reachable)?;
            tracing::debug!(
                target = %target_id,
                deps = inputs.providers.len(),
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
                inputs.action_digests,
                self.sandbox,
            ));
        }
        Ok(())
    }
}

struct DependencyInputs {
    providers: Vec<JsonValue>,
    action_digests: Vec<(String, Digest)>,
}

struct BuildState {
    remaining_deps: HashMap<String, usize>,
    dependents: HashMap<String, Vec<String>>,
    remaining_readers: HashMap<String, usize>,
    ready: VecDeque<String>,
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
                .deps
                .iter()
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

        let ready = remaining_deps
            .iter()
            .filter_map(|(target_id, count)| (*count == 0).then_some(target_id.clone()))
            .collect();

        Ok(Self {
            remaining_deps,
            dependents,
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
                    self.ready.push_back(next_id.clone());
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
        let mut action_digests = Vec::new();
        for dep_id in target
            .deps
            .iter()
            .filter(|dep_id| reachable.contains(*dep_id))
        {
            let (provider, action_digest) = self.read_dependency(dep_id)?;
            providers.push(provider);
            action_digests.push((dep_id.clone(), action_digest));
        }
        Ok(DependencyInputs {
            providers,
            action_digests,
        })
    }

    fn read_dependency(&mut self, dep_id: &str) -> Result<(JsonValue, Digest)> {
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
            Ok((outcome.provider, outcome.action_digest))
        } else {
            let outcome = self
                .outcomes
                .get(dep_id)
                .with_context(|| format!("missing build outcome for dependency `{dep_id}`"))?;
            Ok((outcome.provider.clone(), outcome.action_digest))
        }
    }
}
