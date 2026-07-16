use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

/// A portable filesystem operation declared by a target kind impl.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeclaredActionOperation {
    WriteFile {
        path: String,
        bytes: Vec<u8>,
    },
    CopyPath {
        sources: Vec<String>,
        destination: String,
        mode: DeclaredCopyPathMode,
    },
    MaterializeHostFile {
        source: String,
        source_sha256: String,
        destination: String,
    },
    PreparePath {
        path: String,
        mode: DeclaredPreparePathMode,
    },
    WriteTreeDigest {
        root: String,
        output: String,
        include_suffixes: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeclaredCopyPathMode {
    File,
    Tree,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeclaredPreparePathMode {
    Remove,
    Directory,
}

/// A single action declared by a target kind impl.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DeclaredAction {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<DeclaredActionOperation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arg_files: Vec<DeclaredArgFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    /// Workspace-relative file the command's stdout is redirected into,
    /// captured as an ordinary output. `None` keeps stream capture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    /// Workspace-relative file the command's stderr is redirected into.
    /// Equal to `stdout` merges both streams into one file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clean_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub create_dirs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    #[serde(default = "default_cacheable", skip_serializing_if = "is_true")]
    pub cacheable: bool,
    #[serde(
        default = "default_depends_on_prior_actions",
        skip_serializing_if = "is_true"
    )]
    pub depends_on_prior_actions: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DeclaredArgFile {
    pub path: String,
    pub format: DeclaredArgFileFormat,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DeclaredArgFileFormat {
    LineDelimited,
}

fn default_cacheable() -> bool {
    true
}

fn default_depends_on_prior_actions() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_true(value: &bool) -> bool {
    *value
}

/// Per-target collection of declared outputs, actions, and the host
/// context the target kind impl needs (workspace root + package for globbing,
/// build dir for output declaration).
#[derive(Debug, Default)]
pub struct AnalysisStore {
    pub workspace_root: PathBuf,
    pub package: String,
    pub build_dir: String,
    pub declared_outputs: Vec<String>,
    pub actions: Vec<DeclaredAction>,
    pub(super) host_cache: HostCache,
}

impl AnalysisStore {
    #[must_use]
    pub fn new(workspace_root: PathBuf, package: String, build_dir: String) -> Self {
        Self::with_host_cache(workspace_root, package, build_dir, HostCache::default())
    }

    #[must_use]
    pub(super) fn with_host_cache(
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
    merge_stderr: bool,
}

type HostEnvLookup = dyn Fn(&str) -> Option<String> + Send + Sync;

pub(super) struct HostCache {
    which: Arc<Mutex<BTreeMap<String, Option<String>>>>,
    commands: Arc<Mutex<BTreeMap<CommandKey, String>>>,
    host_env: Arc<HostEnvLookup>,
    tool_paths: Arc<BTreeMap<String, String>>,
}

impl std::fmt::Debug for HostCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostCache").finish_non_exhaustive()
    }
}

impl Clone for HostCache {
    fn clone(&self) -> Self {
        Self {
            which: Arc::clone(&self.which),
            commands: Arc::clone(&self.commands),
            host_env: Arc::clone(&self.host_env),
            tool_paths: Arc::clone(&self.tool_paths),
        }
    }
}

impl Default for HostCache {
    fn default() -> Self {
        Self::with_env_lookup(|name| std::env::var(name).ok())
    }
}

impl HostCache {
    fn with_env_lookup(host_env: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        Self {
            which: Arc::new(Mutex::new(BTreeMap::new())),
            commands: Arc::new(Mutex::new(BTreeMap::new())),
            host_env: Arc::new(host_env),
            tool_paths: Arc::new(BTreeMap::new()),
        }
    }

    pub(super) fn with_tool_paths(tool_paths: BTreeMap<String, String>) -> Self {
        Self {
            tool_paths: Arc::new(tool_paths),
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(super) fn with_env(vars: BTreeMap<String, String>) -> Self {
        let vars = Arc::new(vars);
        Self::with_env_lookup(move |name| vars.get(name).cloned())
    }

    pub(super) fn env(&self, name: &str) -> Option<String> {
        (self.host_env)(name)
    }

    /// Resolve `name` on `PATH`, caching the result.
    ///
    /// The lock is released before the filesystem walk so concurrent
    /// `host_which` calls for different binaries don't serialise on
    /// each other.
    pub(super) fn which(&self, name: &str) -> Result<Option<String>> {
        if let Some(path) = self.tool_paths.get(name) {
            return Ok(Some(path.clone()));
        }
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
    pub(super) fn command(
        &self,
        argv: &[String],
        env: &BTreeMap<String, String>,
        merge_stderr: bool,
    ) -> Result<String> {
        let key = CommandKey {
            argv: argv.to_vec(),
            env: env.clone(),
            merge_stderr,
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
        // Some tools (kotlinc, older javac) print their version banner to
        // stderr. `merge_stderr` folds it in so version probes need no
        // host shell `2>&1`.
        let mut combined = String::from_utf8(output.stdout)
            .map_err(|error| anyhow!("non-utf8 stdout: {error}"))?;
        if merge_stderr {
            let stderr = String::from_utf8(output.stderr)
                .map_err(|error| anyhow!("non-utf8 stderr: {error}"))?;
            combined.push_str(&stderr);
        }
        self.lock_commands()?.insert(key, combined.clone());
        Ok(combined)
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

pub(super) fn with_store_mut<R>(f: impl FnOnce(Option<&mut AnalysisStore>) -> R) -> R {
    ACTIVE_STORE.with(|cell| f(cell.borrow_mut().as_mut()))
}

pub(super) fn with_store<R>(f: impl FnOnce(Option<&AnalysisStore>) -> R) -> R {
    ACTIVE_STORE.with(|cell| f(cell.borrow().as_ref()))
}

pub(super) fn analysis_active() -> bool {
    ACTIVE_STORE.with(|cell| cell.borrow().is_some())
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

pub(super) fn which_candidate_names(name: &str) -> Vec<String> {
    let path_ext = std::env::var_os("PATHEXT").map(|value| value.to_string_lossy().into_owned());
    which_candidate_names_for(name, cfg!(windows), path_ext.as_deref())
}

pub(super) fn which_candidate_names_for(
    name: &str,
    windows: bool,
    path_ext: Option<&str>,
) -> Vec<String> {
    if !windows || Path::new(name).extension().is_some() {
        return vec![name.to_string()];
    }

    let mut candidates = Vec::new();
    let mut seen = BTreeSet::new();
    let path_ext = path_ext.unwrap_or(".COM;.EXE;.BAT;.CMD");
    for ext in path_ext.split(';').map(str::trim) {
        if ext.is_empty() {
            continue;
        }
        for candidate in [
            format!("{name}{ext}"),
            format!("{name}{}", ext.to_ascii_lowercase()),
        ] {
            if seen.insert(candidate.clone()) {
                candidates.push(candidate);
            }
        }
    }
    candidates
}
