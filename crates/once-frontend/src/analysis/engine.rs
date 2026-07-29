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

use super::globals::globals_for_prelude;
use super::store::{
    with_active_store, AnalysisStore, CachedToolCommand, DeclaredAction, HostCache,
};
use super::values::{attr_value_to_starlark, json_to_value, value_to_json};
use crate::graph::{Diagnostic, GraphTarget, TargetKindSchema};
use crate::Target;

type ResolverCallback<'a> =
    dyn FnMut(&GraphTarget, &BTreeMap<String, String>) -> Result<Option<JsonValue>> + 'a;

/// Extra execution context supplied by command surfaces.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
            .finish()
    }
}

impl AnalysisEngine {
    pub fn new() -> Result<Self> {
        Self::from_source_with_path(
            crate::modules::BUILT_IN_MODULE_PATH,
            crate::modules::built_in_module_source(),
            AnalysisOptions::default(),
            false,
        )
    }

    pub fn for_workspace(root: &Path) -> Result<Self> {
        Self::for_workspace_with_options(root, AnalysisOptions::default())
    }

    pub(crate) fn resolve_workspace_targets<T>(
        root: &Path,
        operation: impl FnOnce(&mut ResolverCallback<'_>) -> Result<T>,
    ) -> Result<T> {
        let source = crate::modules::combined_module_source_for_workspace(root)?;
        resolve_targets_in_starlark(
            crate::modules::COMBINED_MODULE_PATH,
            &source,
            root,
            &HostCache::default(),
            &crate::manifest::load_workspace_configuration(root)?,
            operation,
        )
    }

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

    pub fn for_workspace_with_options_and_target_kinds(
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

    pub fn for_workspace_with_options_and_tool_paths(
        root: &Path,
        options: AnalysisOptions,
        tool_paths: BTreeMap<String, String>,
    ) -> Result<Self> {
        Ok(Self::for_workspace_with_options(root, options)?.with_tool_paths(tool_paths))
    }

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
        crate::graph::load_graph_workspace_with_compiled_schemas_and_targets(
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

    #[must_use]
    pub fn with_tool_cache(
        mut self,
        tool_paths: BTreeMap<String, String>,
        commands: Vec<CachedToolCommand>,
    ) -> Self {
        self.host_cache = HostCache::with_tool_cache(tool_paths, commands);
        self
    }

    pub fn cacheable_tool_commands(&self) -> Result<Vec<CachedToolCommand>> {
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
}

fn analyze_target_with_host_cache(
    module: &FrozenModule,
    target_kind_callbacks: &TargetKindCallbacks,
    host_cache: HostCache,
    analysis: &TargetAnalysis<'_>,
) -> Result<AnalysisResult> {
    let build_dir = format!(".once/out/{}", analysis.target.label.id);
    let scratch_dir = format!(".once/tmp/analysis/{}", analysis.target.label.id);
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
    operation: impl FnOnce(&mut ResolverCallback<'_>) -> Result<T>,
) -> Result<T> {
    Module::with_temp_heap(|module| {
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
            Ok(value)
        };
        operation(&mut resolve)
    })
}

fn analysis_failure(target: &GraphTarget, stage: &str, message: &str) -> anyhow::Error {
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
