//! Starlark rule implementation analysis.
//!
//! Rule schemas declared in the prelude carry an optional `impl`
//! callable. The analysis pass evaluates that callable for one target
//! at a time with a `ctx` dict and collects the actions the impl
//! declares through globals defined here.
//!
//! The globals are deliberately generic: `host_arch`, `host_which`,
//! `host_command`, `glob`, `declare_output`, `run_action`. Anything
//! domain-specific (xcrun discovery, SDK names, triple rendering,
//! file extension filtering for a particular language) lives in the
//! starlark prelude, not in Rust. The Rust side is an executor for
//! whatever the prelude declares.
//!
//! State threading uses a thread-local instead of `Evaluator::extra`
//! because `extra` requires implementing the `unsafe`
//! `ProvidesStaticType` trait, and the workspace forbids `unsafe`
//! code. [`with_active_store`] installs a store for the duration of an
//! evaluation and tears it back out, so concurrent evaluations on
//! different threads never trip over each other.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use serde_json::Value as JsonValue;
use starlark::environment::{Globals, GlobalsBuilder, Module};
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::syntax::{AstModule, Dialect};
use starlark::values::dict::{AllocDict, DictRef};
use starlark::values::list::ListRef;
use starlark::values::none::NoneType;
use starlark::values::Value;

use crate::graph::GraphTarget;
use crate::target::AttrValue;

/// A single command declared by a rule impl through `run_action`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeclaredAction {
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
}

/// Per-target collection of declared outputs, actions, and the host
/// context the rule impl needs (workspace root + package for globbing,
/// build dir for output declaration).
#[derive(Debug, Default)]
pub struct AnalysisStore {
    pub workspace_root: PathBuf,
    pub package: String,
    pub build_dir: String,
    pub declared_outputs: Vec<String>,
    pub actions: Vec<DeclaredAction>,
    host_cache: HostCache,
}

impl AnalysisStore {
    #[must_use]
    pub fn new(workspace_root: PathBuf, package: String, build_dir: String) -> Self {
        Self::with_host_cache(workspace_root, package, build_dir, HostCache::default())
    }

    #[must_use]
    fn with_host_cache(
        workspace_root: PathBuf,
        package: String,
        build_dir: String,
        host_cache: HostCache,
    ) -> Self {
        Self {
            workspace_root,
            package,
            build_dir,
            declared_outputs: Vec::new(),
            actions: Vec::new(),
            host_cache,
        }
    }
}

/// Key for cached command results.
///
/// `env` is included so two calls with the same argv but different
/// `DEVELOPER_DIR` (or any other override) get distinct cache slots,
/// which is the whole point of accepting an env at all.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CommandKey {
    argv: Vec<String>,
    env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
struct HostCache {
    which: Arc<Mutex<BTreeMap<String, Option<String>>>>,
    commands: Arc<Mutex<BTreeMap<CommandKey, String>>>,
}

impl HostCache {
    /// Resolve `name` on `PATH`, caching the result.
    ///
    /// The lock is released before the filesystem walk so concurrent
    /// `host_which` calls for different binaries don't serialise on
    /// each other.
    fn which(&self, name: &str) -> Result<Option<String>> {
        if let Some(cached) = self.lock_which()?.get(name).cloned() {
            return Ok(cached);
        }
        let resolved = which_on_path(name).map(|path| path.display().to_string());
        // Cache the result after the slow path. A racing concurrent
        // caller may have populated the slot already; that's fine, we
        // just overwrite with an equivalent value.
        self.lock_which()?
            .insert(name.to_string(), resolved.clone());
        Ok(resolved)
    }

