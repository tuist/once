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
mod source_digest_cache;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context, Result};
use once_cas::{ActionResult, CacheProvider, Digest};
use once_core::{
    EvidenceCacheState, InputFingerprintManifest, ResourceLimits, ResourcePool, SandboxMode,
};
use once_frontend::analysis::{AnalysisEngine, AnalysisOptions, CachedToolCommand};
use once_frontend::GraphTarget;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use self::actions::{run_declared_actions, validate_declared_actions, DeclaredActionValidation};
use self::scheduler::{BuildScheduler, DependencyInputs};
use self::source_digest_cache::SourceDigestCache;

const GRAPH_TOOL_CACHE_SCHEMA: &str = "once.graph-tool-paths.v4";
const GRAPH_TOOL_ENVIRONMENT_KEYS: &[&str] = &[
    "DEVELOPER_DIR",
    "ELIXIR_ERL_OPTIONS",
    "ERL_AFLAGS",
    "ERL_FLAGS",
    "GEM_HOME",
    "GEM_PATH",
    "GOENV",
    "GOFLAGS",
    "GOROOT",
    "HOME",
    "JAVA_HOME",
    "JAVA_TOOL_OPTIONS",
    "JDK_JAVA_OPTIONS",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "MISE_CACHE_DIR",
    "MISE_CONFIG_DIR",
    "MISE_DATA_DIR",
    "MISE_ENABLE_TOOLS",
    "MISE_ENV",
    "MISE_PROFILE",
    "MIX_ENV",
    "NODE_OPTIONS",
    "PATH",
    "PYTHONHOME",
    "PYTHONPATH",
    "PYTHONWARNINGS",
    "RUBYOPT",
    "RUSTFLAGS",
    "RUSTUP_TOOLCHAIN",
    "SDKROOT",
    "USERPROFILE",
    "ZIG_GLOBAL_CACHE_DIR",
    "_JAVA_OPTIONS",
];

