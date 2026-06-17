use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use serde_json::Value as JsonValue;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};
use starlark::values::dict::{AllocDict, DictRef};
use starlark::values::list::ListRef;
use starlark::values::Value;

use super::globals::globals_for_prelude;
use super::store::{with_active_store, AnalysisStore, DeclaredAction, HostCache};
use super::values::{attr_value_to_starlark, json_to_value, value_to_json};
use crate::graph::GraphTarget;

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
/// target. It caches cheap rule metadata and generic host lookups
/// (`host_which`, `host_command`) while still evaluating each target's
/// Starlark impl in an isolated module heap.
#[derive(Clone)]
pub struct AnalysisEngine {
    source_path: Arc<str>,
    source: Arc<str>,
    rule_impls: RuleImpls,
    host_cache: HostCache,
}

impl fmt::Debug for AnalysisEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnalysisEngine")
            .field("source_path", &self.source_path)
            .field("source_len", &self.source.len())
            .field("rule_impls", &self.rule_impls)
            .field("host_cache", &self.host_cache)
            .finish()
    }
}

impl AnalysisEngine {
    pub fn new() -> Result<Self> {
        Self::from_source_with_path(
            crate::rules::BUILT_IN_RULE_PATH,
            crate::rules::built_in_rule_source(),
        )
    }

    pub fn for_workspace(root: &Path) -> Result<Self> {
        let source = crate::rules::combined_rule_source_for_workspace(root)?;
        Self::from_source_with_path(crate::rules::COMBINED_RULE_PATH, source)
    }

    pub fn from_source(source: impl Into<Arc<str>>) -> Result<Self> {
        Self::from_source_with_path(crate::rules::BUILT_IN_RULE_PATH, source)
    }

    fn from_source_with_path(
        source_path: impl Into<Arc<str>>,
        source: impl Into<Arc<str>>,
    ) -> Result<Self> {
        let source_path = source_path.into();
        let source = source.into();
        let rule_impls = parse_rule_impls(&source_path, &source)?;
        Ok(Self {
            source_path,
            source,
            rule_impls,
            host_cache: HostCache::default(),
        })
    }

    #[must_use]
    pub fn rule_source(&self) -> &str {
        &self.source
    }

    #[must_use]
    pub fn rule_has_impl(&self, kind: &str) -> bool {
        self.rule_impls.has_impl(kind)
    }

    /// Run a single target's rule impl and collect its declared
    /// actions and provider record.
    ///
    /// `dep_providers` supplies the provider record each in-graph
    /// dependency already returned; impls iterate it to gather
    /// whatever transitive state their rule family carries (search
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
        analyze_target_with_host_cache(
            &self.source_path,
            &self.source,
            self.host_cache.clone(),
            target,
            workspace_root,
            dep_providers,
            capability,
        )
    }
}

/// Cached view of which rules declare executable impls.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RuleImpls {
    by_kind: BTreeMap<String, bool>,
}

impl RuleImpls {
    #[must_use]
    pub fn has_impl(&self, kind: &str) -> bool {
        self.by_kind.get(kind).copied().unwrap_or(false)
    }
}

/// Run a single target's rule impl and collect its declared actions
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

fn analyze_target_with_host_cache(
    source_path: &str,
    source: &str,
    host_cache: HostCache,
    target: &GraphTarget,
    workspace_root: &Path,
    dep_providers: &[JsonValue],
    capability: &str,
) -> Result<AnalysisResult> {
    let build_dir = format!(".once/out/{}", target.label.id);
    let store = AnalysisStore::with_host_cache(
        workspace_root.to_path_buf(),
        target.label.package.clone(),
        build_dir.clone(),
        host_cache,
    );

    let (store, result) = with_active_store(store, || {
        analyze_in_starlark(
            source_path,
            source,
            target,
            dep_providers,
            &build_dir,
            capability,
        )
    });
    let provider = result?;
    Ok(AnalysisResult {
        actions: store.actions,
        provider,
        declared_outputs: store.declared_outputs,
    })
}

/// Returns true if the rule for `kind` declares an `impl` callable in
/// the prelude. The driver consults this before walking deps so
/// capability-only rules don't trigger analysis of their deps.
pub fn rule_has_impl(kind: &str) -> Result<bool> {
    Ok(AnalysisEngine::new()?.rule_has_impl(kind))
}

