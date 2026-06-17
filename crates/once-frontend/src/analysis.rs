//! Starlark rule implementation analysis.
//!
//! Rule schemas declared in the prelude carry an optional `impl`
//! callable. The analysis pass evaluates that callable for one target
//! at a time with a `ctx` dict and collects the actions the impl
//! declares through globals defined here.
//!
//! The globals are deliberately generic: `host_arch`, `host_which`,
//! `host_command`, `glob`, `declare_output`, `run_action`,
//! `write_file`, `write_bytes`. Anything domain-specific (toolchain
//! discovery, SDK names, triple rendering, binary file formats, file
//! extension filtering for a particular language) lives in the
//! Starlark prelude, not in Rust. The Rust side is an executor for
//! whatever the prelude declares; it has no knowledge of the build
//! systems composed on top of these primitives.
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
use serde::{Deserialize, Serialize};
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
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DeclaredAction {
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default = "default_cacheable", skip_serializing_if = "is_true")]
    pub cacheable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
}

fn default_cacheable() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_true(value: &bool) -> bool {
    *value
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
    /// running on sibling targets aren't blocked by a slow external
    /// process spawn (toolchain discovery, version probes, etc).
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

    /// Active workspace root as an absolute path. Schema parsing
    /// returns `""`.
    fn workspace_root() -> anyhow::Result<String> {
        if !analysis_active() {
            return Ok(String::new());
        }
        with_store(|store| -> Result<String> {
            let store = store.ok_or_else(|| anyhow!("workspace_root called outside analysis"))?;
            Ok(store.workspace_root.to_string_lossy().into_owned())
        })
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
    fn host_command<'v>(argv: Value<'v>, env: Option<Value<'v>>) -> anyhow::Result<String> {
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

    /// Declare an action that materialises `content` at the workspace-
    /// relative `path`. The content is hashed into the input digest so
    /// any edit (including in starlark that produced it) invalidates
    /// downstream consumers.
    ///
    /// Implementation note: the materialisation runs as `/bin/sh -c`
    /// with the path bound to a shell variable first; the parent
    /// directory is computed via the POSIX `${var%/*}` parameter
    /// expansion. Passing `shell_quote(path)` directly inside
    /// `$(dirname ...)` would re-tokenize the escaped quotes a path
    /// like `a'b/c.h` ends up with, so binding once and dereferencing
    /// twice keeps the action robust against single quotes in paths.
    #[allow(clippy::unnecessary_wraps)]
    fn write_file(path: &str, content: &str) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let script = format!(
            "set -eu\n\
             __once_path={path_arg}\n\
             case \"$__once_path\" in */*) mkdir -p \"${{__once_path%/*}}\" ;; esac\n\
             printf '%s' {content_arg} > \"$__once_path\"\n",
            path_arg = shell_quote(path),
            content_arg = shell_quote(content),
        );
        let action = DeclaredAction {
            argv: vec!["/bin/sh".to_string(), "-c".to_string(), script],
            inputs: Vec::new(),
            outputs: vec![path.to_string()],
            env: BTreeMap::new(),
            cacheable: true,
            // Folding the literal content into the toolchain identity
            // keeps the digest pinned to what the file should contain,
            // so changing the content alone invalidates the action.
            toolchain_identity: Some(format!("once.write_file.v1\0{content}")),
            identifier: Some(format!("write_file:{path}")),
        };
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(action);
            }
        });
        Ok(NoneType)
    }

    /// Declare an action that materialises raw bytes at `path`.
    /// `bytes` is a list of integers in `0..=255`. The content is
    /// base64-encoded into the generated shell command so binary
    /// payloads (including NULs) survive shell quoting, and is folded
    /// into the toolchain identity so any change invalidates
    /// downstream consumers. Domain-specific binary formats
    /// (header-maps, mach-o, etc.) are constructed in the prelude
    /// and emitted through this primitive.
    #[allow(clippy::unnecessary_wraps)]
    fn write_bytes<'v>(path: &str, bytes: Value<'v>) -> anyhow::Result<NoneType> {
        if !analysis_active() {
            return Ok(NoneType);
        }
        let bytes = unpack_byte_list(bytes, "bytes")?;
        let encoded = base64_encode(&bytes);
        let script = format!(
            "set -eu\n\
             __once_path={path_arg}\n\
             case \"$__once_path\" in */*) mkdir -p \"${{__once_path%/*}}\" ;; esac\n\
             printf '%s' {encoded_arg} | base64 -d > \"$__once_path\"\n",
            path_arg = shell_quote(path),
            encoded_arg = shell_quote(&encoded),
        );
        let action = DeclaredAction {
            argv: vec!["/bin/sh".to_string(), "-c".to_string(), script],
            inputs: Vec::new(),
            outputs: vec![path.to_string()],
            env: BTreeMap::new(),
            cacheable: true,
            toolchain_identity: Some(format!("once.write_bytes.v1\0{encoded}")),
            identifier: Some(format!("write_bytes:{path}")),
        };
        with_store_mut(|store| {
            if let Some(store) = store {
                store.actions.push(action);
            }
        });
        Ok(NoneType)
    }

    /// Record one command action declaration. Argument shape:
    /// `argv`: list of strings; `inputs`: list of workspace-relative
    /// source paths to hash into the input digest; `outputs`: list of
    /// workspace-relative paths the action produces; `env`: optional
    /// string->string dict; `cacheable`: optional bool, default true;
    /// `toolchain_identity`: optional string folded into the input
    /// digest; `identifier`: optional label for diagnostics.
    fn run_action<'v>(
        argv: Value<'v>,
        inputs: Option<Value<'v>>,
        outputs: Option<Value<'v>>,
        env: Option<Value<'v>>,
        toolchain_identity: Option<String>,
        identifier: Option<String>,
        cacheable: Option<bool>,
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
            cacheable: cacheable.unwrap_or(true),
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

    /// Decode TOML into Starlark dictionaries/lists/scalars. This is a
    /// generic data-format primitive used by dependency resolvers; the
    /// ecosystem-specific interpretation stays in Starlark.
    fn toml_decode<'v>(src: &str, eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        let value: toml::Value = toml::from_str(src)?;
        Ok(toml_value_to_starlark(eval, value))
    }

    /// Decode JSON into Starlark dictionaries/lists/scalars. Dependency
    /// resolvers use this for machine output from ecosystem-native
    /// resolution commands.
    fn json_decode<'v>(src: &str, eval: &mut Evaluator<'v, '_, '_>) -> anyhow::Result<Value<'v>> {
        let value: JsonValue = serde_json::from_str(src)?;
        Ok(json_to_value(eval, &value))
    }
}