/// Per-target outcome cached during a single command invocation.
///
/// Deliberately not `Clone`: each outcome has exactly one owner at a
/// time: first the producing build task, then `outcomes` in
/// [`BuildSession::build_reachable`], and then either the
/// `dep_providers` of the last reader (move) or the returning caller
/// (also a move via `outcomes.remove`).
#[derive(Debug)]
pub(super) struct BuildOutcome {
    pub provider: Arc<JsonValue>,
    pub action_digest: Digest,
    pub input_digest: Option<Digest>,
    pub input_fingerprint: Option<InputFingerprintManifest>,
    pub available_inputs: BTreeMap<String, AvailableInput>,
    pub outputs: Vec<String>,
    pub cache_tag: &'static str,
    pub cache_state: EvidenceCacheState,
    pub result: ActionResult,
    pub cached_results: Vec<ActionResult>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct AvailableInput {
    pub blob_digest: Digest,
    pub producer_action_digest: Digest,
    pub same_target: bool,
    pub materialized: bool,
}

pub(super) struct AnalyzedCapability {
    pub analysis: once_frontend::analysis::AnalysisResult,
    pub dep_action_digests: Vec<(String, Digest)>,
    pub available_inputs: BTreeMap<String, AvailableInput>,
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
    tool_paths: Arc<BTreeMap<String, String>>,
    tool_cache_fingerprint: Option<Digest>,
    cached_tool_commands: Arc<[CachedToolCommand]>,
    resources: Arc<ResourcePool>,
    source_digest_cache: SourceDigestCache,
    sandbox: SandboxMode,
}

impl BuildSession {
    pub(super) async fn load_workspace(
        workspace: &Path,
        cache: &CacheProvider,
        sandbox: SandboxMode,
    ) -> Result<Self> {
        let workspace_targets =
            once_frontend::load_workspace(workspace).context("loading workspace")?;
        let target_kinds = workspace_targets
            .iter()
            .map(|target| target.kind.clone())
            .collect::<BTreeSet<_>>();
        let analyzer =
            AnalysisEngine::for_target_kinds(workspace, AnalysisOptions::default(), &target_kinds)?;
        let graph = analyzer
            .load_graph_workspace_from_targets(workspace, workspace_targets)
            .context("loading graph")?;
        let resolved_tools = resolve_graph_tools(workspace, &graph).await?;
        let analyzer = analyzer.with_tool_cache(
            resolved_tools.paths.clone(),
            resolved_tools.commands.clone(),
        );
        let mut session = Self::new_with_analyzer(workspace, cache, graph, analyzer, sandbox);
        session.tool_paths = Arc::new(resolved_tools.paths);
        session.tool_cache_fingerprint = resolved_tools.fingerprint;
        session.cached_tool_commands = resolved_tools.commands.into();
        Ok(session)
    }

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
        let resolved_tools = resolve_graph_tools(workspace, &graph).await?;
        let target_kinds = graph
            .iter()
            .map(|target| target.kind.clone())
            .collect::<BTreeSet<_>>();
        let analyzer = AnalysisEngine::for_target_kinds(workspace, options, &target_kinds)?
            .with_tool_cache(
                resolved_tools.paths.clone(),
                resolved_tools.commands.clone(),
            );
        let mut session = Self::new_with_analyzer(workspace, cache, graph, analyzer, sandbox);
        session.tool_paths = Arc::new(resolved_tools.paths);
        session.tool_cache_fingerprint = resolved_tools.fingerprint;
        session.cached_tool_commands = resolved_tools.commands.into();
        Ok(session)
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
            tool_paths: Arc::new(BTreeMap::new()),
            tool_cache_fingerprint: None,
            cached_tool_commands: Arc::from([]),
            resources: Arc::new(ResourcePool::new(ResourceLimits::default())),
            source_digest_cache: SourceDigestCache::open(workspace),
            sandbox,
        }
    }

    pub(super) fn with_resource_limits(mut self, limits: ResourceLimits) -> Self {
        self.resources = Arc::new(ResourcePool::new(limits));
        self
    }

    pub(super) fn target(&self, target_id: &str) -> Result<&GraphTarget> {
        let target = self
            .targets
            .get(target_id)
            .map(Arc::as_ref)
            .with_context(|| format!("no target matches `{target_id}`"))?;
        ensure_graph_target_valid(target)?;
        Ok(target)
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
        materialize_cached_outputs(
            &outcome,
            &self.workspace,
            &self.cache,
            Some(&self.source_digest_cache),
        )
        .await
        .with_context(|| format!("materializing outputs for {}", target.label.id))?;
        self.persist_graph_tool_cache();
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
        self.run_with_analysis_and_provider_validation(target, capability, |_, _| Ok(()))
    }

    pub(super) fn run_with_analysis_and_provider_validation<'a>(
        &'a self,
        target: &'a GraphTarget,
        capability: &'a str,
        validate_provider: fn(&GraphTarget, &JsonValue) -> Result<()>,
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

            let AnalyzedCapability {
                analysis,
                dep_action_digests,
                available_inputs,
            } = self.analyze_capability(target, capability).await?;
            validate_provider(target, &analysis.provider)?;
            let outcome = run_declared_actions(
                &self.workspace,
                &self.cache,
                self.module_source_digest,
                target,
                capability,
                analysis,
                &dep_action_digests,
                &available_inputs,
                &self.tool_paths,
                Some(&self.source_digest_cache),
                self.sandbox,
                &self.resources,
            )
            .await
            .with_context(|| format!("executing {capability} for {}", target.label.id))?;
            materialize_cached_outputs(
                &outcome,
                &self.workspace,
                &self.cache,
                Some(&self.source_digest_cache),
            )
            .await
            .with_context(|| {
                format!("materializing {capability} outputs for {}", target.label.id)
            })?;
            self.persist_graph_tool_cache();
            Ok(Some(outcome))
        })
    }

    pub(super) async fn analyze_capability(
        &self,
        target: &GraphTarget,
        capability: &str,
    ) -> Result<AnalyzedCapability> {
        if !self.analyzer.target_kind_has_impl(&target.kind) {
            anyhow::bail!(
                "target kind `{}` does not declare scripted actions",
                target.kind
            );
        }
        let dep_outcomes = self.build_direct_analysis_deps(target).await?;
        tracing::debug!(
            target = %target.label.id,
            capability,
            direct_analysis_deps = dep_outcomes.len(),
            "analysing graph target capability actions"
        );
        let mut providers_by_id = HashMap::with_capacity(dep_outcomes.len());
        let mut dep_action_digests = Vec::with_capacity(dep_outcomes.len());
        let mut available_inputs = BTreeMap::new();
        for (dep_id, outcome) in dep_outcomes {
            dep_action_digests.push((dep_id.clone(), outcome.action_digest));
            available_inputs.extend(outcome.available_inputs.into_iter().map(
                |(path, mut input)| {
                    input.same_target = false;
                    (path, input)
                },
            ));
            providers_by_id.insert(dep_id, outcome.provider);
        }
        let dep_providers = target
            .deps
            .iter()
            .filter_map(|dep_id| providers_by_id.get(dep_id).cloned())
            .collect::<Vec<_>>();
        let dependency_providers = target
            .dependency_edges
            .iter()
            .map(|(role, dep_ids)| {
                let providers = dep_ids
                    .iter()
                    .filter_map(|dep_id| providers_by_id.get(dep_id).cloned())
                    .collect::<Vec<_>>();
                (role.clone(), providers)
            })
            .collect::<BTreeMap<_, _>>();
        let analysis = self
            .analyzer
            .analyze_target_capability_with_shared_dependency_roles(
                target,
                &self.workspace,
                &dep_providers,
                &dependency_providers,
                capability,
            )
            .with_context(|| format!("analysing {}", target.label.id))?;
        Ok(AnalyzedCapability {
            analysis,
            dep_action_digests,
            available_inputs,
        })
    }

    pub(super) async fn validate_actions(
        &self,
        target: &GraphTarget,
        capability: &str,
        selected_index: Option<usize>,
    ) -> Result<Vec<DeclaredActionValidation>> {
        let AnalyzedCapability {
            analysis,
            dep_action_digests,
            available_inputs: _,
        } = self.analyze_capability(target, capability).await?;
        validate_declared_actions(
            &self.workspace,
            &self.cache,
            self.module_source_digest,
            target,
            analysis,
            &dep_action_digests,
            selected_index,
        )
        .await
    }

    fn reachable_analysis_targets(&self, target: &GraphTarget) -> HashSet<String> {
        self.reachable_analysis_targets_from([target.label.id.clone()])
    }

    fn reachable_analysis_deps(&self, target: &GraphTarget) -> HashSet<String> {
        let roots = target.dependency_ids().filter_map(|dep_id| {
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
            for dep_id in target.dependency_ids() {
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

        let mut seen = HashSet::new();
        let direct_deps = target
            .dependency_ids()
            .filter(|dep_id| reachable.contains(*dep_id) && seen.insert((*dep_id).clone()))
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
            self.build_context(),
            &self.targets,
            reachable,
            retained,
        )
        .run()
        .await
    }

    /// Snapshot the per-build values every spawned task needs.
    fn build_context(&self) -> BuildContext {
        BuildContext {
            workspace: self.workspace.clone(),
            cache: self.cache.clone(),
            analyzer: self.analyzer.clone(),
            module_source_digest: Digest::of_bytes(self.analyzer.module_source().as_bytes()),
            tool_paths: Arc::clone(&self.tool_paths),
            source_digest_cache: self.source_digest_cache.clone(),
            sandbox: self.sandbox,
            resources: Arc::clone(&self.resources),
        }
    }

    fn persist_graph_tool_cache(&self) {
        let Some(fingerprint) = self.tool_cache_fingerprint else {
            return;
        };
        let commands = match self.analyzer.cacheable_tool_commands() {
            Ok(commands) => commands,
            Err(error) => {
                tracing::debug!(%error, "failed to read cached graph tool commands");
                return;
            }
        };
        if commands.as_slice() == self.cached_tool_commands.as_ref() {
            return;
        }
        if let Err(error) =
            write_graph_tool_cache(&self.workspace, fingerprint, &self.tool_paths, &commands)
        {
            tracing::debug!(%error, "failed to persist graph tool commands");
        }
    }

    pub(super) fn observed_environment(&self) -> BTreeMap<String, Option<String>> {
        let mut environment = self.analyzer.observed_host_environment();
        for key in GRAPH_TOOL_ENVIRONMENT_KEYS {
            environment
                .entry((*key).to_string())
                .or_insert_with(|| std::env::var(key).ok());
        }
        environment
    }

    pub(super) fn observed_host_paths(&self) -> BTreeSet<PathBuf> {
        let mut paths = self.analyzer.observed_host_paths();
        paths.extend(self.tool_paths.values().map(PathBuf::from));
        paths
    }

    pub(super) fn observed_source_digests(
        &self,
        changes: Option<&[String]>,
    ) -> BTreeMap<String, Digest> {
        self.source_digest_cache.observed_digests_for(changes)
    }

    pub(super) fn source_changes_match(&self, changes: Option<&[String]>) -> bool {
        self.source_digest_cache
            .changes_match(&self.workspace, changes)
    }
}

async fn materialize_cached_outputs(
    outcome: &BuildOutcome,
    workspace: &Path,
    cache: &CacheProvider,
    source_digest_cache: Option<&SourceDigestCache>,
) -> Result<()> {
    for result in &outcome.cached_results {
        match source_digest_cache {
            Some(digests) => digests
                .materialize_outputs(result, workspace, cache)
                .await
                .map_err(anyhow::Error::from)?,
            None => once_core::materialize_outputs(result, workspace, cache)
                .await
                .map_err(anyhow::Error::from)?,
        }
    }
    Ok(())
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
) -> Result<ResolvedGraphTools> {
    if !once_core::workspace_has_mise_config(workspace) {
        return Ok(ResolvedGraphTools::default());
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
                .flat_map(|tool| tool.executables.iter().cloned())
        })
        .collect::<BTreeSet<_>>();
    let tool_names = tool_names.into_iter().collect::<Vec<_>>();
    let executable_names = executable_names.into_iter().collect::<Vec<_>>();
    let fingerprint = graph_tool_cache_fingerprint(workspace, &tool_names, &executable_names)?;
    if let Some(cached) = read_graph_tool_cache(workspace, fingerprint) {
        tracing::debug!(
            tools = cached.paths.len(),
            commands = cached.commands.len(),
            "reused validated graph tool paths"
        );
        return Ok(ResolvedGraphTools {
            paths: cached.paths,
            fingerprint: Some(fingerprint),
            commands: cached.commands,
        });
    }

    let tool_names = Arc::<[String]>::from(tool_names);
    let (mut paths, mut failures) =
        resolve_graph_tool_executables(workspace, &tool_names, &executable_names).await?;
    if !failures.is_empty() {
        let tool_name_refs = tool_names.iter().map(String::as_str).collect::<Vec<_>>();
        if let Err(error) = once_core::workspace_prepare_tools(workspace, &tool_name_refs).await {
            tracing::debug!(
                %error,
                "preparing graph tools through mise failed; falling back to the host toolchain"
            );
        } else {
            (paths, failures) =
                resolve_graph_tool_executables(workspace, &tool_names, &executable_names).await?;
        }
    }
    for (executable, error) in failures {
        tracing::trace!(
            executable,
            %error,
            "graph tool executable is not ready"
        );
    }
    if paths.len() == executable_names.len() {
        if let Err(error) = write_graph_tool_cache(workspace, fingerprint, &paths, &[]) {
            tracing::debug!(%error, "failed to persist graph tool paths");
        }
    }
    Ok(ResolvedGraphTools {
        paths,
        fingerprint: Some(fingerprint),
        commands: Vec::new(),
    })
}

