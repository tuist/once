//! Walks the graph in dependency order and executes declared actions
//! for analysis-backed target kinds.
//!
//! For every target the driver calls [`once_frontend::analysis::analyze_target`].
//! When the target kind has an `impl` callable the analysis returns a list of
//! `DeclaredAction`s plus a provider record; we materialise each
//! declared command according to its cache policy and pass the resulting
//! provider down to consumers. When the target kind has no `impl` declared in
//! the prelude, the driver returns `None` so the caller can fall back to
//! its generic marker action.
//!
//! This module stays toolchain-neutral: it consults the prelude via
//! `target_kind_has_impl` to know which kinds run through analysis, and the
//! analysis layer is fed everything it needs through generic Starlark
//! globals. Dep providers and dep action digests are carried so a
//! parent's input digest composes its deps' action digests.

mod actions;
mod scheduler;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context, Result};
use once_cas::{ActionResult, CacheProvider, Digest};
use once_core::{EvidenceCacheState, SandboxMode};
use once_frontend::analysis::{AnalysisEngine, AnalysisOptions};
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
    pub input_digest: Option<Digest>,
    pub outputs: Vec<String>,
    pub cache_tag: &'static str,
    pub cache_state: EvidenceCacheState,
    pub result: ActionResult,
}

/// Command-scoped graph build session.
///
/// The session owns the target id map and analysis engine so one graph
/// command does not repeatedly parse target kind metadata or linearly scan the
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
    module_source_digest: Digest,
    sandbox: SandboxMode,
}

impl BuildSession {
    pub(super) async fn new(
        workspace: &Path,
        cache: &CacheProvider,
        graph: Vec<GraphTarget>,
        sandbox: SandboxMode,
    ) -> Result<Self> {
        Self::new_with_options(workspace, cache, graph, AnalysisOptions::default(), sandbox).await
    }

    pub(super) async fn new_with_options(
        workspace: &Path,
        cache: &CacheProvider,
        graph: Vec<GraphTarget>,
        options: AnalysisOptions,
        sandbox: SandboxMode,
    ) -> Result<Self> {
        let tool_paths = resolve_graph_tools(workspace, &graph).await?;
        Ok(Self::new_with_analyzer(
            workspace,
            cache,
            graph,
            AnalysisEngine::for_workspace_with_options_and_tool_paths(
                workspace, options, tool_paths,
            )?,
            sandbox,
        ))
    }

    fn new_with_analyzer(
        workspace: &Path,
        cache: &CacheProvider,
        graph: Vec<GraphTarget>,
        analyzer: AnalysisEngine,
        sandbox: SandboxMode,
    ) -> Self {
        let module_source_digest = Digest::of_bytes(analyzer.module_source().as_bytes());
        Self {
            workspace: workspace.to_path_buf(),
            cache: cache.clone(),
            targets: graph
                .into_iter()
                .map(|target| {
                    let id = target.label.id.clone();
                    (id, Arc::new(target))
                })
                .collect(),
            analyzer,
            module_source_digest,
            sandbox,
        }
    }

    pub(super) fn target(&self, target_id: &str) -> Result<&GraphTarget> {
        self.targets
            .get(target_id)
            .map(Arc::as_ref)
            .with_context(|| format!("no target matches `{target_id}`"))
    }

    /// Build a target and the impl-backed portion of its dependency
    /// closure. Returns `Ok(None)` when the target's own kind has no
    /// impl, allowing callers to fall back to generic marker actions.
    pub(super) async fn build_with_analysis(
        &self,
        target: &GraphTarget,
    ) -> Result<Option<BuildOutcome>> {
        if !self.analyzer.target_kind_has_impl(&target.kind) {
            tracing::debug!(
                target = %target.label.id,
                kind = %target.kind,
                "graph target has no Starlark analysis implementation"
            );
            return Ok(None);
        }

        let reachable = self.reachable_analysis_targets(target);
        tracing::debug!(
            target = %target.label.id,
            reachable = reachable.len(),
            "building graph target through Starlark analysis"
        );
        let outcome = self.build_reachable(&target.label.id, &reachable).await?;
        Ok(Some(outcome))
    }