    /// Run `argv` (optionally with extra env vars) and cache its
    /// stdout.
    ///
    /// The lock is released before `Command::output` so other analyses
    /// running on sibling targets aren't blocked by a slow xcrun spawn.
    fn command(&self, argv: &[String], env: &BTreeMap<String, String>) -> Result<String> {
        let key = CommandKey {
            argv: argv.to_vec(),
            env: env.clone(),
        };
        if let Some(cached) = self.lock_commands()?.get(&key).cloned() {
            return Ok(cached);
        }
        let mut iter = argv.iter();
        let program = iter
            .next()
            .ok_or_else(|| anyhow!("host_command requires a non-empty argv"))?;
        let command_args: Vec<&String> = iter.collect();
        let mut command = Command::new(program);
        command.args(&command_args);
        for (key, value) in env {
            command.env(key, value);
        }
        let output = command
            .output()
            .with_context(|| format!("running `{program}`"))?;
        if !output.status.success() {
            let rendered_args = command_args
                .iter()
                .map(|arg| arg.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            return Err(anyhow!(
                "`{program} {}` exited with {}: {}",
                rendered_args,
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let stdout = String::from_utf8(output.stdout)
            .map_err(|error| anyhow!("non-utf8 stdout: {error}"))?;
        self.lock_commands()?.insert(key, stdout.clone());
        Ok(stdout)
    }

    fn lock_which(&self) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, Option<String>>>> {
        self.which
            .lock()
            .map_err(|_| anyhow!("host_which cache lock poisoned"))
    }

    fn lock_commands(&self) -> Result<std::sync::MutexGuard<'_, BTreeMap<CommandKey, String>>> {
        self.commands
            .lock()
            .map_err(|_| anyhow!("host_command cache lock poisoned"))
    }
}

thread_local! {
    static ACTIVE_STORE: RefCell<Option<AnalysisStore>> = const { RefCell::new(None) };
}

/// Install `store` as the active analysis target for the duration of
/// `f`, then return it back to the caller along with the closure's
/// result.
pub fn with_active_store<R>(store: AnalysisStore, f: impl FnOnce() -> R) -> (AnalysisStore, R) {
    ACTIVE_STORE.with(|cell| {
        *cell.borrow_mut() = Some(store);
    });
    let result = f();
    let store = ACTIVE_STORE.with(|cell| cell.borrow_mut().take().unwrap_or_default());
    (store, result)
}

fn with_store_mut<R>(f: impl FnOnce(Option<&mut AnalysisStore>) -> R) -> R {
    ACTIVE_STORE.with(|cell| f(cell.borrow_mut().as_mut()))
}

fn with_store<R>(f: impl FnOnce(Option<&AnalysisStore>) -> R) -> R {
    ACTIVE_STORE.with(|cell| f(cell.borrow().as_ref()))
}

fn analysis_active() -> bool {
    ACTIVE_STORE.with(|cell| cell.borrow().is_some())
}

/// Globals exposed to the prelude.
///
/// The set is intentionally generic: anything platform- or
/// toolchain-specific is implemented in starlark on top of these
/// primitives. Schema parsing references the names without invoking
/// them, so the bodies short-circuit to inert values when no
/// [`AnalysisStore`] is installed.
#[must_use]
pub fn globals_for_prelude() -> Globals {
    GlobalsBuilder::standard().with(prelude_globals).build()
}

#[starlark_module]
fn prelude_globals(builder: &mut GlobalsBuilder) {
    /// Host CPU architecture as a normalized string (e.g. `"arm64"`,
    /// `"x86_64"`). Schema parsing returns `""`.
    #[allow(clippy::unnecessary_wraps)]
    fn host_arch() -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        Ok(host_arch_str().to_string())
    }

    /// Host operating system as a normalized string (e.g. `"macos"`,
    /// `"linux"`). Schema parsing returns `""`.
    #[allow(clippy::unnecessary_wraps)]
    fn host_os() -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        Ok(host_os_str().to_string())
    }

    /// Find `name` on `PATH` and return its absolute path. Fails if
    /// the binary is not found. Schema parsing returns `""`.
    fn host_which(name: &str) -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        let resolved = with_store(|store| -> Result<Option<String>> {
            let store = store.ok_or_else(|| anyhow!("host_which called outside analysis"))?;
            store.host_cache.which(name)
        })?;
        resolved.ok_or_else(|| anyhow!("`{name}` not found on PATH"))
    }

