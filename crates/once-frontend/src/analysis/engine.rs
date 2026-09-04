use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use serde_json::Value as JsonValue;
use starlark::environment::{FrozenModule, Module};
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};
use starlark::values::dict::{AllocDict, DictRef};
use starlark::values::Value;

use sha2::Digest as _;

use super::globals::{globals_for_prelude, UnchangedWorkspace};
use super::store::{
    with_active_store, with_store, AnalysisObservations, AnalysisStore, CachedToolCommand,
    CommandPolicy, DeclaredAction, HostCache, HostToolFailure,
};
use super::values::{attr_value_to_starlark, json_to_value, value_to_json};
use crate::graph::{Diagnostic, GraphTarget, TargetKindSchema};
use crate::Target;

/// Bump when the meaning of an analysis key or a stored analysis changes in a
/// way an older record would not notice.
const ANALYSIS_KEY_SCHEMA: &str = "once.analysis.v1";

/// Resolve a workspace root to one spelling, so two paths naming the same tree
/// through a symlink or a relative prefix do not get separate records.
fn canonical_root(root: &Path) -> PathBuf {
    std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

type ResolverCallback<'a> =
    dyn FnMut(&GraphTarget, &BTreeMap<String, String>) -> Result<Option<JsonValue>> + 'a;

/// Extra execution context supplied by command surfaces.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct AnalysisOptions {
    /// Request a visible runtime surface for run capabilities when the target
    /// kind supports one.
    pub run_visible: bool,
    /// Arguments supplied to the target's generic run capability.
    pub run_arguments: Vec<String>,
    /// Stable semantic test-unit identifiers requested for a test capability.
    pub test_filters: Vec<String>,
    /// Stable batch identifier used to isolate outputs for parallel test runs.
    pub test_batch_id: Option<String>,
}

/// Result of analyzing one target.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AnalysisResult {
    /// Declared command actions in the order the impl emitted them.
    pub actions: Vec<DeclaredAction>,
    /// Provider record returned by the impl (the impl's return value).
    pub provider: JsonValue,
    /// Workspace-relative outputs declared during this analysis.
    pub declared_outputs: Vec<String>,
    /// Everything the impl read from outside itself. Empty when the result was
    /// replayed rather than computed, since the answers were just checked.
    #[serde(skip)]
    pub observations: AnalysisObservations,
}

#[derive(Debug)]
pub struct AnalysisFailure {
    pub diagnostic: Diagnostic,
}

impl fmt::Display for AnalysisFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.diagnostic.message)
    }
}

impl std::error::Error for AnalysisFailure {}

/// Command-scoped analysis helper.
///
/// Construct this once for a graph command and reuse it for every
/// target. It compiles the Starlark module once, caches target kind
/// metadata and generic host lookups (`host_which`, `host_command`), and
/// evaluates each target's impl in an isolated heap.
#[derive(Clone)]
pub struct AnalysisEngine {
    source_path: Arc<str>,
    source: Arc<str>,
    module: Arc<FrozenModule>,
    target_kind_callbacks: TargetKindCallbacks,
    target_kind_schemas: Arc<[TargetKindSchema]>,
    host_cache: HostCache,
    options: AnalysisOptions,
    configuration: crate::manifest::BuildConfiguration,
    configuration_path_suffix: String,
}

impl fmt::Debug for AnalysisEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnalysisEngine")
            .field("source_path", &self.source_path)
            .field("source_len", &self.source.len())
            .field("module", &"<compiled>")
            .field("target_kind_callbacks", &self.target_kind_callbacks)
            .field("target_kind_schemas", &self.target_kind_schemas.len())
            .field("host_cache", &self.host_cache)
            .field("options", &self.options)
            .field("configuration", &self.configuration)
            .field("configuration_path_suffix", &self.configuration_path_suffix)
            .finish()
    }
}

impl AnalysisEngine {
    /// Engine over the built-in prelude only, with no workspace on disk.
    pub fn new() -> Result<Self> {
        Self::from_source_with_path(
            crate::modules::BUILT_IN_MODULE_PATH,
            crate::modules::built_in_module_source(),
            AnalysisOptions::default(),
            false,
        )
    }

    /// Engine over a workspace's prelude and configuration, with default
    /// options.
    pub fn for_workspace(root: &Path) -> Result<Self> {
        Self::for_workspace_with_options(root, AnalysisOptions::default())
    }