    /// Returns a boxed future intentionally because the concrete future
    /// captures dependency outcomes, provider records, and declared action
    /// state. Boxing here keeps graph command callers small enough for
    /// `clippy::large_futures` without spreading `Box::pin` across callers.
    pub(super) fn run_with_analysis<'a>(
        &'a self,
        target: &'a GraphTarget,
        capability: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<BuildOutcome>>> + Send + 'a>> {
        Box::pin(async move {
            if !self.analyzer.target_kind_has_impl(&target.kind) {
                tracing::debug!(
                    target = %target.label.id,
                    kind = %target.kind,
                    capability,
                    "graph target capability has no Starlark analysis implementation"
                );
                return Ok(None);
            }

            let dep_outcomes = self.build_direct_analysis_deps(target).await?;
            tracing::debug!(
                target = %target.label.id,
                capability,
                direct_analysis_deps = dep_outcomes.len(),
                "running graph target capability through Starlark analysis"
            );
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
                self.module_source_digest,
                target,
                capability,
                analysis,
                &dep_action_digests,
                self.sandbox,
            )
            .await
            .with_context(|| format!("executing {capability} for {}", target.label.id))?;
            Ok(Some(outcome))
        })
    }

    fn reachable_analysis_targets(&self, target: &GraphTarget) -> HashSet<String> {
        self.reachable_analysis_targets_from([target.label.id.clone()])
    }

    fn reachable_analysis_deps(&self, target: &GraphTarget) -> HashSet<String> {
        let roots = target.deps.iter().filter_map(|dep_id| {
            let dep = self.targets.get(dep_id)?;
            self.analyzer
                .target_kind_has_impl(&dep.kind)
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
                if self.analyzer.target_kind_has_impl(&dep.kind) {
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
            self.sandbox,
        )
        .run()
        .await
    }
}

/// Resolve the executables declared by the graph's tools to concrete
/// paths through the workspace's mise toolchain.
///
/// Resolution is scoped and best-effort so declaring a tool never makes a
/// build worse off than the host toolchain would:
///
/// * Workspaces without `mise.toml` resolve every executable from the
///   host `PATH`, so an empty map is returned and `host_which` keeps
///   walking `PATH` (and verifying existence) as before.
/// * A declared tool the workspace does not actually pin (for example a
///   rust target in a node-only workspace) is not managed by mise. Its
///   preparation and resolution failures are logged and the executable is
///   left out of the map, so the target falls back to the host toolchain
///   instead of aborting the whole session.
async fn resolve_graph_tools(
    workspace: &Path,
    graph: &[GraphTarget],
) -> Result<BTreeMap<String, String>> {
    if !once_core::workspace_has_mise_config(workspace) {
        return Ok(BTreeMap::new());
    }

    let tool_names = graph
        .iter()
        .flat_map(|target| target.tools.iter().map(|tool| tool.name.clone()))
        .collect::<BTreeSet<_>>();
    let executable_names = graph
        .iter()
        .flat_map(|target| {
            target
                .tools
                .iter()
                .flat_map(|tool| tool.executables.iter().map(String::as_str))
        })
        .collect::<BTreeSet<_>>();
    let tool_names = tool_names.into_iter().collect::<Vec<_>>();
    let tool_name_refs = tool_names.iter().map(String::as_str).collect::<Vec<_>>();
    if let Err(error) = once_core::workspace_prepare_tools(workspace, &tool_name_refs).await {
        tracing::debug!(
            %error,
            "preparing graph tools through mise failed; falling back to the host toolchain"
        );
    }

    // Shared across every resolution task instead of cloned per executable.
    let tool_names = Arc::<[String]>::from(tool_names);
    let mut tasks = tokio::task::JoinSet::new();
    for executable in executable_names {
        let workspace = workspace.to_path_buf();
        let executable = executable.to_string();
        let tool_names = Arc::clone(&tool_names);
        tasks.spawn(async move {
            let tool_name_refs = tool_names.iter().map(String::as_str).collect::<Vec<_>>();
            let path = once_core::workspace_executable(&workspace, &executable, &tool_name_refs)
                .await;
            (executable, path)
        });
    }

    let mut paths = BTreeMap::new();
    while let Some(result) = tasks.join_next().await {
        let (executable, path) = result.context("joining graph tool resolution")?;
        match path {
            Ok(path) => {
                paths.insert(executable, path);
            }
            Err(error) => {
                tracing::debug!(
                    executable,
                    %error,
                    "resolving graph tool executable through mise failed; falling back to the host PATH"
                );
            }
        }
    }
    Ok(paths)
}

#[allow(clippy::too_many_arguments)]
async fn build_one(
    workspace: PathBuf,
    cache: CacheProvider,
    analyzer: AnalysisEngine,
    module_source_digest: Digest,
    target: Arc<GraphTarget>,
    dep_providers: Vec<JsonValue>,
    dep_action_digests: Vec<(String, Digest)>,
    sandbox: SandboxMode,
) -> Result<(String, BuildOutcome)> {
    let target_id = target.label.id.clone();
    tracing::debug!(
        target = %target_id,
        dep_providers = dep_providers.len(),
        dep_action_digests = dep_action_digests.len(),
        "starting graph target analysis"
    );
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
    tracing::debug!(
        target = %target_id,
        declared_actions = analysis.actions.len(),
        declared_outputs = analysis.declared_outputs.len(),
        "finished graph target analysis"
    );

    let outcome = run_declared_actions(
        &workspace,
        &cache,
        module_source_digest,
        &target,
        "build",
        analysis,
        &dep_action_digests,
        sandbox,
    )
    .await?;
    Ok((target_id, outcome))
}

#[cfg(test)]
mod tests;