fn parse_rule_impls(path: &str, source: &str) -> Result<RuleImpls> {
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse(path, source.to_string(), &Dialect::Standard)
            .map_err(|error| anyhow!("prelude parse failed: {error:?}"))?;
        let globals = globals_for_prelude();
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)
            .map_err(|error| anyhow!("prelude eval failed: {error:?}"))?;
        let rules_value = module
            .get("RULES")
            .context("prelude is missing RULES export")?;
        let rules = ListRef::from_value(rules_value).context("RULES is not a list")?;
        let mut by_kind = BTreeMap::new();
        for rule in rules.iter() {
            let dict =
                DictRef::from_value(rule).ok_or_else(|| anyhow!("RULES entry is not a dict"))?;
            let Some(rule_kind) = dict.get_str("kind").and_then(Value::unpack_str) else {
                continue;
            };
            let impl_value = dict.get_str("impl");
            if by_kind
                .insert(
                    rule_kind.to_string(),
                    impl_value.is_some_and(|value| !value.is_none()),
                )
                .is_some()
            {
                return Err(anyhow!(
                    "rule kind `{rule_kind}` is declared more than once"
                ));
            }
        }
        Ok(RuleImpls { by_kind })
    })
}

fn analyze_in_starlark(
    path: &str,
    source: &str,
    target: &GraphTarget,
    dep_providers: &[JsonValue],
    build_dir: &str,
    capability: &str,
) -> Result<JsonValue> {
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse(path, source.to_string(), &Dialect::Standard)
            .map_err(|error| anyhow!("prelude parse failed: {error:?}"))?;
        let globals = globals_for_prelude();
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)
            .map_err(|error| anyhow!("prelude eval failed: {error:?}"))?;
        let rules_value = module
            .get("RULES")
            .context("prelude is missing RULES export")?;
        let rules = ListRef::from_value(rules_value).context("RULES is not a list")?;
        let impl_value = find_impl_for_kind(rules, &target.kind)?;
        let Some(impl_value) = impl_value else {
            return Ok(JsonValue::Null);
        };
        let ctx = build_ctx(&eval, target, dep_providers, build_dir, capability);
        let provider = eval
            .eval_function(impl_value, &[ctx], &[])
            .map_err(|error| anyhow!("impl eval failed for {}: {error:?}", target.label.id))?;
        Ok(value_to_json(provider))
    })
}

fn find_impl_for_kind<'v>(rules: &ListRef<'v>, kind: &str) -> Result<Option<Value<'v>>> {
    for rule in rules.iter() {
        let dict = DictRef::from_value(rule).ok_or_else(|| anyhow!("RULES entry is not a dict"))?;
        let rule_kind = dict.get_str("kind").and_then(Value::unpack_str);
        if rule_kind == Some(kind) {
            let impl_value = dict
                .get_str("impl")
                .ok_or_else(|| anyhow!("rule `{kind}` is missing `impl` field"))?;
            if impl_value.is_none() {
                return Ok(None);
            }
            return Ok(Some(impl_value));
        }
    }
    Err(anyhow!("no rule found for kind `{kind}`"))
}

fn build_ctx<'v>(
    eval: &Evaluator<'v, '_, '_>,
    target: &GraphTarget,
    dep_providers: &[JsonValue],
    build_dir: &str,
    capability: &str,
) -> Value<'v> {
    let heap = eval.heap();
    let label = heap.alloc(AllocDict([
        ("package", heap.alloc(target.label.package.clone())),
        ("name", heap.alloc(target.label.name.clone())),
        ("id", heap.alloc(target.label.id.clone())),
    ]));
    let attr_pairs: Vec<(String, Value<'v>)> = target
        .attrs
        .iter()
        .map(|(key, value)| (key.clone(), attr_value_to_starlark(eval, value)))
        .collect();
    let attr = heap.alloc(AllocDict(attr_pairs));
    let srcs_value = heap.alloc(target.srcs.clone());
    let dep_values: Vec<Value<'v>> = dep_providers
        .iter()
        .map(|provider| json_to_value(eval, provider))
        .collect();
    let deps = heap.alloc(dep_values);
    heap.alloc(AllocDict([
        ("label", label),
        ("attr", attr),
        ("srcs", srcs_value),
        ("deps", deps),
        ("build_dir", heap.alloc(build_dir.to_string())),
        ("capability", heap.alloc(capability.to_string())),
    ]))
}