async fn resolve_graph_tool_executables(
    workspace: &Path,
    tool_names: &Arc<[String]>,
    executable_names: &[String],
) -> Result<(
    BTreeMap<String, String>,
    Vec<(String, once_core::ToolEnvError)>,
)> {
    let mut tasks = tokio::task::JoinSet::new();
    for executable in executable_names {
        let workspace = workspace.to_path_buf();
        let executable = executable.clone();
        let tool_names = Arc::clone(tool_names);
        tasks.spawn(async move {
            let tool_name_refs = tool_names.iter().map(String::as_str).collect::<Vec<_>>();
            let path =
                once_core::workspace_executable(&workspace, &executable, &tool_name_refs).await;
            (executable, path)
        });
    }

    let mut paths = BTreeMap::new();
    let mut failures = Vec::new();
    while let Some(result) = tasks.join_next().await {
        let (executable, path) = result.context("joining graph tool resolution")?;
        match path {
            Ok(path) => {
                paths.insert(executable, path);
            }
            Err(error) => {
                failures.push((executable, error));
            }
        }
    }
    Ok((paths, failures))
}

#[derive(Debug, Serialize, Deserialize)]
struct GraphToolCache {
    schema: String,
    fingerprint: String,
    paths_fingerprint: String,
    paths: BTreeMap<String, String>,
    commands: Vec<CachedToolCommand>,
}