    pub(crate) fn resolve_workspace_targets<T>(
        root: &Path,
        observations: &mut AnalysisObservations,
        operation: impl FnOnce(&mut ResolverCallback<'_>) -> Result<T>,
    ) -> Result<T> {
        let source = crate::modules::combined_module_source_for_workspace(root)?;
        resolve_targets_in_starlark(
            crate::modules::COMBINED_MODULE_PATH,
            &source,
            root,
            &HostCache::default(),
            &crate::manifest::load_workspace_configuration(root)?,
            observations,
            operation,
        )
    }

    /// [`for_workspace`](Self::for_workspace) with explicit options.
    pub fn for_workspace_with_options(root: &Path, options: AnalysisOptions) -> Result<Self> {
        let source = crate::modules::combined_module_source_for_workspace(root)?;
        let mut engine = Self::from_source_with_path(
            crate::modules::COMBINED_MODULE_PATH,
            source,
            options,
            true,
        )?;
        engine.configuration = crate::manifest::load_workspace_configuration(root)?;
        Ok(engine)
    }

    /// Like [`for_workspace_with_options`](Self::for_workspace_with_options)
    /// but loads only the prelude modules the given target kinds need.
    ///
    /// A workspace that uses two target kinds pays for two, not for every
    /// built-in module. Tool paths are layered on afterwards with
    /// [`with_tool_paths`](Self::with_tool_paths) rather than through a
    /// further constructor.
    pub fn for_target_kinds(
        root: &Path,
        options: AnalysisOptions,
        target_kinds: &BTreeSet<String>,
    ) -> Result<Self> {
        let source =
            crate::modules::combined_analysis_module_source_for_workspace(root, target_kinds)?;
        let mut engine = Self::from_source_with_path(
            crate::modules::COMBINED_MODULE_PATH,
            source,
            options,
            true,
        )?;
        engine.configuration = crate::manifest::load_workspace_configuration(root)?;
        Ok(engine)
    }

    /// Engine over Starlark source held in memory. Mainly for tests and
    /// for evaluating a module without a workspace behind it.
    pub fn from_source(source: impl Into<Arc<str>>) -> Result<Self> {
        Self::from_source_with_path(
            crate::modules::BUILT_IN_MODULE_PATH,
            source,
            AnalysisOptions::default(),
            false,
        )
    }

    pub fn from_source_with_options(
        source: impl Into<Arc<str>>,
        options: AnalysisOptions,
    ) -> Result<Self> {
        Self::from_source_with_path(crate::modules::BUILT_IN_MODULE_PATH, source, options, false)
    }

    fn from_source_with_path(
        source_path: impl Into<Arc<str>>,
        source: impl Into<Arc<str>>,
        options: AnalysisOptions,
        extract_target_kind_schemas: bool,
    ) -> Result<Self> {
        let source_path = source_path.into();
        let source = source.into();
        let (module, target_kind_callbacks, target_kind_schemas) =
            compile_target_kind_callbacks(&source_path, &source, extract_target_kind_schemas)?;
        Ok(Self {
            source_path,
            source,
            module: Arc::new(module),
            target_kind_callbacks,
            target_kind_schemas: target_kind_schemas.into(),
            host_cache: HostCache::default(),
            options,
            configuration: crate::manifest::BuildConfiguration::default(),
            configuration_path_suffix: String::new(),
        })
    }

    #[must_use]
    pub fn module_source(&self) -> &str {
        &self.source
    }

    #[must_use]
    pub fn target_kind_has_impl(&self, kind: &str) -> bool {
        self.target_kind_callbacks.has_impl(kind)
    }

    pub fn load_graph_workspace(&self, root: &Path) -> Result<Vec<GraphTarget>> {
        crate::graph::load_graph_workspace_with_compiled_schemas(root, &self.target_kind_schemas)
            .map_err(anyhow::Error::new)
    }

    pub fn load_graph_workspace_from_targets(
        &self,
        root: &Path,
        targets: Vec<Target>,
    ) -> Result<Vec<GraphTarget>> {
        Ok(self
            .load_graph_workspace_from_targets_recorded(root, targets)?
            .0)
    }

    /// Load the graph and report what deriving it read.
    ///
    /// The record is what lets a caller decide, next time, whether deriving it
    /// again could produce anything different.
    pub fn load_graph_workspace_from_targets_recorded(
        &self,
        root: &Path,
        targets: Vec<Target>,
    ) -> Result<(Vec<GraphTarget>, crate::ResolutionRecord)> {
        crate::graph::load_graph_workspace_with_compiled_schemas_and_targets_recorded(
            root,
            &self.target_kind_schemas,
            targets,
        )
        .map_err(anyhow::Error::new)
    }

    #[must_use]
    pub fn with_tool_paths(mut self, tool_paths: BTreeMap<String, String>) -> Self {
        self.host_cache = HostCache::with_tool_paths(tool_paths);
        self
    }

    /// Set the effective configuration and its output-path suffix.
    ///
    /// The configuration drives Starlark analysis and dependency selection;
    /// the suffix scopes analysis output directories so two invocations
    /// with different configurations never share build outputs. An empty
    /// suffix keeps paths identical to the workspace-default case.
    #[must_use]
    pub fn with_configuration(
        mut self,
        configuration: crate::manifest::BuildConfiguration,
        configuration_path_suffix: String,
    ) -> Self {
        self.configuration = configuration;
        self.configuration_path_suffix = configuration_path_suffix;
        self
    }

    #[must_use]
    pub fn with_tool_cache(
        mut self,
        tool_paths: BTreeMap<String, String>,
        commands: Vec<CachedToolCommand>,
    ) -> Self {
        self.host_cache = HostCache::with_tool_cache(tool_paths, commands);
        self
    }

    #[must_use]
    pub fn cacheable_tool_commands(&self) -> Vec<CachedToolCommand> {
        self.host_cache.cacheable_tool_commands()
    }

    pub fn observed_host_environment(&self) -> BTreeMap<String, Option<String>> {
        self.host_cache.observed_environment()
    }

    pub fn observed_host_paths(&self) -> BTreeSet<PathBuf> {
        self.host_cache.observed_paths()
    }

    /// Run a single target's target kind impl and collect its declared
    /// actions and provider record.
    ///
    /// `dep_providers` supplies the provider record each in-graph
    /// dependency already returned; impls iterate it to gather
    /// whatever transitive state their target kind family carries (search
    /// paths, archives, linker flags, and so on).
    pub fn analyze_target(
        &self,
        target: &GraphTarget,
        workspace_root: &Path,
        dep_providers: &[JsonValue],
    ) -> Result<AnalysisResult> {
        self.analyze_target_capability(target, workspace_root, dep_providers, "build")
    }

    pub fn analyze_target_capability(
        &self,
        target: &GraphTarget,
        workspace_root: &Path,
        dep_providers: &[JsonValue],
        capability: &str,
    ) -> Result<AnalysisResult> {
        self.analyze_target_capability_with_dependency_roles(
            target,
            workspace_root,
            dep_providers,
            &BTreeMap::new(),
            capability,
        )
    }

    pub fn analyze_target_capability_with_dependency_roles(
        &self,
        target: &GraphTarget,
        workspace_root: &Path,
        dep_providers: &[JsonValue],
        dependency_providers: &BTreeMap<String, Vec<JsonValue>>,
        capability: &str,
    ) -> Result<AnalysisResult> {
        let dep_providers = dep_providers.iter().collect::<Vec<_>>();
        let dependency_providers = dependency_providers
            .iter()
            .map(|(role, providers)| (role.clone(), providers.iter().collect::<Vec<_>>()))
            .collect::<BTreeMap<_, _>>();
        self.analyze_target_capability_with_provider_refs(
            target,
            workspace_root,
            &dep_providers,
            &dependency_providers,
            capability,
        )
    }

    pub fn analyze_target_capability_with_shared_dependency_roles(
        &self,
        target: &GraphTarget,
        workspace_root: &Path,
        dep_providers: &[Arc<JsonValue>],
        dependency_providers: &BTreeMap<String, Vec<Arc<JsonValue>>>,
        capability: &str,
    ) -> Result<AnalysisResult> {
        let dep_providers = dep_providers.iter().map(Arc::as_ref).collect::<Vec<_>>();
        let dependency_providers = dependency_providers
            .iter()
            .map(|(role, providers)| {
                (
                    role.clone(),
                    providers.iter().map(Arc::as_ref).collect::<Vec<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        self.analyze_target_capability_with_provider_refs(
            target,
            workspace_root,
            &dep_providers,
            &dependency_providers,
            capability,
        )
    }

    /// Name for one analysis: the same name means the same call, so a stored
    /// result under it describes this exact question.
    ///
    /// Everything that reaches the impl has to be in here. The target
    /// definition and its dependencies' providers are the arguments; the
    /// capability, the options, and the configuration select which branch it
    /// takes; the workspace root and the module source say which code runs
    /// against which tree. The running executable is in it too, because the
    /// globals the impl calls are implemented here, in Rust, and their
    /// behaviour can change with no Starlark edit to show for it.
    ///
    /// What is deliberately absent is the resolved tool paths and any cached
    /// discovery command output: those reach the impl only through globals,
    /// whose answers are recorded as observations and checked on replay.
    pub fn analysis_key(
        &self,
        target: &GraphTarget,
        workspace_root: &Path,
        dep_providers: &[Arc<JsonValue>],
        dependency_providers: &BTreeMap<String, Vec<Arc<JsonValue>>>,
        capability: &str,
        executable_identity: &str,
    ) -> Option<String> {
        let mut hasher = sha2::Sha256::new();
        let mut part = |bytes: &[u8]| {
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        };
        part(ANALYSIS_KEY_SCHEMA.as_bytes());
        part(executable_identity.as_bytes());
        part(self.source_path.as_bytes());
        part(self.source.as_bytes());
        part(canonical_root(workspace_root).to_string_lossy().as_bytes());
        part(capability.as_bytes());
        part(&serde_json::to_vec(target).ok()?);
        part(&serde_json::to_vec(&self.options).ok()?);
        part(&serde_json::to_vec(&self.configuration).ok()?);
        part(self.configuration_path_suffix.as_bytes());
        part(&(dep_providers.len() as u64).to_le_bytes());
        for provider in dep_providers {
            part(&serde_json::to_vec(provider.as_ref()).ok()?);
        }
        for (role, providers) in dependency_providers {
            part(role.as_bytes());
            part(&(providers.len() as u64).to_le_bytes());
            for provider in providers {
                part(&serde_json::to_vec(provider.as_ref()).ok()?);
            }
        }
        Some(super::globals::hex_digest(&hasher.finalize()))
    }

    /// Whether every answer a stored analysis recorded is still the answer the
    /// host gives, in which case replaying it produces what re-running would.
    ///
    /// Runs against this engine's own host cache, so a lookup shared by many
    /// targets is paid once for the invocation rather than once per target.
    pub fn observations_hold(
        &self,
        workspace_root: &Path,
        observations: &AnalysisObservations,
        policy: CommandPolicy,
        unchanged: &UnchangedWorkspace,
    ) -> bool {
        super::globals::observations_hold(
            workspace_root,
            &self.host_cache,
            observations,
            policy,
            unchanged,
        )
    }

    /// The first recorded answer that no longer matches, for diagnosing why a
    /// stored analysis was not reused.
    pub fn first_stale_observation(
        &self,
        workspace_root: &Path,
        observations: &AnalysisObservations,
        policy: CommandPolicy,
        unchanged: &UnchangedWorkspace,
    ) -> Option<String> {
        super::globals::first_stale_observation(
            workspace_root,
            &self.host_cache,
            observations,
            policy,
            unchanged,
        )
    }

    fn analyze_target_capability_with_provider_refs(
        &self,
        target: &GraphTarget,
        workspace_root: &Path,
        dep_providers: &[&JsonValue],
        dependency_providers: &BTreeMap<String, Vec<&JsonValue>>,
        capability: &str,
    ) -> Result<AnalysisResult> {
        let analysis = TargetAnalysis {
            target,
            workspace_root,
            dep_providers,
            dependency_providers,
            capability,
            options: self.options.clone(),
            configuration: &self.configuration,
            configuration_path_suffix: &self.configuration_path_suffix,
        };
        analyze_target_with_host_cache(
            &self.module,
            &self.target_kind_callbacks,
            self.host_cache.clone(),
            &analysis,
        )
    }
}

/// Cached view of which target kinds declare executable impls.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetKindCallbacks {
    by_kind: BTreeMap<String, TargetKindCallbackFlags>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetKindCallbackFlags {
    export_name: String,
    has_impl: bool,
}

impl TargetKindCallbacks {
    #[must_use]
    pub fn has_impl(&self, kind: &str) -> bool {
        self.by_kind.get(kind).is_some_and(|flags| flags.has_impl)
    }

    fn export_name(&self, kind: &str) -> Result<&str> {
        self.by_kind
            .get(kind)
            .map(|flags| flags.export_name.as_str())
            .ok_or_else(|| anyhow!("no target kind found for kind `{kind}`"))
    }
}

/// Run a single target's target kind impl and collect its declared actions
/// and provider record.
///
/// `dep_providers` supplies the provider record each in-graph
/// dependency already returned; impls iterate it to gather declared
/// provider fields.
pub fn analyze_target(
    target: &GraphTarget,
    workspace_root: &Path,
    dep_providers: &[JsonValue],
) -> Result<AnalysisResult> {
    AnalysisEngine::new()?.analyze_target(target, workspace_root, dep_providers)
}

struct TargetAnalysis<'a> {
    target: &'a GraphTarget,
    workspace_root: &'a Path,
    dep_providers: &'a [&'a JsonValue],
    dependency_providers: &'a BTreeMap<String, Vec<&'a JsonValue>>,
    capability: &'a str,
    options: AnalysisOptions,
    configuration: &'a crate::manifest::BuildConfiguration,
    configuration_path_suffix: &'a str,
}

fn analyze_target_with_host_cache(
    module: &FrozenModule,
    target_kind_callbacks: &TargetKindCallbacks,
    host_cache: HostCache,
    analysis: &TargetAnalysis<'_>,
) -> Result<AnalysisResult> {
    let build_dir = format!(
        ".once/out/{}{}",
        analysis.target.label.id, analysis.configuration_path_suffix
    );
    let scratch_dir = format!(
        ".once/tmp/analysis/{}{}",
        analysis.target.label.id, analysis.configuration_path_suffix
    );
    let store = AnalysisStore::with_host_cache(
        analysis.workspace_root.to_path_buf(),
        analysis.target.label.package.clone(),
        build_dir.clone(),
        host_cache,
    );

    let (store, result) = with_active_store(store, || {
        analyze_in_starlark(
            module,
            target_kind_callbacks,
            analysis,
            &build_dir,
            &scratch_dir,
        )
    });
    let provider = result?;
    Ok(AnalysisResult {
        actions: store.actions,
        provider,
        declared_outputs: store.declared_outputs,
        observations: store.observations,
    })
}

/// Returns true if the target kind declares an `impl` callable in the
/// prelude. The driver consults this before walking deps so
/// capability-only target kinds don't trigger analysis of their deps.
pub fn target_kind_has_impl(kind: &str) -> Result<bool> {
    Ok(AnalysisEngine::new()?.target_kind_has_impl(kind))
}

fn compile_target_kind_callbacks(
    path: &str,
    source: &str,
    extract_target_kind_schemas: bool,
) -> Result<(FrozenModule, TargetKindCallbacks, Vec<TargetKindSchema>)> {
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse(path, source.to_string(), &Dialect::Standard)
            .map_err(|error| anyhow!("prelude parse failed: {error:?}"))?;
        let globals = globals_for_prelude();
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)
            .map_err(|error| anyhow!("prelude eval failed: {error:?}"))?;
        let mut by_kind = BTreeMap::new();
        let target_kinds = crate::modules::exported_target_kind_values(&module);
        let target_kind_schemas = if extract_target_kind_schemas {
            crate::graph::target_kind_schemas_from_compiled_exports(&target_kinds)
                .map_err(|error| anyhow!(error))?
        } else {
            Vec::new()
        };
        for export in target_kinds {
            let dict = DictRef::from_value(export.value)
                .ok_or_else(|| anyhow!("target kind export `{}` is not a dict", export.name))?;
            let target_kind = crate::modules::target_kind(export.value, export.name)
                .map_err(|message| anyhow!(message))?;
            let impl_value = dict.get_str("impl");
            if by_kind
                .insert(
                    target_kind.clone(),
                    TargetKindCallbackFlags {
                        export_name: export.name.to_string(),
                        has_impl: impl_value.is_some_and(|value| !value.is_none()),
                    },
                )
                .is_some()
            {
                return Err(anyhow!(
                    "target kind `{target_kind}` is declared more than once"
                ));
            }
        }
        drop(eval);
        let module = module
            .freeze()
            .map_err(|error| anyhow!("prelude freeze failed: {error:?}"))?;
        Ok((module, TargetKindCallbacks { by_kind }, target_kind_schemas))
    })
}

