use std::cell::RefCell;
use std::collections::BTreeMap;
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default = "default_cacheable", skip_serializing_if = "is_true")]
    pub cacheable: bool,
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
    RustcResponse,
}

fn default_cacheable() -> bool {
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
}

#[derive(Debug, Clone, Default)]
pub(super) struct HostCache {
    which: Arc<Mutex<BTreeMap<String, Option<String>>>>,
    commands: Arc<Mutex<BTreeMap<CommandKey, String>>>,
}

impl HostCache {
    /// Resolve `name` on `PATH`, caching the result.
    ///
    /// The lock is released before the filesystem walk so concurrent
    /// `host_which` calls for different binaries don't serialise on
    /// each other.
    pub(super) fn which(&self, name: &str) -> Result<Option<String>> {
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
    ) -> Result<String> {
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