fn toml_value_to_starlark<'v>(eval: &Evaluator<'v, '_, '_>, value: toml::Value) -> Value<'v> {
    let heap = eval.heap();
    match value {
        toml::Value::String(value) => heap.alloc(value),
        toml::Value::Integer(value) => heap.alloc(value),
        toml::Value::Float(value) => heap.alloc(value),
        toml::Value::Boolean(value) => Value::new_bool(value),
        toml::Value::Array(values) => heap.alloc(
            values
                .into_iter()
                .map(|value| toml_value_to_starlark(eval, value))
                .collect::<Vec<_>>(),
        ),
        toml::Value::Table(values) => heap.alloc(AllocDict(
            values
                .into_iter()
                .map(|(key, value)| (key, toml_value_to_starlark(eval, value))),
        )),
        toml::Value::Datetime(value) => heap.alloc(value.to_string()),
    }
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

/// Standard base64 alphabet encoder. The output is consumed by
/// `base64 -d` in a generated shell script, so we need round-tripping
/// fidelity, not a fancy MIME-line-wrapped form.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut chunks = bytes.chunks_exact(3);
    for chunk in &mut chunks {
        let bits = (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]);
        out.push(ALPHABET[((bits >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((bits >> 12) & 0x3F) as usize] as char);
        out.push(ALPHABET[((bits >> 6) & 0x3F) as usize] as char);
        out.push(ALPHABET[(bits & 0x3F) as usize] as char);
    }
    let remainder = chunks.remainder();
    match remainder.len() {
        0 => {}
        1 => {
            let bits = u32::from(remainder[0]) << 16;
            out.push(ALPHABET[((bits >> 18) & 0x3F) as usize] as char);
            out.push(ALPHABET[((bits >> 12) & 0x3F) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let bits = (u32::from(remainder[0]) << 16) | (u32::from(remainder[1]) << 8);
            out.push(ALPHABET[((bits >> 18) & 0x3F) as usize] as char);
            out.push(ALPHABET[((bits >> 12) & 0x3F) as usize] as char);
            out.push(ALPHABET[((bits >> 6) & 0x3F) as usize] as char);
            out.push('=');
        }
        _ => unreachable!(),
    }
    out
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
    let candidates = which_candidate_names(name);
    for entry in std::env::split_paths(&paths) {
        for candidate_name in &candidates {
            let candidate = entry.join(candidate_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn which_candidate_names(name: &str) -> Vec<String> {
    if !cfg!(windows) || Path::new(name).extension().is_some() {
        return vec![name.to_string()];
    }
    let mut candidates = vec![name.to_string()];
    let path_ext = std::env::var_os("PATHEXT")
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into())
        .to_string_lossy()
        .into_owned();
    for ext in path_ext.split(';') {
        if ext.is_empty() {
            continue;
        }
        candidates.push(format!("{name}{ext}"));
        candidates.push(format!("{name}{}", ext.to_ascii_lowercase()));
    }
    candidates.sort();
    candidates.dedup();
    candidates
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
        Self::from_source(BUILT_IN_PRELUDE)
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
            self.source,
            self.host_cache.clone(),
            target,
            workspace_root,
            dep_providers,
            capability,
        )
    }
}

pub(crate) const BUILT_IN_PRELUDE: &str = concat!(
    include_str!("../prelude/apple.star"),
    "\n",
    include_str!("../prelude/rust.star"),
);

pub(crate) const BUILT_IN_PRELUDE_FILES: &[(&str, &str)] = &[
    (
        "once//prelude/apple.star",
        include_str!("../prelude/apple.star"),
    ),
    (
        "once//prelude/rust.star",
        include_str!("../prelude/rust.star"),
    ),
];

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
        analyze_in_starlark(source, target, dep_providers, &build_dir, capability)
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
    capability: &str,
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

/// If `value` is the canonical select-shape Map (`{ "select": { ... }
/// }`), return the inner branch map. Otherwise return `None`. The
/// resolution mechanism itself lives in the Starlark prelude so that
/// rule-family-specific configuration knowledge (which attributes
/// feed the configuration, which token names are recognised) stays
/// out of the Rust executor; this helper exists only so the graph
/// schema layer can flag selects on `configurable = False` attributes
/// before the prelude ever runs.
#[must_use]
pub fn select_branches(value: &AttrValue) -> Option<&BTreeMap<String, AttrValue>> {
    if let AttrValue::Map(map) = value {
        if map.len() == 1 {
            if let Some(AttrValue::Map(branches)) = map.get("select") {
                return Some(branches);
            }
        }
    }
    None
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

pub(crate) fn json_to_value<'v>(eval: &Evaluator<'v, '_, '_>, json: &JsonValue) -> Value<'v> {
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

pub(crate) fn value_to_json(value: Value<'_>) -> JsonValue {
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

fn unpack_byte_list(value: Value<'_>, field: &str) -> anyhow::Result<Vec<u8>> {
    let list = ListRef::from_value(value).ok_or_else(|| {
        anyhow!(
            "expected `{field}` to be a list of integers in 0..=255, got `{}`",
            value.get_type()
        )
    })?;
    list.iter()
        .map(|item| {
            let int = item.unpack_i32().ok_or_else(|| {
                anyhow!(
                    "expected `{field}` entries to be integers, got `{}`",
                    item.get_type()
                )
            })?;
            u8::try_from(int)
                .map_err(|_| anyhow!("expected `{field}` entries to be in 0..=255, got `{int}`"))
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
    use std::process::Command;
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
        assert!(store.actions[0].cacheable);
    }

    #[test]
    fn run_action_can_mark_declarations_uncacheable() {
        let tmp = TempDir::new().unwrap();
        let store = store_for(tmp.path(), "apps/ios/App");
        let (store, ()) = with_active_store(store, || {
            run(r#"
run_action(
    argv = ["open", ".once/out/apps/ios/App/App.app"],
    outputs = [".once/out/apps/ios/App/run/run.json"],
    cacheable = False,
)
"#)
            .unwrap();
        });
        assert_eq!(store.actions.len(), 1);
        assert!(!store.actions[0].cacheable);
    }

    #[test]
    fn declared_action_defaults_cacheable_when_omitted() {
        let action: DeclaredAction = serde_json::from_value(serde_json::json!({
            "argv": ["swiftc", "App.swift"],
            "outputs": [".once/out/App.o"]
        }))
        .unwrap();

        assert!(action.cacheable);
        assert_eq!(
            serde_json::to_value(&action).unwrap(),
            serde_json::json!({
                "argv": ["swiftc", "App.swift"],
                "outputs": [".once/out/App.o"]
            })
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

    /// Two calls with the same argv but different `env` must spawn the
    /// process twice (no shared cache slot). This is what makes the
    /// `xcode_developer_dir` pin partition Xcode toolchains correctly.
    #[cfg(unix)]
    #[test]
    fn host_command_cache_keys_on_env() {
        let tmp = TempDir::new().unwrap();
        let counter = tmp.path().join("counter");
        let cache = HostCache::default();
        let argv = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf x >> \"$1\"; printf \"$ONCE_TEST_PIN\"".to_string(),
            "sh".to_string(),
            counter.display().to_string(),
        ];
        let mut env_a = BTreeMap::new();
        env_a.insert("ONCE_TEST_PIN".to_string(), "a".to_string());
        let mut env_b = BTreeMap::new();
        env_b.insert("ONCE_TEST_PIN".to_string(), "b".to_string());

        // Distinct env values land in distinct cache slots and the
        // process is re-spawned for each, so the script's stdout
        // reflects each env's pin value.
        assert_eq!(cache.command(&argv, &env_a).unwrap(), "a");
        assert_eq!(cache.command(&argv, &env_b).unwrap(), "b");
        // Repeat the first call: the env_a slot is now warm and
        // reuses the cached stdout without spawning the process.
        assert_eq!(cache.command(&argv, &env_a).unwrap(), "a");

        // Counter increments once per spawn: env_a (cold), env_b
        // (cold), env_a (warm, no spawn) -> two ticks total.
        assert_eq!(std::fs::read_to_string(counter).unwrap(), "xx");
    }

    #[test]
    fn shell_quote_handles_empty_strings_quotes_and_specials() {
        assert_eq!(shell_quote(""), "''");
        // No special characters: single-quote wrap with no escapes.
        assert_eq!(shell_quote("abc"), "'abc'");
        // Single quote in the middle uses the close/escape/reopen form
        // so the resulting word still expands to a single token.
        assert_eq!(shell_quote("a'b"), "'a'\"'\"'b'");
        // Backslashes, dollar signs, double quotes are inert inside the
        // single-quoted POSIX form, so they pass through verbatim.
        assert_eq!(shell_quote("$x \\n \"y\""), "'$x \\n \"y\"'");
    }

    /// [`write_file`] declares an action whose argv is
    /// `["/bin/sh", "-c", script]`. The script must (a) bind the path
    /// before computing its parent directory, (b) include the content
    /// as the only `printf` argument, and (c) declare the path as the
    /// only output.
    #[test]
    fn write_file_records_an_action_with_path_binding_and_content() {
        let tmp = TempDir::new().unwrap();
        let store = store_for(tmp.path(), "apps/ios/Mixed");
        let (store, ()) = with_active_store(store, || {
            run(r#"write_file(".once/out/apps/ios/Mixed/module.modulemap", "module Mixed { export * }\n")"#).unwrap();
        });
        assert_eq!(store.actions.len(), 1);
        let action = &store.actions[0];
        assert_eq!(
            action.outputs,
            vec![".once/out/apps/ios/Mixed/module.modulemap".to_string()]
        );
        assert_eq!(action.argv[0], "/bin/sh");
        assert_eq!(action.argv[1], "-c");
        let script = &action.argv[2];
        assert!(script.contains("__once_path="), "{script}");
        assert!(
            script.contains("printf '%s' 'module Mixed { export * }"),
            "{script}"
        );
        assert!(script.contains("> \"$__once_path\""), "{script}");
        // The path is referenced via the binding, never inlined into
        // the dirname call, so single quotes in the path can't escape
        // the substitution.
        assert!(!script.contains("$(dirname"), "{script}");
    }

    /// The end-to-end script the action declares must actually run and
    /// produce the file on a real shell. This catches scripting bugs
    /// that the structural assertions above would miss.
    #[cfg(unix)]
    #[test]
    fn write_file_script_actually_creates_the_file() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("nested/dir/holds/output.txt");
        // Use a path with a single quote in a parent dir to lock in
        // the fix from the review thread: the script must survive
        // quotes inside `__once_path` without re-tokenising.
        let quoted = tmp.path().join("a'b").join("out.txt");
        let store = AnalysisStore::new(tmp.path().to_path_buf(), String::new(), String::new());
        let (store, ()) = with_active_store(store, || {
            run(&format!(
                r#"write_file({nested:?}, "hello\n")
write_file({quoted:?}, "quoted\n")
"#,
                nested = nested.display().to_string(),
                quoted = quoted.display().to_string(),
            ))
            .unwrap();
        });
        assert_eq!(store.actions.len(), 2);
        for action in &store.actions {
            let status = std::process::Command::new(&action.argv[0])
                .arg(&action.argv[1])
                .arg(&action.argv[2])
                .status()
                .expect("script should spawn");
            assert!(status.success(), "script failed: {:?}", action.argv);
        }
        assert_eq!(std::fs::read_to_string(&nested).unwrap(), "hello\n");
        assert_eq!(std::fs::read_to_string(&quoted).unwrap(), "quoted\n");
    }

    /// `write_bytes` should accept a list of 0..=255 integers, encode
    /// them as base64 in the generated shell script, fold the encoded
    /// bytes into the toolchain identity, and declare the path as its
    /// only output. The primitive is intentionally domain-agnostic;
    /// callers compose arbitrary binary formats in the prelude.
    #[test]
    fn write_bytes_records_action_with_base64_payload() {
        let tmp = TempDir::new().unwrap();
        let store = store_for(tmp.path(), "p");
        let (store, ()) = with_active_store(store, || {
            run(r#"write_bytes(".once/out/p/blob.bin", [0, 1, 2, 254, 255])"#).unwrap();
        });
        assert_eq!(store.actions.len(), 1);
        let action = &store.actions[0];
        assert_eq!(action.outputs, vec![".once/out/p/blob.bin".to_string()]);
        assert_eq!(action.argv[0], "/bin/sh");
        let script = &action.argv[2];
        assert!(script.contains("base64 -d"), "{script}");
        assert!(
            action
                .toolchain_identity
                .as_deref()
                .is_some_and(|id| id.starts_with("once.write_bytes.v1\0")),
            "{:?}",
            action.toolchain_identity
        );
    }

    /// The shell script the action declares must run end-to-end and
    /// reproduce the exact byte sequence on disk, NULs and 0xFF
    /// inclusive. Round-tripping through base64 + `base64 -d` is the
    /// part of `write_bytes` that domain-specific callers depend on.
    #[cfg(unix)]
    #[test]
    fn write_bytes_script_reproduces_exact_byte_sequence() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("blob.bin");
        let store = AnalysisStore::new(tmp.path().to_path_buf(), String::new(), String::new());
        let (store, ()) = with_active_store(store, || {
            run(&format!(
                r"write_bytes({path:?}, [0, 1, 2, 255, 0, 128, 64])",
                path = out.display().to_string(),
            ))
            .unwrap();
        });
        let action = &store.actions[0];
        let status = std::process::Command::new(&action.argv[0])
            .arg(&action.argv[1])
            .arg(&action.argv[2])
            .status()
            .expect("script should spawn");
        assert!(status.success(), "script failed: {:?}", action.argv);
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(bytes, vec![0, 1, 2, 255, 0, 128, 64]);
    }

    #[test]
    fn write_bytes_rejects_out_of_range_integers() {
        let tmp = TempDir::new().unwrap();
        let store = store_for(tmp.path(), "p");
        let (_, err) = with_active_store(store, || {
            run(r#"write_bytes(".once/out/p/blob.bin", [256])"#).unwrap_err()
        });
        let message = format!("{err:?}");
        assert!(message.contains("0..=255"), "{message}");
    }

    #[test]
    fn base64_encode_round_trips_for_short_inputs() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
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
        // `script` is the canonical example of a rule kind that the
        // bundled prelude knows about but provides no Starlark impl
        // for; the analysis driver should hand back a null provider
        // and no actions so the CLI falls back to its own runner.
        let tmp = TempDir::new().unwrap();
        let result = analyze_target(&target("script"), tmp.path(), &[]);
        // `script` does not appear in the Apple prelude's RULES list
        // and is supplied by the CLI's script-runner path; the
        // frontend should error with the same "no rule found" surface
        // it uses for unknown kinds. Confirm that here so the
        // "no impl available" path is exercised end-to-end.
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no rule found for kind `script`"), "{err}");
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
    fn rule_has_impl_reads_custom_rule_impls() {
        let engine = AnalysisEngine::from_source(
            r#"
RULES = [
    {"kind": "custom_library", "impl": lambda ctx: None},
]
"#,
        )
        .unwrap();

        assert!(engine.rule_has_impl("custom_library"));
    }

    #[test]
    fn apple_library_swift_compile_is_split_into_module_and_archive_actions() {
        let source = include_str!("../prelude/apple.star");

        assert!(source.contains("identifier = \"swift_module_compile_"));
        assert!(source.contains("outputs = [swiftmodule, swiftdoc, swift_objc_header]"));
        assert!(source.contains("identifier = \"swift_archive_compile_"));
        assert!(source.contains("outputs = [swift_archive]"));
    }

    #[test]
    fn rule_has_impl_returns_true_for_swift_macro() {
        assert!(rule_has_impl("swift_macro").unwrap());
    }

    #[test]
    fn rule_has_impl_returns_true_for_all_apple_bundle_rules() {
        // Every bundled Apple rule kind now has a Starlark impl that
        // declares actions; the CLI's generic fallback action is
        // bypassed for these kinds in favour of the Starlark-driven
        // analysis.
        assert!(rule_has_impl("apple_framework").unwrap());
        assert!(rule_has_impl("apple_application").unwrap());
        assert!(rule_has_impl("apple_test_bundle").unwrap());
    }

    #[test]
    fn rule_has_impl_returns_false_for_unknown_kind() {
        assert!(!rule_has_impl("mystery_rule").unwrap());
    }

    fn select_attr_value(branches: &[(&str, AttrValue)]) -> AttrValue {
        let inner: BTreeMap<String, AttrValue> = branches
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect();
        let mut outer = BTreeMap::new();
        outer.insert("select".to_string(), AttrValue::Map(inner));
        AttrValue::Map(outer)
    }

    fn eval_prelude_function(
        function_name: &str,
        call_source: &str,
    ) -> std::result::Result<String, String> {
        let prelude = include_str!("../prelude/apple.star");
        eval_prelude_function_in(prelude, function_name, call_source)
    }

    fn eval_prelude_function_in(
        prelude: &str,
        function_name: &str,
        call_source: &str,
    ) -> std::result::Result<String, String> {
        let source = format!("{prelude}\nresult = repr({function_name}{call_source})\n");
        eval_prelude_source_to_repr(source)
    }

    fn eval_prelude_source_to_repr(source: String) -> std::result::Result<String, String> {
        // Build a Starlark module that splices the prelude's source
        // inline and invokes the requested helper. Returning the
        // result as a string via `repr()` keeps the test independent
        // of starlark Value plumbing details.
        Module::with_temp_heap(|module| {
            let ast = AstModule::parse("test.star", source, &Dialect::Standard)
                .map_err(|error| format!("parse: {error:?}"))?;
            let globals = globals_for_prelude();
            let mut eval = Evaluator::new(&module);
            // The prelude calls host_arch() in some helpers, but the
            // resolver path itself doesn't. The host primitives
            // already return inert values outside of an active
            // analysis store, so this evaluates cleanly.
            eval.eval_module(ast, &globals)
                .map_err(|error| format!("eval: {error:?}"))?;
            let result = module
                .get("result")
                .ok_or_else(|| "missing result".to_string())?;
            Ok(result
                .unpack_str()
                .ok_or_else(|| "result was not a string".to_string())?
                .to_string())
        })
    }

    fn eval_prelude_string_function(
        function_name: &str,
        call_source: &str,
    ) -> std::result::Result<String, String> {
        let prelude = include_str!("../prelude/apple.star");
        let source = format!("{prelude}\nresult = {function_name}{call_source}\n");
        Module::with_temp_heap(|module| {
            let ast = AstModule::parse("test.star", source, &Dialect::Standard)
                .map_err(|error| format!("parse: {error:?}"))?;
            let globals = globals_for_prelude();
            let mut eval = Evaluator::new(&module);
            eval.eval_module(ast, &globals)
                .map_err(|error| format!("eval: {error:?}"))?;
            let result = module
                .get("result")
                .ok_or_else(|| "missing result".to_string())?;
            Ok(result
                .unpack_str()
                .ok_or_else(|| "result was not a string".to_string())?
                .to_string())
        })
    }

    fn starlark_string_literal(value: &str) -> String {
        serde_json::to_string(value).unwrap()
    }

    #[cfg(unix)]
    fn write_executable(path: &Path, contents: &str) {
        use std::os::unix::fs::PermissionsExt;

        std::fs::write(path, contents).unwrap();
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    #[test]
    fn prelude_resolve_select_picks_matching_branch() {
        let out = eval_prelude_function(
            "_resolve_select",
            r#"({"select": {"ios": ["FOO"], "macos": ["BAR"]}}, ["ios"], "tgt", "defines")"#,
        )
        .unwrap();
        assert_eq!(out, "[\"FOO\"]");
    }

    #[test]
    fn prelude_resolve_select_falls_back_to_default() {
        let out = eval_prelude_function(
            "_resolve_select",
            r#"({"select": {"macos": "M", "default": "fallback"}}, ["ios"], "tgt", "x")"#,
        )
        .unwrap();
        assert_eq!(out, "\"fallback\"");
    }

    #[test]
    fn prelude_resolve_select_prefers_longest_composite_key() {
        let out = eval_prelude_function(
            "_resolve_select",
            r#"({"select": {"ios": "ios-any", "ios:simulator": "ios-sim"}}, ["ios", "simulator"], "tgt", "x")"#,
        )
        .unwrap();
        assert_eq!(out, "\"ios-sim\"");
    }

    #[test]
    fn prelude_resolve_select_fails_without_default() {
        let err = eval_prelude_function(
            "_resolve_select",
            r#"({"select": {"macos": "M"}}, ["ios"], "tgt", "x")"#,
        )
        .unwrap_err();
        assert!(err.contains("no branch matching"), "{err}");
    }

    #[test]
    fn prelude_cargo_metadata_targets_preserve_rust_target() {
        let prelude = format!(
            "{}\n{}",
            include_str!("../prelude/apple.star"),
            include_str!("../prelude/rust.star")
        );
        let out = eval_prelude_function_in(
            &prelude,
            "_cargo_metadata_targets",
            r#"({
                "attrs": {
                    "target": "x86_64-apple-darwin",
                    "vendor_dir": "third_party/rust/vendor",
                },
            }, {
                "packages": [{
                    "id": "registry+https://github.com/rust-lang/crates.io-index#cpufeatures@0.2.17",
                    "name": "cpufeatures",
                    "version": "0.2.17",
                    "source": "registry+https://github.com/rust-lang/crates.io-index",
                    "manifest_path": "/workspace/vendor/cpufeatures-0.2.17/Cargo.toml",
                    "targets": [{
                        "name": "cpufeatures",
                        "kind": ["lib"],
                        "crate_types": ["lib"],
                        "src_path": "/workspace/vendor/cpufeatures-0.2.17/src/lib.rs",
                        "edition": "2018",
                    }],
                }],
                "resolve": {
                    "nodes": [{
                        "id": "registry+https://github.com/rust-lang/crates.io-index#cpufeatures@0.2.17",
                        "features": [],
                        "deps": [],
                    }],
                },
            })"#,
        )
        .unwrap();

        assert!(out.contains("\"target\": \"x86_64-apple-darwin\""), "{out}");
        assert!(
            out.contains("\"srcs\": [\"third_party/rust/vendor/cpufeatures-0.2.17/Cargo.toml\", \"third_party/rust/vendor/cpufeatures-0.2.17/build.rs\", \"third_party/rust/vendor/cpufeatures-0.2.17/src/**/*.rs\"]"),
            "{out}"
        );
    }

    #[test]
    fn prelude_cargo_metadata_targets_split_proc_macro_host_deps() {
        let prelude = format!(
            "{}\n{}",
            include_str!("../prelude/apple.star"),
            include_str!("../prelude/rust.star")
        );
        let source = format!(
            r#"{prelude}
targets = _cargo_metadata_targets({{
    "attrs": {{
        "target": "x86_64-apple-darwin",
        "vendor_dir": "third_party/rust/vendor",
    }},
}}, {{
    "packages": [
        {{
            "id": "registry+https://github.com/rust-lang/crates.io-index#quote@1.0.45",
            "name": "quote",
            "version": "1.0.45",
            "source": "registry+https://github.com/rust-lang/crates.io-index",
            "manifest_path": "/workspace/vendor/quote-1.0.45/Cargo.toml",
            "targets": [{{
                "name": "quote",
                "kind": ["lib"],
                "crate_types": ["lib"],
                "src_path": "/workspace/vendor/quote-1.0.45/src/lib.rs",
                "edition": "2018",
            }}],
        }},
        {{
            "id": "registry+https://github.com/rust-lang/crates.io-index#linktime-proc-macro@0.2.0",
            "name": "linktime-proc-macro",
            "version": "0.2.0",
            "source": "registry+https://github.com/rust-lang/crates.io-index",
            "manifest_path": "/workspace/vendor/linktime-proc-macro-0.2.0/Cargo.toml",
            "targets": [{{
                "name": "linktime_proc_macro",
                "kind": ["proc-macro"],
                "crate_types": ["proc-macro"],
                "src_path": "/workspace/vendor/linktime-proc-macro-0.2.0/src/lib.rs",
                "edition": "2021",
            }}],
        }},
    ],
    "resolve": {{
        "nodes": [
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#quote@1.0.45",
                "features": [],
                "deps": [],
            }},
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#linktime-proc-macro@0.2.0",
                "features": [],
                "deps": [{{
                    "name": "quote",
                    "pkg": "registry+https://github.com/rust-lang/crates.io-index#quote@1.0.45",
                    "dep_kinds": [{{"kind": None}}],
                }}],
            }},
        ],
    }},
}})
by_name = {{target["name"]: target for target in targets}}
result = repr([
    by_name["quote-1.0.45"]["attrs"].get("target"),
    by_name["quote-1.0.45-host"]["attrs"].get("target"),
    by_name["linktime-proc-macro-0.2.0"]["attrs"].get("target"),
    by_name["linktime-proc-macro-0.2.0"]["deps"],
])
"#
        );
        let out = eval_prelude_source_to_repr(source).unwrap();

        assert_eq!(
            out,
            "[\"x86_64-apple-darwin\", None, None, [\"./quote-1.0.45-host\"]]"
        );
    }

    #[cfg(unix)]
    #[test]
    fn prelude_rust_build_script_metadata_deps_are_not_duplicated() {
        let prelude = format!(
            "{}\n{}",
            include_str!("../prelude/apple.star"),
            include_str!("../prelude/rust.star")
        );
        let source = format!(
            r#"{prelude}
ctx = {{
    "label": {{
        "package": "crates/app",
        "name": "app",
        "id": "crates/app/app",
    }},
    "attr": {{
        "build_script": "build.rs",
        "crate_root": "src/lib.rs",
    }},
    "deps": [{{
        "label_id": "third_party/rust/native",
        "crate_name": "native",
        "rlib": ".once/out/native/libnative.rlib",
        "links": "native",
        "build_script_stdout": ".once/out/native/build-script.stdout",
    }}],
    "srcs": ["src/**/*.rs"],
}}
_rust_compile(ctx, "rlib", "src/lib.rs", "libapp.rlib")
result = repr("ok")
"#
        );
        let workspace = TempDir::new().unwrap();
        let store = store_for(workspace.path(), "crates/app/app");

        let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

        assert_eq!(out.unwrap(), "\"ok\"");
        let script = store
            .actions
            .iter()
            .find(|action| action.identifier.as_deref() == Some("crates/app/app:build-script"))
            .and_then(|action| action.argv.get(2))
            .unwrap();
        assert_eq!(script.matches("done <").count(), 1, "{script}");
    }

    #[cfg(unix)]
    #[test]
    fn prelude_rust_build_script_env_encodes_rustflags() {
        let prelude = format!(
            "{}\n{}",
            include_str!("../prelude/apple.star"),
            include_str!("../prelude/rust.star")
        );
        let source = format!(
            r#"{prelude}
rustc, _identity, host_triple = _rustc_toolchain("")
ctx = {{
    "label": {{
        "package": "crates/app",
        "name": "app",
        "id": "crates/app/app",
    }},
    "attr": {{
        "rustc_flags": ["-C", "opt-level=3"],
    }},
    "deps": [],
    "srcs": [],
}}
env = _rust_build_script_env(
    ctx,
    rustc,
    host_triple,
    host_triple,
    ".once/out/app/build",
    "crates/app/build.rs",
)
result = repr(env.get("CARGO_ENCODED_RUSTFLAGS"))
"#
        );
        let workspace = TempDir::new().unwrap();
        let store = store_for(workspace.path(), "crates/app/app");

        let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

        assert_eq!(out.unwrap(), "\"-C\\x1fopt-level=3\"");
    }

    #[cfg(unix)]
    #[test]
    fn prelude_rust_proc_macro_compile_uses_host_target() {
        let prelude = format!(
            "{}\n{}",
            include_str!("../prelude/apple.star"),
            include_str!("../prelude/rust.star")
        );
        let source = format!(
            r#"{prelude}
_rustc, _identity, host_triple = _rustc_toolchain("")
def other_target(host_triple):
    if host_triple == "aarch64-unknown-linux-gnu":
        return "x86_64-unknown-linux-gnu"
    return "aarch64-unknown-linux-gnu"
ctx = {{
    "label": {{
        "package": "macros/stringify",
        "name": "stringify",
        "id": "macros/stringify",
    }},
    "attr": {{
        "target": other_target(host_triple),
        "crate_root": "src/lib.rs",
    }},
    "deps": [],
    "srcs": ["src/**/*.rs"],
}}
_rust_compile(ctx, "proc-macro", "src/lib.rs", "libstringify.so")
result = repr("ok")
"#
        );
        let workspace = TempDir::new().unwrap();
        let store = store_for(workspace.path(), "macros/stringify");

        let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

        assert_eq!(out.unwrap(), "\"ok\"");
        let action = &store.actions[0];
        assert!(
            !action.argv.iter().any(|arg| arg == "--target"),
            "{:?}",
            action.argv
        );
        assert_eq!(
            action.outputs,
            vec![".once/out/macros/stringify/libstringify.so".to_string()]
        );
    }

    #[cfg(unix)]
    #[test]
    fn prelude_rust_build_script_env_uses_absolute_c_tool_paths() {
        let prelude = format!(
            "{}\n{}",
            include_str!("../prelude/apple.star"),
            include_str!("../prelude/rust.star")
        );
        let source = format!(
            r#"{prelude}
rustc, _identity, host_triple = _rustc_toolchain("")
ctx = {{
    "label": {{
        "package": "third_party/rust/vendor/pkg-1.0.0",
        "name": "pkg",
        "id": "third_party/rust/vendor/pkg-1.0.0",
    }},
    "attr": {{}},
    "srcs": [],
}}
tool_env = _rust_c_tool_env(host_triple, host_triple)
build_env = _rust_build_script_env(
    ctx,
    rustc,
    host_triple,
    host_triple,
    ".once/out/pkg/build",
    "third_party/rust/vendor/pkg-1.0.0/build.rs",
)
result = repr([
    tool_env.get("CC"),
    tool_env.get("AR"),
    tool_env.get("PATH"),
    build_env.get("CC"),
    build_env.get("AR"),
    build_env.get("PATH"),
])
"#
        );
        let workspace = TempDir::new().unwrap();
        let store = store_for(workspace.path(), "");

        let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
        let values: Vec<String> = serde_json::from_str(&out.unwrap()).unwrap();

        assert!(std::path::Path::new(&values[0]).is_absolute());
        assert!(std::path::Path::new(&values[1]).is_absolute());
        assert_eq!(values[0], values[3]);
        assert_eq!(values[1], values[4]);
        assert_eq!(values[2], values[5]);
        for entry in values[2].split(':') {
            assert!(std::path::Path::new(entry).is_absolute());
        }
        let cc_dir = std::path::Path::new(&values[0])
            .parent()
            .unwrap()
            .to_string_lossy();
        assert!(values[2].split(':').any(|entry| entry == cc_dir));
    }

    #[cfg(unix)]
    #[test]
    fn prelude_rust_build_script_env_does_not_use_host_c_tool_for_cross_target() {
        let prelude = format!(
            "{}\n{}",
            include_str!("../prelude/apple.star"),
            include_str!("../prelude/rust.star")
        );
        let source = format!(
            r#"{prelude}
_rustc, _identity, host_triple = _rustc_toolchain("")
def other_target(host_triple):
    if host_triple == "aarch64-unknown-linux-gnu":
        return "x86_64-unknown-linux-gnu"
    return "aarch64-unknown-linux-gnu"
target = other_target(host_triple)
env = _rust_c_tool_env(target, host_triple)
result = repr([
    env.get("CC"),
    env.get("AR"),
    env.get("PATH"),
    env.get("CC_" + target.replace("-", "_")),
    env.get("CC_" + target),
])
"#
        );
        let workspace = TempDir::new().unwrap();
        let store = store_for(workspace.path(), "");

        let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

        assert_eq!(out.unwrap(), "[None, None, None, None, None]");
    }

    #[test]
    fn prelude_ios_simulator_selection_filters_to_iphone_and_ipad() {
        let out = eval_prelude_string_function(
            "_ios_simulator_selection_script",
            r#"("/usr/bin/xcrun")"#,
        )
        .unwrap();

        assert!(out.contains("ONCE_APPLE_SIMULATOR_UDID"), "{out}");
        assert!(out.contains("simctl list devices booted"), "{out}");
        assert!(out.contains("simctl list devices available"), "{out}");
        assert!(out.contains("/iPhone/ s/^.*"), "{out}");
        assert!(out.contains("/iPad/ s/^.*"), "{out}");
        assert!(out.contains("(Booted)[[:space:]]*$"), "{out}");
        assert!(out.contains("(Shutdown)[[:space:]]*$"), "{out}");
        assert!(!out.contains("sed -n 's/.*"), "{out}");
    }

    #[cfg(unix)]
    #[test]
    fn prelude_ios_simulator_selection_script_picks_booted_ios_device() {
        let tmp = TempDir::new().unwrap();
        let xcrun = tmp.path().join("xcrun");
        write_executable(
            &xcrun,
            r#"#!/bin/sh
if [ "${1:-}" = "simctl" ] && [ "${2:-}" = "list" ] && [ "${3:-}" = "devices" ] && [ "${4:-}" = "booted" ]; then
  printf '%s\n' '    Apple TV (AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA) (Booted)'
  printf '%s\n' '    iPhone Preview (BBBBBBBB-BBBB-BBBB-BBBB-BBBBBBBBBBBB) (Extra) (Booted)'
  printf '%s\n' '    iPhone 15 Pro (11111111-1111-1111-1111-111111111111) (Booted)'
  exit 0
fi
if [ "${1:-}" = "simctl" ] && [ "${2:-}" = "list" ] && [ "${3:-}" = "devices" ] && [ "${4:-}" = "available" ]; then
  printf '%s\n' '    iPad Pro (22222222-2222-2222-2222-222222222222) (Shutdown)'
  exit 0
fi
exit 1
"#,
        );
        let call = format!(
            "({})",
            starlark_string_literal(&xcrun.display().to_string())
        );
        let selection_script =
            eval_prelude_string_function("_ios_simulator_selection_script", &call).unwrap();
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("{selection_script}\nprintf '%s' \"$simulator_id\""))
            .output()
            .unwrap();

        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "11111111-1111-1111-1111-111111111111"
        );
    }

    #[cfg(unix)]
    #[test]
    fn prelude_ios_simulator_selection_script_errors_without_ios_device() {
        let tmp = TempDir::new().unwrap();
        let xcrun = tmp.path().join("xcrun");
        write_executable(
            &xcrun,
            r#"#!/bin/sh
if [ "${1:-}" = "simctl" ] && [ "${2:-}" = "list" ] && [ "${3:-}" = "devices" ]; then
  printf '%s\n' '    Apple TV (AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA) (Booted)'
  exit 0
fi
exit 1
"#,
        );
        let call = format!(
            "({})",
            starlark_string_literal(&xcrun.display().to_string())
        );
        let selection_script =
            eval_prelude_string_function("_ios_simulator_selection_script", &call).unwrap();
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("{selection_script}\nprintf '%s' \"$simulator_id\""))
            .output()
            .unwrap();

        assert!(!output.status.success(), "{output:?}");
        assert!(String::from_utf8(output.stderr)
            .unwrap()
            .contains("no booted or available iOS simulator found"));
    }

    #[cfg(unix)]
    #[test]
    fn prelude_swift_testing_macros_plugin_uses_swift_toolchain_path() {
        let tmp = TempDir::new().unwrap();
        let xcrun = tmp.path().join("xcrun");
        let swiftc = tmp
            .path()
            .join("Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc");
        std::fs::create_dir_all(swiftc.parent().unwrap()).unwrap();
        write_executable(
            &xcrun,
            &format!(
                r#"#!/bin/sh
if [ "${{1:-}}" = "--find" ] && [ "${{2:-}}" = "swiftc" ]; then
  printf '%s\n' {}
  exit 0
fi
exit 1
"#,
                starlark_string_literal(&swiftc.display().to_string())
            ),
        );
        let store = store_for(tmp.path(), "");
        let call = format!(
            "({}, {{}})",
            starlark_string_literal(&xcrun.display().to_string())
        );

        let (_, out) = with_active_store(store, || {
            eval_prelude_string_function("_swift_testing_macros_plugin", &call)
        });

        assert_eq!(
            out.unwrap(),
            tmp.path()
                .join("Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/host/plugins/testing/libTestingMacros.dylib")
                .display()
                .to_string()
        );
    }

    #[cfg(unix)]
    #[test]
    fn prelude_swift_testing_macros_plugin_rejects_unexpected_swiftc_path() {
        let tmp = TempDir::new().unwrap();
        let xcrun = tmp.path().join("xcrun");
        write_executable(
            &xcrun,
            r#"#!/bin/sh
if [ "${1:-}" = "--find" ] && [ "${2:-}" = "swiftc" ]; then
  printf '%s\n' '/tmp/swiftc'
  exit 0
fi
exit 1
"#,
        );
        let store = store_for(tmp.path(), "");
        let call = format!(
            "({}, {{}})",
            starlark_string_literal(&xcrun.display().to_string())
        );

        let (_, err) = with_active_store(store, || {
            eval_prelude_string_function("_swift_testing_macros_plugin", &call).unwrap_err()
        });

        assert!(
            err.contains("unable to derive Swift toolchain path"),
            "{err}"
        );
    }

    #[test]
    fn prelude_ios_simulator_selection_helper_feeds_run_and_test_scripts() {
        let source = include_str!("../prelude/apple.star");

        assert_eq!(
            source
                .matches("_ios_simulator_selection_script(xcrun) +")
                .count(),
            2
        );
    }

    /// The prelude `_serialize_hmap` helper must lay out the
    /// header-map byte sequence correctly: 4-byte magic, version 1,
    /// reserved 0, the rest of the header, a power-of-two bucket
    /// array, and a string table that starts with a 0 byte. We assert
    /// each invariant from a Starlark-driven run so the format
    /// implementation stays a Starlark concern.
    #[test]
    fn prelude_serialize_hmap_lays_out_canonical_header_and_entries() {
        let prelude = include_str!("../prelude/apple.star");
        let source = format!(
            "{prelude}\n\
             entries = {{\"Foo.h\": \"AppCore/Foo.h\", \"Bar.h\": \"AppCore/Bar.h\"}}\n\
             bytes = _serialize_hmap(entries)\n"
        );
        let mut bytes: Option<Vec<u8>> = None;
        Module::with_temp_heap(|module| {
            let ast = AstModule::parse("test.star", source, &Dialect::Standard)?;
            let globals = globals_for_prelude();
            let mut eval = Evaluator::new(&module);
            eval.eval_module(ast, &globals)?;
            let value = module.get("bytes").expect("bytes binding");
            let list = ListRef::from_value(value).expect("bytes is a list");
            let collected: Vec<u8> = list
                .iter()
                .map(|item| u8::try_from(item.unpack_i32().expect("int byte")).expect("0..=255"))
                .collect();
            bytes = Some(collected);
            starlark::Result::Ok(())
        })
        .expect("prelude eval");
        let bytes = bytes.unwrap();

        // magic + version + reserved
        assert_eq!(&bytes[0..4], &0x6861_6D70_u32.to_le_bytes());
        assert_eq!(&bytes[4..6], &1u16.to_le_bytes());
        assert_eq!(&bytes[6..8], &0u16.to_le_bytes());

        // num_entries == 2; num_buckets is a power of two; strings
        // offset lands right after the bucket array.
        let strings_off = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
        let num_entries = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        let num_buckets = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        assert_eq!(num_entries, 2);
        assert!(num_buckets.is_power_of_two() && num_buckets >= 2);
        assert_eq!(strings_off, 24 + (num_buckets as usize) * 12);
        assert_eq!(bytes[strings_off], 0);
    }

    #[test]
    fn prelude_apple_config_tokens_rejects_select_on_platform() {
        let err = eval_prelude_function(
            "_apple_config_tokens",
            r#"({"platform": {"select": {"default": "ios"}}}, "tgt")"#,
        )
        .unwrap_err();
        assert!(
            err.contains("attribute `platform` cannot use select()"),
            "{err}"
        );
    }

    /// `_resolve_attrs` must reject `select()` on attributes the rule
    /// schema marks non-configurable (e.g. `module_name`). Without
    /// this guard, a select on `module_name` would silently resolve
    /// against the configuration and the build would proceed with a
    /// rewritten module name, defeating the schema's intent.
    #[test]
    fn prelude_resolve_attrs_rejects_select_on_non_configurable_attr() {
        let err = eval_prelude_function(
            "_resolve_attrs",
            r#"({"platform": "ios", "module_name": {"select": {"ios": "X", "default": "Y"}}}, "tgt", ["module_name"])"#,
        )
        .unwrap_err();
        assert!(
            err.contains("attribute `module_name` is not configurable but uses select()"),
            "{err}"
        );
    }

    #[test]
    fn select_branches_detects_canonical_shape() {
        let value = select_attr_value(&[("ios", AttrValue::String("yes".to_string()))]);
        assert!(select_branches(&value).is_some());

        let not_a_select = AttrValue::Map(BTreeMap::from([(
            "select".to_string(),
            AttrValue::String("x".to_string()),
        )]));
        assert!(select_branches(&not_a_select).is_none());

        let map_with_extra_key = AttrValue::Map(BTreeMap::from([
            ("select".to_string(), AttrValue::Map(BTreeMap::new())),
            ("else".to_string(), AttrValue::String("x".to_string())),
        ]));
        assert!(select_branches(&map_with_extra_key).is_none());
    }
}