#[derive(Debug, Default)]
struct ResolvedGraphTools {
    paths: BTreeMap<String, String>,
    fingerprint: Option<Digest>,
    commands: Vec<CachedToolCommand>,
}

fn graph_tool_cache_fingerprint(
    workspace: &Path,
    tool_names: &[String],
    executable_names: &[String],
) -> Result<Digest> {
    let mut bytes = Vec::new();
    push_fingerprint_part(&mut bytes, GRAPH_TOOL_CACHE_SCHEMA.as_bytes());
    push_fingerprint_part(&mut bytes, once_core::MANAGED_MISE_VERSION.as_bytes());
    push_fingerprint_part(&mut bytes, std::env::consts::OS.as_bytes());
    push_fingerprint_part(&mut bytes, std::env::consts::ARCH.as_bytes());
    for path in ["mise.toml", "mise.lock"] {
        push_fingerprint_part(&mut bytes, path.as_bytes());
        match std::fs::read(workspace.join(path)) {
            Ok(contents) => push_fingerprint_part(&mut bytes, &contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                push_fingerprint_part(&mut bytes, b"missing");
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("reading graph tool configuration `{path}`"));
            }
        }
    }
    for tool in tool_names {
        push_fingerprint_part(&mut bytes, tool.as_bytes());
    }
    for executable in executable_names {
        push_fingerprint_part(&mut bytes, executable.as_bytes());
    }
    for key in GRAPH_TOOL_ENVIRONMENT_KEYS {
        push_fingerprint_part(&mut bytes, key.as_bytes());
        match std::env::var_os(key) {
            Some(value) => push_fingerprint_part(&mut bytes, value.to_string_lossy().as_bytes()),
            None => push_fingerprint_part(&mut bytes, b"missing"),
        }
    }
    Ok(Digest::of_bytes(&bytes))
}

