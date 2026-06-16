//! Walks the graph in dependency order and executes declared actions
//! for analysis-backed rules.
//!
//! For every target the driver calls [`once_frontend::analysis::analyze_target`].
//! When the rule has an `impl` callable the analysis returns a list of
//! `DeclaredAction`s plus a provider record; we materialise each
//! declared command according to its cache policy and pass the resulting
//! provider down to consumers. When the rule has no `impl` declared in
//! the prelude, the driver returns `None` so the caller can fall back to
//! its generic marker action.
//!
//! This module has no Apple-specific logic: it consults the prelude
//! via `rule_has_impl` to know which kinds run through analysis, and
//! the analysis layer is fed everything it needs through generic
//! starlark globals. Dep providers and dep action digests are carried
//! Buck2/Bazel-style so a parent's input digest composes its deps'
//! action digests.

mod actions;
mod scheduler;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use once_cas::{CacheProvider, Digest};
use once_frontend::analysis::AnalysisEngine;
use once_frontend::GraphTarget;
use serde_json::Value as JsonValue;

use self::actions::run_declared_actions;
use self::scheduler::BuildScheduler;

/// Per-target outcome cached during a single command invocation.
///
/// Deliberately not `Clone`: each outcome has exactly one owner at a
/// time: first the producing build task, then `outcomes` in
/// [`BuildSession::build_reachable`], and then either the
/// `dep_providers` of the last reader (move) or the returning caller
/// (also a move via `outcomes.remove`).
#[derive(Debug)]
pub(super) struct BuildOutcome {
    pub provider: JsonValue,
    pub action_digest: Digest,
    pub outputs: Vec<String>,
    pub cache_tag: &'static str,
}

/// Command-scoped graph build session.
///
/// The session owns the target id map and analysis engine so one graph
/// command does not repeatedly parse rule metadata or linearly scan the
/// graph for every dependency edge.
pub(super) struct BuildSession {
    workspace: PathBuf,
    cache: CacheProvider,
    /// Graph targets are wrapped in `Arc` so spawned tasks can hold a
    /// cheap shared handle (refcount bump) instead of deep-cloning the
    /// whole `GraphTarget` (which itself owns several `Vec`s and a
    /// `BTreeMap`) once for the analysis task and again for action
    /// execution.
    targets: HashMap<String, Arc<GraphTarget>>,
    analyzer: AnalysisEngine,
}

impl BuildSession {
    pub(super) fn new(
        workspace: &Path,
        cache: &CacheProvider,
        graph: &[GraphTarget],
    ) -> Result<Self> {
        Ok(Self::new_with_analyzer(
            workspace,
            cache,
            graph,
            AnalysisEngine::new()?,
        ))
    }