fn analyze_in_starlark(
    compiled_module: &FrozenModule,
    target_kind_callbacks: &TargetKindCallbacks,
    analysis: &TargetAnalysis<'_>,
    build_dir: &str,
    scratch_dir: &str,
) -> Result<JsonValue> {
    Module::with_temp_heap(|module| {
        let export_name = target_kind_callbacks.export_name(&analysis.target.kind)?;
        let target_kind = compiled_module
            .get(export_name)
            .with_context(|| format!("reading target kind export `{export_name}`"))?;
        let target_kind = module.heap().access_owned_frozen_value(&target_kind);
        let dict = DictRef::from_value(target_kind)
            .ok_or_else(|| anyhow!("target kind export `{export_name}` is not a dict"))?;
        let impl_value = dict.get_str("impl").ok_or_else(|| {
            anyhow!(
                "target kind `{}` is missing `impl` field",
                analysis.target.kind
            )
        })?;
        if impl_value.is_none() {
            return Ok(JsonValue::Null);
        }
        let mut eval = Evaluator::new(&module);
        let ctx = build_ctx(&eval, analysis, build_dir, scratch_dir);
        let provider = eval
            .eval_function(impl_value, &[ctx], &[])
            .map_err(|error| {
                analysis_failure(analysis.target, "implementation", &error.to_string())
            })?;
        Ok(value_to_json(provider))
    })
}

