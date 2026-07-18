use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::Value as JsonValue;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};
use starlark::values::dict::{AllocDict, DictRef};
use starlark::values::Value;

use super::globals::globals_for_prelude;
use super::store::{with_active_store, AnalysisStore, DeclaredAction, HostCache};
use super::values::{attr_value_to_starlark, json_to_value, value_to_json};
use crate::graph::GraphTarget;

/// Extra execution context supplied by command surfaces.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnalysisOptions {
    /// Request a visible runtime surface for run capabilities when the target
    /// kind supports one.
    pub run_visible: bool,
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

/// Command-scoped analysis helper.
///
/// Construct this once for a graph command and reuse it for every
/// target. It caches cheap target kind metadata and generic host lookups
/// (`host_which`, `host_command`) while still evaluating each target's
/// Starlark impl in an isolated module heap.
#[derive(Clone)]
pub struct AnalysisEngine {
    source_path: Arc<str>,
    source: Arc<str>,
    target_kind_impls: TargetKindImpls,
    host_cache: HostCache,
    options: AnalysisOptions,
}

impl fmt::Debug for AnalysisEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnalysisEngine")
            .field("source_path", &self.source_path)
            .field("source_len", &self.source.len())
            .field("target_kind_impls", &self.target_kind_impls)
            .field("host_cache", &self.host_cache)
            .field("options", &self.options)
            .finish()
    }
}

impl AnalysisEngine {
    pub fn new() -> Result<Self> {
        Self::from_source_with_path(
            crate::modules::BUILT_IN_MODULE_PATH,
            crate::modules::built_in_module_source(),
            AnalysisOptions::default(),
        )
    }

    pub fn for_workspace(root: &Path) -> Result<Self> {
        Self::for_workspace_with_options(root, AnalysisOptions::default())
    }

    pub fn for_workspace_with_options(root: &Path, options: AnalysisOptions) -> Result<Self> {
        let source = crate::modules::combined_module_source_for_workspace(root)?;
        Self::from_source_with_path(crate::modules::COMBINED_MODULE_PATH, source, options)
    }

    pub fn for_workspace_with_options_and_tool_paths(
        root: &Path,
        options: AnalysisOptions,
        tool_paths: BTreeMap<String, String>,
    ) -> Result<Self> {
        let mut engine = Self::for_workspace_with_options(root, options)?;
        engine.host_cache = HostCache::with_tool_paths(tool_paths);
        Ok(engine)
    }

    pub fn from_source(source: impl Into<Arc<str>>) -> Result<Self> {
        Self::from_source_with_path(
            crate::modules::BUILT_IN_MODULE_PATH,
            source,
            AnalysisOptions::default(),
        )
    }

    pub fn from_source_with_options(
        source: impl Into<Arc<str>>,
        options: AnalysisOptions,
    ) -> Result<Self> {
        Self::from_source_with_path(crate::modules::BUILT_IN_MODULE_PATH, source, options)
    }

    fn from_source_with_path(
        source_path: impl Into<Arc<str>>,
        source: impl Into<Arc<str>>,
        options: AnalysisOptions,
    ) -> Result<Self> {
        let source_path = source_path.into();
        let source = source.into();
        let target_kind_impls = parse_target_kind_impls(&source_path, &source)?;
        Ok(Self {
            source_path,
            source,
            target_kind_impls,
            host_cache: HostCache::default(),
            options,
        })
    }

    #[must_use]
    pub fn module_source(&self) -> &str {
        &self.source
    }

    #[must_use]
    pub fn target_kind_has_impl(&self, kind: &str) -> bool {
        self.target_kind_impls.has_impl(kind)
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
        let analysis = TargetAnalysis {
            target,
            workspace_root,
            dep_providers,
            capability,
            options: self.options.clone(),
        };
        analyze_target_with_host_cache(
            &self.source_path,
            &self.source,
            self.host_cache.clone(),
            &analysis,
        )
    }
}

/// Cached view of which target kinds declare executable impls.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetKindImpls {
    by_kind: BTreeMap<String, bool>,
}

