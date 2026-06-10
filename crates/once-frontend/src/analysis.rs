//! Starlark rule implementation analysis.
//!
//! Rule schemas declared in the prelude carry an optional `impl`
//! callable. The analysis pass evaluates that callable for one target
//! at a time with a `ctx` dict and collects the actions the impl
//! declares through globals defined here (`run_action`,
//! `declare_output`) plus helpers backed by the host toolchain
//! (`xcrun_swiftc`, `apple_triple`).
//!
//! For schema parsing the same globals are registered as defined names
//! so prelude functions that reference them compile. When called
//! without an active [`AnalysisStore`] in the thread-local the stubs
//! return inert defaults instead of failing, since the schema-parse
//! evaluator never invokes rule impls.
//!
//! State threading uses a thread-local instead of `Evaluator::extra`
//! because `extra` requires implementing the `unsafe`
//! `ProvidesStaticType` trait, and the workspace forbids `unsafe`
//! code. [`with_active_store`] installs a store for the duration of an
//! evaluation and tears it back out, so concurrent evaluations on
//! different threads never trip over each other.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;

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

/// Per-target collection of declared outputs and actions, populated by
/// starlark native callbacks during a rule impl evaluation.
#[derive(Debug, Default)]
pub struct AnalysisStore {
    pub build_dir: String,
    pub declared_outputs: Vec<String>,
    pub actions: Vec<DeclaredAction>,
}

impl AnalysisStore {
    #[must_use]
    pub fn new(build_dir: String) -> Self {
        Self {
            build_dir,
            declared_outputs: Vec::new(),
            actions: Vec::new(),
        }
    }
}

thread_local! {
    static ACTIVE_STORE: RefCell<Option<AnalysisStore>> = const { RefCell::new(None) };
}

/// Install `store` as the active analysis target for the duration of
/// `f`, then return it back to the caller along with the closure's
/// result. Nested calls overwrite the inner closure's view; the
/// callers serialize analyses per thread, so nesting is not expected.
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

fn analysis_active() -> bool {
    ACTIVE_STORE.with(|cell| cell.borrow().is_some())
}

/// Globals used by both schema parsing and target analysis.
///
/// Schema parsing only needs the names resolvable; the bodies are not
/// invoked. Analysis re-uses the same globals plus an [`AnalysisStore`]
/// installed via [`with_active_store`].
#[must_use]
pub fn globals_for_prelude() -> Globals {
    GlobalsBuilder::standard().with(prelude_globals).build()
}

#[starlark_module]
fn prelude_globals(builder: &mut GlobalsBuilder) {
    /// Resolve the Swift toolchain for `platform`. Returns a 3-tuple
    /// `(xcrun_path, sdk_name, identity)` where `identity` is a stable
    /// string used to partition the action cache by Xcode version.
    /// Outside analysis the three values are empty strings, so schema
    /// parsing never spawns `xcrun`.
    fn xcrun_swiftc<'v>(
        platform: &str,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Value<'v>> {
        let heap = eval.heap();
        if !analysis_active() {
            return Ok(heap.alloc((String::new(), String::new(), String::new())));
        }
        let resolution = crate::analysis_host::resolve_swift_toolchain(platform)?;
        Ok(heap.alloc((resolution.xcrun, resolution.sdk, resolution.identity)))
    }

    /// Render a target triple for `platform`/`minimum_os` using the
    /// host architecture.
    #[allow(clippy::unnecessary_wraps)]
    fn apple_triple(platform: &str, minimum_os: &str) -> anyhow::Result<String> {
        // Result wrapping keeps the starlark_module signature uniform
        // with the other globals; triple_parts is infallible because
        // unknown platforms fall back to a literal.
        Ok(crate::analysis_host::apple_triple(platform, minimum_os))
    }

    /// Reserve a workspace-relative output path under the active
    /// target's build directory and return it. Outside analysis this
    /// returns the bare name, which is harmless because schema parsing
    /// never inspects the value.
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

    /// Record one `run_action` declaration. Argument shape:
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

/// Result of analyzing one target.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AnalysisResult {
    /// Declared command actions in the order the impl emitted them.
    pub actions: Vec<DeclaredAction>,
    /// Provider record returned by the impl (the impl's return value).
    /// JSON shape so downstream rules can read field values without
    /// re-evaluating starlark across analysis boundaries.
    pub provider: JsonValue,
    /// Workspace-relative outputs declared during this analysis.
    pub declared_outputs: Vec<String>,
}

/// Run a single target's rule impl and collect its declared actions
/// and provider record.
///
/// `srcs` is the resolved-against-the-workspace list of source paths
/// (glob expansion is the caller's responsibility). `dep_providers`
/// supplies the provider record each in-graph dependency already
/// returned, in dependency declaration order; impls iterate it to
/// gather things like swiftmodule search paths.
pub fn analyze_target(
    target: &GraphTarget,
    srcs: &[String],
    dep_providers: &[JsonValue],
    _workspace: &Path,
) -> Result<AnalysisResult> {
    let source = include_str!("../prelude/apple.star");
    let build_dir = format!(".once/out/{}", target.label.id);
    let store = AnalysisStore::new(build_dir.clone());

    let (store, result) = with_active_store(store, || {
        analyze_in_starlark(source, target, srcs, dep_providers, &build_dir)
    });
    let provider = result?;
    Ok(AnalysisResult {
        actions: store.actions,
        provider,
        declared_outputs: store.declared_outputs,
    })
}