fn resolve_targets_in_starlark<T>(
    path: &str,
    source: &str,
    workspace_root: &Path,
    host_cache: &HostCache,
    configuration: &crate::manifest::BuildConfiguration,
    observations: &mut AnalysisObservations,
    operation: impl FnOnce(&mut ResolverCallback<'_>) -> Result<T>,
) -> Result<T> {
    let recorded = std::cell::RefCell::new(AnalysisObservations::default());
    let result = Module::with_temp_heap(|module| {
        let ast = AstModule::parse(path, source.to_string(), &Dialect::Standard)
            .map_err(|error| anyhow!("prelude parse failed: {error:?}"))?;
        let globals = globals_for_prelude();
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)
            .map_err(|error| anyhow!("prelude eval failed: {error:?}"))?;
        let target_kinds = crate::modules::exported_target_kind_values(&module);
        let mut resolve = |target: &GraphTarget,
                           files: &BTreeMap<String, String>|
         -> Result<Option<JsonValue>> {
            let resolver = find_callback_for_kind(&target_kinds, &target.kind, "resolver")?;
            let Some(resolver) = resolver else {
                return Ok(None);
            };
            let build_dir = format!(".once/out/{}", target.label.id);
            let store = AnalysisStore::with_host_cache(
                workspace_root.to_path_buf(),
                target.label.package.clone(),
                build_dir,
                host_cache.clone(),
            );
            let (store, result) = with_active_store(store, || -> Result<Option<JsonValue>> {
                let ctx = build_resolver_ctx(&eval, target, files, configuration);
                let graph_data = eval
                    .eval_function(resolver, &[ctx], &[])
                    .map_err(|error| analysis_failure(target, "resolver", &error.to_string()))?;
                Ok(Some(value_to_json(graph_data)))
            });
            let value = result?;
            if !store.actions.is_empty() || !store.declared_outputs.is_empty() {
                return Err(anyhow!(
                    "resolver for {} declared actions or outputs; resolvers may only return graph data",
                    target.label.id
                ));
            }
            // One ledger for the whole expansion: the resolvers run in sequence
            // against one host, and what any of them read is what the derived
            // graph depends on.
            recorded.borrow_mut().absorb(store.observations);
            Ok(value)
        };
        operation(&mut resolve)
    });
    *observations = recorded.into_inner();
    result
}