impl TargetKindImpls {
    #[must_use]
    pub fn has_impl(&self, kind: &str) -> bool {
        self.by_kind.get(kind).copied().unwrap_or(false)
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
    dep_providers: &'a [JsonValue],
    capability: &'a str,
    options: AnalysisOptions,
}

fn analyze_target_with_host_cache(
    source_path: &str,
    source: &str,
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
        analyze_in_starlark(source_path, source, analysis, &build_dir, &scratch_dir)
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

fn parse_target_kind_impls(path: &str, source: &str) -> Result<TargetKindImpls> {
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse(path, source.to_string(), &Dialect::Standard)
            .map_err(|error| anyhow!("prelude parse failed: {error:?}"))?;
        let globals = globals_for_prelude();
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)
            .map_err(|error| anyhow!("prelude eval failed: {error:?}"))?;
        let mut by_kind = BTreeMap::new();
        for export in crate::modules::exported_target_kind_values(&module) {
            let dict = DictRef::from_value(export.value)
                .ok_or_else(|| anyhow!("target kind export `{}` is not a dict", export.name))?;
            let target_kind = crate::modules::target_kind(export.value, export.name)
                .map_err(|message| anyhow!(message))?;
            let impl_value = dict.get_str("impl");
            if by_kind
                .insert(
                    target_kind.clone(),
                    impl_value.is_some_and(|value| !value.is_none()),
                )
                .is_some()
            {
                return Err(anyhow!(
                    "target kind `{target_kind}` is declared more than once"
                ));
            }
        }
        Ok(TargetKindImpls { by_kind })
    })
}

fn analyze_in_starlark(
    path: &str,
    source: &str,
    analysis: &TargetAnalysis<'_>,
    build_dir: &str,
    scratch_dir: &str,
) -> Result<JsonValue> {
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse(path, source.to_string(), &Dialect::Standard)
            .map_err(|error| anyhow!("prelude parse failed: {error:?}"))?;
        let globals = globals_for_prelude();
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)
            .map_err(|error| anyhow!("prelude eval failed: {error:?}"))?;
        let target_kinds = crate::modules::exported_target_kind_values(&module);
        let impl_value = find_impl_for_kind(&target_kinds, &analysis.target.kind)?;
        let Some(impl_value) = impl_value else {
            return Ok(JsonValue::Null);
        };
        let ctx = build_ctx(&eval, analysis, build_dir, scratch_dir);
        let provider = eval
            .eval_function(impl_value, &[ctx], &[])
            .map_err(|error| {
                anyhow!(
                    "impl eval failed for {}: {error:?}",
                    analysis.target.label.id
                )
            })?;
        Ok(value_to_json(provider))
    })
}

fn find_impl_for_kind<'v>(
    target_kinds: &[crate::modules::TargetKindExport<'v>],
    kind: &str,
) -> Result<Option<Value<'v>>> {
    for export in target_kinds {
        let dict = DictRef::from_value(export.value)
            .ok_or_else(|| anyhow!("target kind export `{}` is not a dict", export.name))?;
        let target_kind = crate::modules::target_kind(export.value, export.name)
            .map_err(|message| anyhow!(message))?;
        if target_kind == kind {
            let impl_value = dict
                .get_str("impl")
                .ok_or_else(|| anyhow!("target kind `{kind}` is missing `impl` field"))?;
            if impl_value.is_none() {
                return Ok(None);
            }
            return Ok(Some(impl_value));
        }
    }
    Err(anyhow!("no target kind found for kind `{kind}`"))
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
    let run = heap.alloc(AllocDict([(
        "visible",
        Value::new_bool(analysis.options.run_visible),
    )]));
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
    heap.alloc(AllocDict([
        ("label", label),
        ("attr", attr),
        ("srcs", srcs_value),
        ("deps", deps),
        ("build_dir", heap.alloc(build_dir.to_string())),
        ("scratch_dir", heap.alloc(scratch_dir.to_string())),
        ("capability", heap.alloc(analysis.capability.to_string())),
        ("run", run),
        ("test", test),
    ]))
}