    /// Run `argv[0]` with `argv[1..]` as arguments and return its
    /// stdout as a string. Fails if the process exits non-zero;
    /// includes stderr in the error message. Optional `env` is a
    /// `dict<string, string>` of environment variables overlaid on the
    /// host process env. Both `argv` and `env` participate in the
    /// cache key, so a different `DEVELOPER_DIR` resolves to a
    /// different cached result. Schema parsing returns `""`.
    fn host_command<'v>(
        argv: Value<'v>,
        env: Option<Value<'v>>,
    ) -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        let argv = unpack_string_list(argv, "argv")?;
        let env = env
            .map(|value| unpack_string_dict(value, "env"))
            .transpose()?
            .unwrap_or_default();
        with_store(|store| -> Result<String> {
            let store = store.ok_or_else(|| anyhow!("host_command called outside analysis"))?;
            store.host_cache.command(&argv, &env)
        })
    }

    /// Expand a list of glob patterns against the active target's
    /// package directory. Returns sorted, deduplicated, workspace-
    /// relative file paths. Schema parsing returns an empty list.
    fn glob<'v>(
        patterns: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let heap = eval.heap();
        if !analysis_active() {
            return Ok(heap.alloc(Vec::<String>::new()));
        }
        let patterns = unpack_string_list(patterns, "patterns")?;
        let resolved = with_store(|store| -> Result<Vec<String>> {
            let store = store.ok_or_else(|| anyhow!("glob called outside analysis"))?;
            expand_globs(&store.workspace_root, &store.package, &patterns)
        })?;
        Ok(heap.alloc(resolved))
    }

    /// Reserve a workspace-relative output path under the active
    /// target's build directory and return it. Outside analysis this
    /// returns the bare name.
    fn declare_output(name: &str) -> anyhow::Result<String> {
        with_store_mut(|store| match store {
            Some(store) => {
                let path = format!("{}/{}", store.build_dir, name);
                store.declared_outputs.push(path.clone());
                Ok(path)
            }
            None => Ok(name.to_string()),
        })
    }

    /// Record one command action declaration. Argument shape:
    /// `argv`: list of strings; `inputs`: list of workspace-relative
    /// source paths to hash into the input digest; `outputs`: list of
    /// workspace-relative paths the action produces; `env`: optional
    /// string->string dict; `toolchain_identity`: optional string
    /// folded into the input digest; `identifier`: optional label for
    /// diagnostics.
    fn run_action<'v>(
        argv: Value<'v>,
        inputs: Option<Value<'v>>,
        outputs: Option<Value<'v>>,
        env: Option<Value<'v>>,
        toolchain_identity: Option<String>,
        identifier: Option<String>,
    ) -> anyhow::Result<NoneType> {
        let argv = unpack_string_list(argv, "argv")?;
        let inputs = inputs
            .map(|value| unpack_string_list(value, "inputs"))
            .transpose()?
            .unwrap_or_default();
        let outputs = outputs
            .map(|value| unpack_string_list(value, "outputs"))
            .transpose()?
            .unwrap_or_default();
        let env = env
            .map(|value| unpack_string_dict(value, "env"))
            .transpose()?
            .unwrap_or_default();
        let action = DeclaredAction {
            argv,
            inputs,
            outputs,
            env,
            toolchain_identity,
            identifier,
        };
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(action);
            }
        });
        Ok(NoneType)
    }
}

fn host_arch_str() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        std::env::consts::ARCH
    }
}

fn host_os_str() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        std::env::consts::OS
    }
}