fn push_fingerprint_part(bytes: &mut Vec<u8>, part: &[u8]) {
    bytes.extend_from_slice(&(part.len() as u64).to_le_bytes());
    bytes.extend_from_slice(part);
}

fn read_graph_tool_cache(workspace: &Path, expected_fingerprint: Digest) -> Option<GraphToolCache> {
    read_graph_tool_cache_at(&graph_tool_cache_path(workspace), expected_fingerprint)
}

fn read_graph_tool_cache_at(path: &Path, expected_fingerprint: Digest) -> Option<GraphToolCache> {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::debug!(%error, path = %path.display(), "failed to read graph tool paths");
            return None;
        }
    };
    let cached = match serde_json::from_slice::<GraphToolCache>(&raw) {
        Ok(cached) => cached,
        Err(error) => {
            tracing::debug!(%error, path = %path.display(), "failed to parse graph tool paths");
            return None;
        }
    };
    if cached.schema != GRAPH_TOOL_CACHE_SCHEMA
        || cached.fingerprint != expected_fingerprint.to_string()
        || graph_tool_paths_fingerprint(&cached.paths)
            .is_none_or(|fingerprint| fingerprint.to_string() != cached.paths_fingerprint)
    {
        return None;
    }
    Some(cached)
}

fn write_graph_tool_cache(
    workspace: &Path,
    fingerprint: Digest,
    paths: &BTreeMap<String, String>,
    commands: &[CachedToolCommand],
) -> Result<()> {
    write_graph_tool_cache_at(
        &graph_tool_cache_path(workspace),
        fingerprint,
        paths,
        commands,
    )
}

fn write_graph_tool_cache_at(
    path: &Path,
    fingerprint: Digest,
    paths: &BTreeMap<String, String>,
    commands: &[CachedToolCommand],
) -> Result<()> {
    let parent = path
        .parent()
        .expect("graph tool cache path always has a parent");
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating graph tool cache directory `{}`", parent.display()))?;
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let paths_fingerprint =
        graph_tool_paths_fingerprint(paths).context("fingerprinting graph tool paths")?;
    let record = GraphToolCache {
        schema: GRAPH_TOOL_CACHE_SCHEMA.to_string(),
        fingerprint: fingerprint.to_string(),
        paths_fingerprint: paths_fingerprint.to_string(),
        paths: paths.clone(),
        commands: commands.to_vec(),
    };
    let raw = serde_json::to_vec(&record).context("serializing graph tool paths")?;
    std::fs::write(&temporary, raw)
        .with_context(|| format!("writing graph tool cache `{}`", temporary.display()))?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        if !path.is_file() {
            return Err(error)
                .with_context(|| format!("activating graph tool cache `{}`", path.display()));
        }
        std::fs::remove_file(path)
            .with_context(|| format!("replacing graph tool cache `{}`", path.display()))?;
        std::fs::rename(&temporary, path)
            .with_context(|| format!("activating graph tool cache `{}`", path.display()))?;
    }
    Ok(())
}