/// The host tool failure behind `message`, when one explains it.
///
/// A resolver stops at its first failure, so the last recorded one is the
/// candidate. Requiring the executable to appear in `message` is what ties the
/// two together: without it a stale record from an earlier target could
/// describe a failure it had nothing to do with.
fn causing_tool_failure(message: &str) -> Option<HostToolFailure> {
    let failure = with_store(|store| store.and_then(|store| store.host_cache.last_tool_failure()))?;
    message.contains(&failure.program).then_some(failure)
}

/// Report a host tool that ran and refused, leading with the executable and
/// the reason rather than the Starlark traceback that wraps them.
///
/// The traceback names the prelude function that reached for the tool, which
/// reads as a fault in Once. The fault is in the tool: Once ran whatever the
/// host resolves for that name, so the repair is to fix that executable, and
/// the way to see it plainly is to run it outside Once.
fn tool_failure_diagnostic(
    target: &GraphTarget,
    stage: &str,
    failure: &HostToolFailure,
) -> anyhow::Error {
    let name = failure.name();
    let mut message = format!(
        "target kind {stage} failed for `{target}`: `{name}` is not usable\n\n  resolved to  {program}\n  exited with  {status}",
        target = target.label.id,
        program = failure.program,
        status = failure.status,
    );
    if !failure.stderr.is_empty() {
        message.push_str("\n  stderr       ");
        message.push_str(&failure.stderr.replace('\n', "\n               "));
    }
    // Carried in the message, not only in the repair: the repair reaches
    // structured consumers, and this is the line a person needs to read.
    let repair = format!(
        "Once runs the `{name}` the host resolves, not one of its own. Run `{name}` yourself from the workspace root to see the same failure, then repair that installation."
    );
    message.push_str("\n\n");
    message.push_str(&repair);
    let diagnostic = Diagnostic::new("host_tool_not_usable", message)
        .with_target(&target.label.id)
        .with_repair(repair);
    anyhow!(AnalysisFailure { diagnostic })
}