fn which_on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for entry in std::env::split_paths(&paths) {
        let candidate = entry.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Expand `patterns` against `package` and return workspace-relative
/// file paths.
///
/// Each match is canonicalized and required to land inside the
/// canonical workspace root, which rejects symlinks that point
/// outside the tree. The check is best-effort against the on-disk
/// state at evaluation time: a write-capable attacker on the
/// workspace could in principle swap a symlink between
/// `glob::glob` and `canonicalize`. Once treats the workspace as
/// trusted (a developer's own checkout), so this TOCTOU window is
/// out of scope for the threat model; the check exists to surface
/// honest mistakes (a stray `..` symlink), not adversarial races.
/// Windows junctions are not exercised by tests yet; the
/// `canonicalize` call covers them in production but a dedicated
/// Windows test should land alongside Windows CI.
fn expand_globs(workspace_root: &Path, package: &str, patterns: &[String]) -> Result<Vec<String>> {
    let package_dir = if package.is_empty() {
        workspace_root.to_path_buf()
    } else {
        workspace_root.join(package)
    };
    let canonical_workspace = std::fs::canonicalize(workspace_root)
        .with_context(|| format!("canonicalizing workspace `{}`", workspace_root.display()))?;
    let mut out: Vec<String> = Vec::new();
    for pattern in patterns {
        let abs_pattern = package_dir.join(pattern);
        let pattern_str = abs_pattern
            .to_str()
            .ok_or_else(|| anyhow!("non-utf8 glob pattern `{pattern}`"))?;
        for entry in
            glob::glob(pattern_str).with_context(|| format!("invalid glob pattern `{pattern}`"))?
        {
            let path = entry.with_context(|| format!("glob walk failed for `{pattern}`"))?;
            if !path.is_file() {
                continue;
            }
            let canonical = std::fs::canonicalize(&path)
                .with_context(|| format!("canonicalizing `{}`", path.display()))?;
            let stripped = canonical
                .strip_prefix(&canonical_workspace)
                .with_context(|| {
                    format!(
                        "glob result `{}` is outside the workspace `{}`",
                        canonical.display(),
                        canonical_workspace.display()
                    )
                })?;
            let ws_rel = stripped
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if !ws_rel.is_empty() {
                out.push(ws_rel);
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
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
/// target. It caches cheap rule metadata and generic host lookups
/// (`host_which`, `host_command`) while still evaluating each target's
/// Starlark impl in an isolated module heap.
#[derive(Debug, Clone)]
pub struct AnalysisEngine {
    source: &'static str,
    rule_impls: RuleImpls,
    host_cache: HostCache,
}

impl AnalysisEngine {
    pub fn new() -> Result<Self> {
        let source = include_str!("../prelude/apple.star");
        Self::from_source(source)
    }

    pub fn from_source(source: &'static str) -> Result<Self> {
        Ok(Self {
            source,
            rule_impls: parse_rule_impls(source)?,
            host_cache: HostCache::default(),
        })
    }

    #[must_use]
    pub fn rule_has_impl(&self, kind: &str) -> bool {
        self.rule_impls.has_impl(kind)
    }

    /// Run a single target's rule impl and collect its declared
    /// actions and provider record.
    ///
    /// `dep_providers` supplies the provider record each in-graph
    /// dependency already returned; impls iterate it to gather things
    /// like swiftmodule search paths.
    pub fn analyze_target(
        &self,
        target: &GraphTarget,
        workspace_root: &Path,
        dep_providers: &[JsonValue],
    ) -> Result<AnalysisResult> {
        analyze_target_with_host_cache(
            self.source,
            self.host_cache.clone(),
            target,
            workspace_root,
            dep_providers,
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
/// dependency already returned; impls iterate it to gather things
/// like swiftmodule search paths.
pub fn analyze_target(
    target: &GraphTarget,
    workspace_root: &Path,
    dep_providers: &[JsonValue],
) -> Result<AnalysisResult> {
    AnalysisEngine::new()?.analyze_target(target, workspace_root, dep_providers)
}

fn analyze_target_with_host_cache(
    source: &'static str,
    host_cache: HostCache,
    target: &GraphTarget,
    workspace_root: &Path,
    dep_providers: &[JsonValue],
) -> Result<AnalysisResult> {
    let build_dir = format!(".once/out/{}", target.label.id);
    let store = AnalysisStore::with_host_cache(
        workspace_root.to_path_buf(),
        target.label.package.clone(),
        build_dir.clone(),
        host_cache,
    );

    let (store, result) = with_active_store(store, || {
        analyze_in_starlark(source, target, dep_providers, &build_dir)
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
/// placeholder rules don't trigger analysis of their library deps.
pub fn rule_has_impl(kind: &str) -> Result<bool> {
    Ok(AnalysisEngine::new()?.rule_has_impl(kind))
}

fn parse_rule_impls(source: &'static str) -> Result<RuleImpls> {
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse(
            "once//prelude/apple.star",
            source.to_string(),
            &Dialect::Standard,
        )
        .map_err(|error| anyhow!("prelude parse failed: {error:?}"))?;
        let globals = globals_for_prelude();
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)
            .map_err(|error| anyhow!("prelude eval failed: {error:?}"))?;
        let rules_value = module
            .get("APPLE_RULES")
            .context("prelude is missing APPLE_RULES export")?;
        let rules = ListRef::from_value(rules_value).context("APPLE_RULES is not a list")?;
        let mut by_kind = BTreeMap::new();
        for rule in rules.iter() {
            let dict = DictRef::from_value(rule)
                .ok_or_else(|| anyhow!("APPLE_RULES entry is not a dict"))?;
            let Some(rule_kind) = dict.get_str("kind").and_then(Value::unpack_str) else {
                continue;
            };
            let impl_value = dict.get_str("impl");
            by_kind.insert(
                rule_kind.to_string(),
                impl_value.is_some_and(|value| !value.is_none()),
            );
        }
        Ok(RuleImpls { by_kind })
    })
}

fn analyze_in_starlark(
    source: &str,
    target: &GraphTarget,
    dep_providers: &[JsonValue],
    build_dir: &str,
) -> Result<JsonValue> {
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse(
            "once//prelude/apple.star",
            source.to_string(),
            &Dialect::Standard,
        )
        .map_err(|error| anyhow!("prelude parse failed: {error:?}"))?;
        let globals = globals_for_prelude();
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)
            .map_err(|error| anyhow!("prelude eval failed: {error:?}"))?;
        let rules_value = module
            .get("APPLE_RULES")
            .context("prelude is missing APPLE_RULES export")?;
        let rules = ListRef::from_value(rules_value).context("APPLE_RULES is not a list")?;
        let impl_value = find_impl_for_kind(rules, &target.kind)?;
        let Some(impl_value) = impl_value else {
            return Ok(JsonValue::Null);
        };
        let ctx = build_ctx(&eval, target, dep_providers, build_dir);
        let provider = eval
            .eval_function(impl_value, &[ctx], &[])
            .map_err(|error| anyhow!("impl eval failed for {}: {error:?}", target.label.id))?;
        Ok(value_to_json(provider))
    })
}

fn find_impl_for_kind<'v>(rules: &ListRef<'v>, kind: &str) -> Result<Option<Value<'v>>> {
    for rule in rules.iter() {
        let dict =
            DictRef::from_value(rule).ok_or_else(|| anyhow!("APPLE_RULES entry is not a dict"))?;
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
    ]))
}

fn attr_value_to_starlark<'v>(eval: &Evaluator<'v, '_, '_>, value: &AttrValue) -> Value<'v> {
    let heap = eval.heap();
    match value {
        AttrValue::String(string) => heap.alloc(string.clone()),
        AttrValue::Integer(integer) => heap.alloc(*integer),
        AttrValue::Float(float) => heap.alloc(*float),
        AttrValue::Bool(boolean) => Value::new_bool(*boolean),
        AttrValue::List(items) => {
            let values: Vec<Value<'v>> = items
                .iter()
                .map(|item| attr_value_to_starlark(eval, item))
                .collect();
            heap.alloc(values)
        }
        AttrValue::Map(entries) => {
            let pairs: Vec<(String, Value<'v>)> = entries
                .iter()
                .map(|(key, value)| (key.clone(), attr_value_to_starlark(eval, value)))
                .collect();
            heap.alloc(AllocDict(pairs))
        }
    }
}