fn analyze_in_starlark(
    source: &str,
    target: &GraphTarget,
    srcs: &[String],
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
            // Rule with no impl falls back to placeholder behavior;
            // the caller decides what to do (e.g. legacy shell scripts).
            return Ok(JsonValue::Null);
        };
        let ctx = build_ctx(&eval, target, srcs, dep_providers, build_dir);
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
    srcs: &[String],
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
    let srcs_value = heap.alloc(srcs.to_vec());
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
    // Fallback: render anything else as its string form so a misshaped
    // provider record surfaces visibly instead of silently turning into
    // null.
    JsonValue::String(value.to_string())
}

fn unpack_string_list(value: Value<'_>, field: &str) -> anyhow::Result<Vec<String>> {
    let list = ListRef::from_value(value).ok_or_else(|| {
        anyhow::anyhow!(
            "expected `{field}` to be a list of strings, got `{}`",
            value.get_type()
        )
    })?;
    list.iter()
        .map(|item| {
            item.unpack_str().map(ToOwned::to_owned).ok_or_else(|| {
                anyhow::anyhow!(
                    "expected `{field}` entries to be strings, got `{}`",
                    item.get_type()
                )
            })
        })
        .collect()
}

fn unpack_string_dict(value: Value<'_>, field: &str) -> anyhow::Result<BTreeMap<String, String>> {
    let dict = DictRef::from_value(value).ok_or_else(|| {
        anyhow::anyhow!(
            "expected `{field}` to be a dict<string, string>, got `{}`",
            value.get_type()
        )
    })?;
    let mut out = BTreeMap::new();
    for (key, value) in dict.iter() {
        let key = key
            .unpack_str()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "expected `{field}` keys to be strings, got `{}`",
                    key.get_type()
                )
            })?
            .to_owned();
        let value = value
            .unpack_str()
            .ok_or_else(|| {
                anyhow::anyhow!(
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

    fn run(source: &str) -> starlark::Result<()> {
        Module::with_temp_heap(|module| {
            let ast = AstModule::parse("test.star", source.to_string(), &Dialect::Standard)?;
            let globals = globals_for_prelude();
            let mut evaluator = Evaluator::new(&module);
            evaluator.eval_module(ast, &globals)?;
            starlark::Result::Ok(())
        })
    }

    #[test]
    fn schema_parse_path_resolves_native_globals_without_calling_them() {
        // The functions are referenced but never invoked, matching
        // what prelude schema parsing does with rule impls.
        run("def _impl():\n    return xcrun_swiftc\n").unwrap();
    }

    #[test]
    fn declare_output_outside_analysis_returns_bare_name() {
        run(r#"x = declare_output("AppCore.a")"#).unwrap();
    }

    #[test]
    fn run_action_records_declarations_when_analysis_is_active() {
        let store = AnalysisStore::new(".once/out/apps/ios/AppCore".to_string());
        let (store, ()) = with_active_store(store, || {
            run(r#"
run_action(
    argv = ["xcrun", "swiftc", "-o", "AppCore.a"],
    inputs = ["apps/ios/AppCore/Sources/main.swift"],
    outputs = ["AppCore.a"],
    toolchain_identity = "id-1",
    identifier = "swift_compile",
)
"#)
            .unwrap();
        });
        assert_eq!(store.actions.len(), 1);
        assert_eq!(store.actions[0].argv[0], "xcrun");
        assert_eq!(store.actions[0].outputs, vec!["AppCore.a".to_string()]);
        assert_eq!(
            store.actions[0].identifier.as_deref(),
            Some("swift_compile")
        );
    }

    #[test]
    fn declare_output_attaches_active_build_dir() {
        let store = AnalysisStore::new(".once/out/apps/ios/AppCore".to_string());
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
        let store = AnalysisStore::new("d".to_string());
        let (_, err) = with_active_store(store, || {
            run(r#"run_action(argv = [1, "swiftc"])"#).unwrap_err()
        });
        let message = format!("{err:?}");
        assert!(message.contains("entries to be strings"), "{message}");
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
        // apple_framework intentionally has no impl yet; analyzing it
        // should be a no-op rather than an error so the caller can
        // fall back to its placeholder path.
        let result = analyze_target(
            &target("apple_framework"),
            &[],
            &[],
            std::path::Path::new("/tmp"),
        )
        .unwrap();
        assert!(result.actions.is_empty());
        assert_eq!(result.provider, JsonValue::Null);
        assert!(result.declared_outputs.is_empty());
    }

    #[test]
    fn analyze_target_errors_on_unknown_rule_kind() {
        let err = analyze_target(
            &target("mystery_rule"),
            &[],
            &[],
            std::path::Path::new("/tmp"),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("no rule found for kind `mystery_rule`"),
            "{err}"
        );
    }
}