    fn new_with_analyzer(
        workspace: &Path,
        cache: &CacheProvider,
        graph: &[GraphTarget],
        analyzer: AnalysisEngine,
    ) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            cache: cache.clone(),
            targets: graph
                .iter()
                .map(|target| (target.label.id.clone(), Arc::new(target.clone())))
                .collect(),
            analyzer,
        }
    }

    /// Build a target and the impl-backed portion of its dependency
    /// closure. Returns `Ok(None)` when the target's own rule has no
    /// impl, allowing callers to fall back to generic marker actions.
    pub(super) async fn build_with_analysis(
        &self,
        target: &GraphTarget,
    ) -> Result<Option<BuildOutcome>> {
        if !self.analyzer.rule_has_impl(&target.kind) {
            return Ok(None);
        }

        let reachable = self.reachable_analysis_targets(target);
        let outcome = self.build_reachable(&target.label.id, &reachable).await?;
        Ok(Some(outcome))
    }

    pub(super) async fn run_with_analysis(
        &self,
        target: &GraphTarget,
        capability: &str,
    ) -> Result<Option<BuildOutcome>> {
        if !self.analyzer.rule_has_impl(&target.kind) {
            return Ok(None);
        }

        let dep_outcomes = self.build_direct_analysis_deps(target).await?;
        let mut dep_providers = Vec::with_capacity(dep_outcomes.len());
        let mut dep_action_digests = Vec::with_capacity(dep_outcomes.len());
        for (dep_id, outcome) in dep_outcomes {
            dep_action_digests.push((dep_id, outcome.action_digest));
            dep_providers.push(outcome.provider);
        }

        let analysis = self
            .analyzer
            .analyze_target_capability(target, &self.workspace, &dep_providers, capability)
            .with_context(|| format!("analysing {}", target.label.id))?;
        let outcome = run_declared_actions(
            &self.workspace,
            &self.cache,
            target,
            analysis,
            &dep_action_digests,
        )
        .await
        .with_context(|| format!("executing {capability} for {}", target.label.id))?;
        Ok(Some(outcome))
    }

    fn reachable_analysis_targets(&self, target: &GraphTarget) -> HashSet<String> {
        self.reachable_analysis_targets_from([target.label.id.clone()])
    }

    fn reachable_analysis_deps(&self, target: &GraphTarget) -> HashSet<String> {
        let roots = target.deps.iter().filter_map(|dep_id| {
            let dep = self.targets.get(dep_id)?;
            self.analyzer
                .rule_has_impl(&dep.kind)
                .then(|| dep_id.clone())
        });
        self.reachable_analysis_targets_from(roots)
    }

    fn reachable_analysis_targets_from(
        &self,
        roots: impl IntoIterator<Item = String>,
    ) -> HashSet<String> {
        let mut reachable = HashSet::new();
        let mut stack = roots.into_iter().collect::<Vec<_>>();
        while let Some(target_id) = stack.pop() {
            // Check membership before insert so the owned `target_id`
            // moves into the set instead of being cloned.
            if reachable.contains(&target_id) {
                continue;
            }
            let target = self.targets.get(&target_id).cloned();
            reachable.insert(target_id);
            let Some(target) = target else {
                continue;
            };
            for dep_id in &target.deps {
                let Some(dep) = self.targets.get(dep_id) else {
                    continue;
                };
                if self.analyzer.rule_has_impl(&dep.kind) {
                    stack.push(dep_id.clone());
                }
            }
        }
        reachable
    }

    async fn build_direct_analysis_deps(
        &self,
        target: &GraphTarget,
    ) -> Result<Vec<(String, BuildOutcome)>> {
        let reachable = self.reachable_analysis_deps(target);
        if reachable.is_empty() {
            return Ok(Vec::new());
        }

        let direct_deps = target
            .deps
            .iter()
            .filter(|dep_id| reachable.contains(*dep_id))
            .cloned()
            .collect::<Vec<_>>();
        let retained = direct_deps.iter().cloned().collect::<HashSet<_>>();
        let mut outcomes = self
            .build_reachable_retaining(&target.label.id, &reachable, &retained)
            .await?;
        direct_deps
            .into_iter()
            .map(|dep_id| {
                let outcome = outcomes
                    .remove(&dep_id)
                    .with_context(|| format!("missing build outcome for dependency `{dep_id}`"))?;
                Ok((dep_id, outcome))
            })
            .collect()
    }

    async fn build_reachable(
        &self,
        root_id: &str,
        reachable: &HashSet<String>,
    ) -> Result<BuildOutcome> {
        let retained = HashSet::from([root_id.to_string()]);
        let mut outcomes = self
            .build_reachable_retaining(root_id, reachable, &retained)
            .await?;
        outcomes
            .remove(root_id)
            .with_context(|| format!("missing build outcome for `{root_id}`"))
    }

    async fn build_reachable_retaining(
        &self,
        root_id: &str,
        reachable: &HashSet<String>,
        retained: &HashSet<String>,
    ) -> Result<HashMap<String, BuildOutcome>> {
        BuildScheduler::new(
            root_id,
            &self.workspace,
            &self.cache,
            &self.targets,
            &self.analyzer,
            reachable,
            retained,
        )
        .run()
        .await
    }
}

async fn build_one(
    workspace: PathBuf,
    cache: CacheProvider,
    analyzer: AnalysisEngine,
    target: Arc<GraphTarget>,
    dep_providers: Vec<JsonValue>,
    dep_action_digests: Vec<(String, Digest)>,
) -> Result<(String, BuildOutcome)> {
    let target_id = target.label.id.clone();
    // Cheap refcount bumps so the analyzer task and the action runner
    // both reach the same `GraphTarget` without deep-cloning it.
    let analysis_target = Arc::clone(&target);
    let analysis_workspace = workspace.clone();
    let analysis = tokio::task::spawn_blocking(move || {
        analyzer
            .analyze_target(&analysis_target, &analysis_workspace, &dep_providers)
            .with_context(|| format!("analysing {}", analysis_target.label.id))
    })
    .await
    .context("joining graph analysis task")??;

    let outcome =
        run_declared_actions(&workspace, &cache, &target, analysis, &dep_action_digests).await?;
    Ok((target_id, outcome))
}

#[cfg(test)]
mod tests;