fn analysis_failure(target: &GraphTarget, stage: &str, message: &str) -> anyhow::Error {
    if let Some(failure) = causing_tool_failure(message) {
        return tool_failure_diagnostic(target, stage, &failure);
    }
    let (code, repair) = if message.contains("select()") && message.contains("no") {
        (
            "select_no_matching_branch",
            "Add a branch matching the target configuration or add a `default` branch",
        )
    } else if message.contains("not found on PATH") {
        (
            "required_tool_not_found",
            "Install the required tool or configure the target kind to use an available executable",
        )
    } else if message.contains("not implemented") {
        (
            "unimplemented_attr",
            "Remove the unavailable attribute or choose a target kind that implements it",
        )
    } else {
        (
            "target_kind_analysis_failed",
            "Inspect the target kind schema, correct the target attributes or toolchain configuration, and retry",
        )
    };
    let mut diagnostic = Diagnostic::new(
        code,
        format!(
            "target kind {stage} failed for `{}`: {message}",
            target.label.id
        ),
    )
    .with_target(&target.label.id)
    .with_repair(repair);
    if let Some(attribute) = quoted_attribute(message) {
        diagnostic = diagnostic.with_attribute(attribute);
    }
    anyhow!(AnalysisFailure { diagnostic })
}

fn quoted_attribute(message: &str) -> Option<String> {
    let rest = message.split_once("attribute `")?.1;
    let (attribute, _) = rest.split_once('`')?;
    (!attribute.is_empty()).then(|| attribute.to_string())
}