fn json_to_value<'v>(eval: &Evaluator<'v, '_, '_>, json: &JsonValue) -> Value<'v> {
    let heap = eval.heap();
    match json {
        JsonValue::Null => Value::new_none(),
        JsonValue::Bool(boolean) => Value::new_bool(*boolean),
        JsonValue::Number(number) => {
            if let Some(integer) = number.as_i64() {
                heap.alloc(integer)
            } else if let Some(float) = number.as_f64() {
                heap.alloc(float)
            } else {
                Value::new_none()
            }
        }
        JsonValue::String(string) => heap.alloc(string.clone()),
        JsonValue::Array(items) => {
            let values: Vec<Value<'v>> =
                items.iter().map(|item| json_to_value(eval, item)).collect();
            heap.alloc(values)
        }
        JsonValue::Object(entries) => {
            let pairs: Vec<(String, Value<'v>)> = entries
                .iter()
                .map(|(key, value)| (key.clone(), json_to_value(eval, value)))
                .collect();
            heap.alloc(AllocDict(pairs))
        }
    }
}

fn value_to_json(value: Value<'_>) -> JsonValue {
    if value.is_none() {
        return JsonValue::Null;
    }
    if let Some(boolean) = value.unpack_bool() {
        return JsonValue::Bool(boolean);
    }
    if let Some(integer) = value.unpack_i32() {
        return JsonValue::Number(serde_json::Number::from(integer));
    }
    if let Some(string) = value.unpack_str() {
        return JsonValue::String(string.to_string());
    }
    if let Some(list) = ListRef::from_value(value) {
        return JsonValue::Array(list.iter().map(value_to_json).collect());
    }
    if let Some(dict) = DictRef::from_value(value) {
        let mut map = serde_json::Map::new();
        for (key, child) in dict.iter() {
            let Some(key_str) = key.unpack_str() else {
                continue;
            };
            map.insert(key_str.to_string(), value_to_json(child));
        }
        return JsonValue::Object(map);
    }
    JsonValue::String(value.to_string())
}