fn graph_tool_cache_path(workspace: &Path) -> PathBuf {
    graph_tool_cache_path_from(&once_core::Xdg::from_env().once_toolchains(), workspace)
}

fn graph_tool_cache_path_from(toolchain_root: &Path, workspace: &Path) -> PathBuf {
    let workspace = std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let workspace_id = Digest::of_bytes(workspace.to_string_lossy().as_bytes());
    toolchain_root.join(format!("{workspace_id}.json"))
}

fn graph_tool_paths_fingerprint(paths: &BTreeMap<String, String>) -> Option<Digest> {
    let mut bytes = Vec::new();
    for (name, path) in paths {
        let metadata = std::fs::metadata(path).ok()?;
        if !metadata.is_file() {
            return None;
        }
        let modified = metadata
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        push_fingerprint_part(&mut bytes, name.as_bytes());
        push_fingerprint_part(&mut bytes, path.as_bytes());
        push_fingerprint_part(&mut bytes, &metadata.len().to_le_bytes());
        push_fingerprint_part(&mut bytes, &modified.as_nanos().to_le_bytes());
    }
    Some(Digest::of_bytes(&bytes))
}

/// Everything a target build needs that is the same for every target in
/// one graph build.
///
/// Cloned once per spawned build task. Every field is a handle or an
/// `Arc`, so a clone is a few refcount bumps rather than a deep copy of
/// the analyzer or the cache.
#[derive(Clone)]
struct BuildContext {
    pub workspace: PathBuf,
    pub cache: CacheProvider,
    pub analyzer: AnalysisEngine,
    pub module_source_digest: Digest,
    pub tool_paths: Arc<BTreeMap<String, String>>,
    pub source_digest_cache: SourceDigestCache,
    pub sandbox: SandboxMode,
    pub resources: Arc<ResourcePool>,
}

async fn build_one(
    context: BuildContext,
    target: Arc<GraphTarget>,
    inputs: DependencyInputs,
) -> Result<(String, BuildOutcome)> {
    let BuildContext {
        workspace,
        cache,
        analyzer,
        module_source_digest,
        tool_paths,
        source_digest_cache,
        sandbox,
        resources,
    } = context;
    let DependencyInputs {
        providers,
        providers_by_role,
        action_digests,
        available_inputs,
    } = inputs;

    ensure_graph_target_valid(&target)?;
    let target_id = target.label.id.clone();
    tracing::trace!(
        target = %target_id,
        dep_providers = providers.len(),
        dependency_roles = providers_by_role.len(),
        dep_action_digests = action_digests.len(),
        "starting graph target analysis"
    );
    // Cheap refcount bumps so the analyzer task and the action runner
    // both reach the same `GraphTarget` without deep-cloning it.
    let analysis_target = Arc::clone(&target);
    let analysis_workspace = workspace.clone();
    let analysis = tokio::task::spawn_blocking(move || {
        analyzer
            .analyze_target_capability_with_shared_dependency_roles(
                &analysis_target,
                &analysis_workspace,
                &providers,
                &providers_by_role,
                "build",
            )
            .with_context(|| format!("analysing {}", analysis_target.label.id))
    })
    .await
    .context("joining graph analysis task")??;
    tracing::trace!(
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
        &action_digests,
        &available_inputs,
        &tool_paths,
        Some(&source_digest_cache),
        sandbox,
        &resources,
    )
    .await?;
    Ok((target_id, outcome))
}

fn ensure_graph_target_valid(target: &GraphTarget) -> Result<()> {
    let Some(diagnostic) = target.diagnostics.first() else {
        return Ok(());
    };
    Err(anyhow::Error::new(
        once_frontend::analysis::AnalysisFailure {
            diagnostic: diagnostic.clone(),
        },
    ))
}

#[cfg(test)]
mod tests;