fn find_callback_for_kind<'v>(
    target_kinds: &[crate::modules::TargetKindExport<'v>],
    kind: &str,
    field: &str,
) -> Result<Option<Value<'v>>> {
    for export in target_kinds {
        let dict = DictRef::from_value(export.value)
            .ok_or_else(|| anyhow!("target kind export `{}` is not a dict", export.name))?;
        let target_kind = crate::modules::target_kind(export.value, export.name)
            .map_err(|message| anyhow!(message))?;
        if target_kind != kind {
            continue;
        }
        let value = dict
            .get_str(field)
            .ok_or_else(|| anyhow!("target kind `{kind}` is missing `{field}` field"))?;
        return Ok((!value.is_none()).then_some(value));
    }
    Err(anyhow!("no target kind found for kind `{kind}`"))
}

fn build_resolver_ctx<'v>(
    eval: &Evaluator<'v, '_, '_>,
    target: &GraphTarget,
    files: &BTreeMap<String, String>,
    configuration: &crate::manifest::BuildConfiguration,
) -> Value<'v> {
    let heap = eval.heap();
    let label = heap.alloc(AllocDict([
        ("package", heap.alloc(target.label.package.clone())),
        ("name", heap.alloc(target.label.name.clone())),
        ("id", heap.alloc(target.label.id.clone())),
    ]));
    let attr_pairs = target
        .attrs
        .iter()
        .map(|(key, value)| (key.clone(), attr_value_to_starlark(eval, value)))
        .collect::<Vec<_>>();
    let attr = heap.alloc(AllocDict(attr_pairs));
    let file_pairs = files
        .iter()
        .map(|(path, contents)| (path.clone(), heap.alloc(contents.clone())))
        .collect::<Vec<_>>();
    let file_values = heap.alloc(AllocDict(file_pairs));
    let configuration = configuration_value(eval, configuration);
    heap.alloc(AllocDict([
        ("label", label),
        ("attr", attr),
        ("attrs", attr),
        ("srcs", heap.alloc(target.srcs.clone())),
        ("files", file_values),
        ("configuration", configuration),
    ]))
}