fn unpack_string_list(value: Value<'_>, field: &str) -> anyhow::Result<Vec<String>> {
    let list = ListRef::from_value(value).ok_or_else(|| {
        anyhow!(
            "expected `{field}` to be a list of strings, got `{}`",
            value.get_type()
        )
    })?;
    list.iter()
        .map(|item| {
            item.unpack_str().map(ToOwned::to_owned).ok_or_else(|| {
                anyhow!(
                    "expected `{field}` entries to be strings, got `{}`",
                    item.get_type()
                )
            })
        })
        .collect()
}

fn unpack_string_dict(value: Value<'_>, field: &str) -> anyhow::Result<BTreeMap<String, String>> {
    let dict = DictRef::from_value(value).ok_or_else(|| {
        anyhow!(
            "expected `{field}` to be a dict<string, string>, got `{}`",
            value.get_type()
        )
    })?;
    let mut out = BTreeMap::new();
    for (key, value) in dict.iter() {
        let key = key
            .unpack_str()
            .ok_or_else(|| {
                anyhow!(
                    "expected `{field}` keys to be strings, got `{}`",
                    key.get_type()
                )
            })?
            .to_owned();
        let value = value
            .unpack_str()
            .ok_or_else(|| {
                anyhow!(
                    "expected `{field}` values to be strings, got `{}`",
                    value.get_type()
                )
            })?
            .to_owned();
        out.insert(key, value);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use starlark::environment::Module;
    use starlark::syntax::{AstModule, Dialect};
    use tempfile::TempDir;

    fn run(source: &str) -> starlark::Result<()> {
        Module::with_temp_heap(|module| {
            let ast = AstModule::parse("test.star", source.to_string(), &Dialect::Standard)?;
            let globals = globals_for_prelude();
            let mut evaluator = Evaluator::new(&module);
            evaluator.eval_module(ast, &globals)?;
            starlark::Result::Ok(())
        })
    }

    fn store_for(workspace: &Path, package: &str) -> AnalysisStore {
        AnalysisStore::new(
            workspace.to_path_buf(),
            package.to_string(),
            format!(".once/out/{package}"),
        )
    }

    #[test]
    fn schema_parse_path_resolves_native_globals_without_calling_them() {
        run("def _impl():\n    return run_action\n").unwrap();
    }

    #[test]
    fn declare_output_outside_analysis_returns_bare_name() {
        run(r#"x = declare_output("AppCore.a")"#).unwrap();
    }

    #[test]
    fn run_action_records_declarations_when_analysis_is_active() {
        let tmp = TempDir::new().unwrap();
        let store = store_for(tmp.path(), "apps/ios/AppCore");
        let (store, ()) = with_active_store(store, || {
            run(r#"
run_action(
    argv = ["swiftc", "-o", "AppCore.a"],
    inputs = ["apps/ios/AppCore/Sources/main.swift"],
    outputs = ["AppCore.a"],
    toolchain_identity = "id-1",
    identifier = "swift_compile",
)
"#)
            .unwrap();
        });
        assert_eq!(store.actions.len(), 1);
        assert_eq!(store.actions[0].argv[0], "swiftc");
        assert_eq!(store.actions[0].outputs, vec!["AppCore.a".to_string()]);
        assert_eq!(
            store.actions[0].identifier.as_deref(),
            Some("swift_compile")
        );
    }

    #[test]
    fn declare_output_attaches_active_build_dir() {
        let tmp = TempDir::new().unwrap();
        let store = store_for(tmp.path(), "apps/ios/AppCore");
        let (store, ()) = with_active_store(store, || {
            run(r#"x = declare_output("AppCore.a")"#).unwrap();
        });
        assert_eq!(
            store.declared_outputs,
            vec![".once/out/apps/ios/AppCore/AppCore.a".to_string()]
        );
    }

    #[test]
    fn run_action_rejects_non_string_argv_entries() {
        let tmp = TempDir::new().unwrap();
        let store = store_for(tmp.path(), "p");
        let (_, err) = with_active_store(store, || {
            run(r#"run_action(argv = [1, "swiftc"])"#).unwrap_err()
        });
        let message = format!("{err:?}");
        assert!(message.contains("entries to be strings"), "{message}");
    }

    #[cfg(unix)]
    #[test]
    fn host_command_cache_reuses_identical_argv_results() {
        let tmp = TempDir::new().unwrap();
        let counter = tmp.path().join("counter");
        let cache = HostCache::default();
        let argv = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf x >> \"$1\"; printf done".to_string(),
            "sh".to_string(),
            counter.display().to_string(),
        ];

        let env = BTreeMap::new();
        assert_eq!(cache.command(&argv, &env).unwrap(), "done");
        assert_eq!(cache.command(&argv, &env).unwrap(), "done");

        assert_eq!(std::fs::read_to_string(counter).unwrap(), "x");
    }

    #[test]
    fn glob_expands_against_active_package_directory() {
        let tmp = TempDir::new().unwrap();
        let pkg = tmp.path().join("apps/ios/AppCore/Sources");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("a.swift"), "").unwrap();
        std::fs::write(pkg.join("b.swift"), "").unwrap();
        std::fs::write(pkg.join("c.txt"), "").unwrap();

        let store = store_for(tmp.path(), "apps/ios/AppCore");
        let (store, ()) = with_active_store(store, || {
            run(r#"
matches = glob(["Sources/*.swift"])
run_action(argv = ["echo"] + matches, outputs = ["out"])
"#)
            .unwrap();
        });
        assert_eq!(store.actions.len(), 1);
        let argv = &store.actions[0].argv;
        assert_eq!(argv[0], "echo");
        assert!(argv[1..].iter().any(|p| p.ends_with("Sources/a.swift")));
        assert!(argv[1..].iter().any(|p| p.ends_with("Sources/b.swift")));
        assert!(!argv[1..].iter().any(|p| p.ends_with("Sources/c.txt")));
    }

    /// A symlink that resolves outside the workspace must surface as
    /// an error rather than silently leaking external paths into the
    /// returned list. The check rejects honest mistakes; the threat
    /// model assumes a non-adversarial workspace (documented on
    /// `expand_globs`). Windows junctions/symlinks behave similarly
    /// via the same canonicalize call, but get their own test once
    /// Windows CI exists.
    #[cfg(unix)]
    #[test]
    fn glob_rejects_symlink_that_escapes_workspace() {
        let workspace = TempDir::new().unwrap();
        let external = TempDir::new().unwrap();
        let pkg = workspace.path().join("apps/ios/AppCore");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(external.path().join("stolen.swift"), "").unwrap();
        std::os::unix::fs::symlink(
            external.path().join("stolen.swift"),
            pkg.join("escape.swift"),
        )
        .unwrap();

        let err = expand_globs(
            workspace.path(),
            "apps/ios/AppCore",
            &["escape.swift".to_string()],
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("outside the workspace"), "{err}");
    }

    fn target(kind: &str) -> GraphTarget {
        use crate::graph::{Capability, TargetLabel};
        GraphTarget {
            label: TargetLabel {
                package: "apps/ios".to_string(),
                name: "Sample".to_string(),
                id: "apps/ios/Sample".to_string(),
            },
            kind: kind.to_string(),
            deps: Vec::new(),
            srcs: Vec::new(),
            attrs: BTreeMap::new(),
            capabilities: vec![Capability {
                name: "build".to_string(),
                output_groups: Vec::new(),
                requires_outputs: Vec::new(),
            }],
            providers: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn analyze_target_returns_null_provider_for_rules_without_impl() {
        let tmp = TempDir::new().unwrap();
        let result = analyze_target(&target("apple_framework"), tmp.path(), &[]).unwrap();
        assert!(result.actions.is_empty());
        assert_eq!(result.provider, JsonValue::Null);
        assert!(result.declared_outputs.is_empty());
    }

    #[test]
    fn analyze_target_errors_on_unknown_rule_kind() {
        let tmp = TempDir::new().unwrap();
        let err = analyze_target(&target("mystery_rule"), tmp.path(), &[])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("no rule found for kind `mystery_rule`"),
            "{err}"
        );
    }

    #[test]
    fn rule_has_impl_returns_true_for_apple_library() {
        assert!(rule_has_impl("apple_library").unwrap());
    }

    #[test]
    fn rule_has_impl_returns_false_for_placeholder_rules() {
        assert!(!rule_has_impl("apple_framework").unwrap());
        assert!(!rule_has_impl("apple_application").unwrap());
        assert!(!rule_has_impl("apple_test_bundle").unwrap());
    }

    #[test]
    fn rule_has_impl_returns_false_for_unknown_kind() {
        assert!(!rule_has_impl("mystery_rule").unwrap());
    }
}