fn build_ctx<'v>(
    eval: &Evaluator<'v, '_, '_>,
    analysis: &TargetAnalysis<'_>,
    build_dir: &str,
    scratch_dir: &str,
) -> Value<'v> {
    let heap = eval.heap();
    let label = heap.alloc(AllocDict([
        ("package", heap.alloc(analysis.target.label.package.clone())),
        ("name", heap.alloc(analysis.target.label.name.clone())),
        ("id", heap.alloc(analysis.target.label.id.clone())),
    ]));
    let attr_pairs: Vec<(String, Value<'v>)> = analysis
        .target
        .attrs
        .iter()
        .map(|(key, value)| (key.clone(), attr_value_to_starlark(eval, value)))
        .collect();
    let attr = heap.alloc(AllocDict(attr_pairs));
    let srcs_value = heap.alloc(analysis.target.srcs.clone());
    let dep_values: Vec<Value<'v>> = analysis
        .dep_providers
        .iter()
        .map(|provider| json_to_value(eval, provider))
        .collect();
    let deps = heap.alloc(dep_values);
    let mut providers_by_role = Vec::with_capacity(analysis.dependency_providers.len() + 1);
    providers_by_role.push(("deps".to_string(), deps));
    providers_by_role.extend(
        analysis
            .dependency_providers
            .iter()
            .map(|(role, providers)| {
                let values = providers
                    .iter()
                    .map(|provider| json_to_value(eval, provider))
                    .collect::<Vec<_>>();
                (role.clone(), heap.alloc(values))
            }),
    );
    let deps_by_role = heap.alloc(AllocDict(providers_by_role));
    let is_run = analysis.capability == "run";
    let run = heap.alloc(AllocDict([
        (
            "visible",
            Value::new_bool(is_run && analysis.options.run_visible),
        ),
        (
            "args",
            heap.alloc(if is_run {
                analysis.options.run_arguments.clone()
            } else {
                Vec::new()
            }),
        ),
    ]));
    let test = heap.alloc(AllocDict([
        ("filters", heap.alloc(analysis.options.test_filters.clone())),
        (
            "batch_id",
            analysis
                .options
                .test_batch_id
                .as_ref()
                .map_or(Value::new_none(), |id| heap.alloc(id.clone())),
        ),
    ]));
    let configuration = configuration_value(eval, analysis.configuration);
    heap.alloc(AllocDict([
        ("label", label),
        ("attr", attr),
        ("srcs", srcs_value),
        ("deps", deps),
        ("deps_by_role", deps_by_role),
        ("build_dir", heap.alloc(build_dir.to_string())),
        ("scratch_dir", heap.alloc(scratch_dir.to_string())),
        ("capability", heap.alloc(analysis.capability.to_string())),
        ("run", run),
        ("test", test),
        ("configuration", configuration),
    ]))
}

fn configuration_value<'v>(
    eval: &Evaluator<'v, '_, '_>,
    configuration: &crate::manifest::BuildConfiguration,
) -> Value<'v> {
    let heap = eval.heap();
    heap.alloc(AllocDict([
        ("os", heap.alloc(configuration.os.clone())),
        ("arch", heap.alloc(configuration.arch.clone())),
        ("tokens", heap.alloc(configuration.tokens.clone())),
    ]))
}
