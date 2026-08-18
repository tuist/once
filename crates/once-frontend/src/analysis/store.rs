use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

const HOST_COMMAND_OUTPUT_LIMIT: u64 = 16 * 1024 * 1024;

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
    LinkPath {
        source: String,
        destination: String,
    },
    MaterializeHostFile {
        source: String,
        source_sha256: String,
        destination: String,
    },
    MaterializeHostTree {
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
    WriteArchive {
        entries: Vec<DeclaredArchiveEntry>,
        output: String,
        sha256_output: Option<String>,
        format: DeclaredArchiveFormat,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DeclaredArchiveEntry {
    pub kind: DeclaredArchiveEntryKind,
    pub source: Option<String>,
    pub path: String,
    pub mode: u32,
    pub directory_mode: u32,
    pub owner_id: u64,
    pub group_id: u64,
    pub mtime: u64,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeclaredArchiveEntryKind {
    File,
    Directory,
    Tree,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeclaredArchiveFormat {
    Tar,
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
    /// Network policy the action declares. `None` keeps the default
    /// (unrestricted). Mirrors `sandbox`: a string parsed at lowering time
    /// so the graph record stays a plain declaration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(
        default = "default_success_exit_codes",
        skip_serializing_if = "is_default_success_exit_codes"
    )]
    pub success_exit_codes: Vec<i32>,
    #[serde(default = "default_cacheable", skip_serializing_if = "is_true")]
    pub cacheable: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub inherit_parent_env: bool,
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

fn default_success_exit_codes() -> Vec<i32> {
    vec![0]
}

fn is_default_success_exit_codes(codes: &[i32]) -> bool {
    codes == [0]
}

fn default_depends_on_prior_actions() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_true(value: &bool) -> bool {
    *value
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

/// One answer a target kind impl received from outside itself.
///
/// Analysis is a function of the target definition, its dependencies'
/// providers, and the answers recorded here. Replaying it therefore means
/// asking each of these questions again and comparing: same questions, same
/// answers, same analysis. Every global that can return something the caller
/// did not already know has a variant, because a channel with no variant is a
/// way for the world to change without the record noticing.
///
/// Answers that could be large are held as content digests rather than
/// content, so a record stays small enough to be worth reading.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Observation {
    Platform {
        arch: String,
        os: String,
    },
    Env {
        name: String,
        value: Option<String>,
    },
    Which {
        name: String,
        resolved: Option<String>,
    },
    Command {
        argv: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<String>,
        merge_stderr: bool,
        output_sha256: String,
        /// Description of the program that answered, so a caller that cannot
        /// afford to ask again has something to compare. Absent in records
        /// written before this was captured, which such a caller must treat as
        /// unanswerable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        program: Option<String>,
    },
    FileDigest {
        path: String,
        sha256: String,
    },
    TreeDigest {
        path: String,
        sha256: String,
    },
    FileExists {
        path: String,
        exists: bool,
    },
    /// Whether anything is at the path, of any kind. Distinct from
    /// `FileExists`, which asks whether a regular file is.
    PathExists {
        path: String,
        exists: bool,
    },
    /// Whether one path resolves to somewhere inside another. Both sides are
    /// resolved through their symlinks, so this is a question about the
    /// filesystem and not only about the two strings.
    PathWithin {
        path: String,
        root: String,
        within: bool,
    },
    FileText {
        path: String,
        sha256: String,
    },
    FileContains {
        path: String,
        needle: String,
        found: bool,
    },
    ReadDir {
        path: String,
        names: Vec<String>,
    },
    Paths {
        expansion: String,
        package: String,
        patterns: Vec<String>,
        excludes: Vec<String>,
        matches: Vec<String>,
    },
}

/// How much a replay is willing to spend to check a recorded host command.
///
/// A version probe is cheap to ask again, and asking leaves no gap between what
/// was recorded and what is true. A package manager's dependency resolution is
/// not: asking again costs the entire saving. For those, the caller settles for
/// the program being the same program, and takes on the assumption that the
/// command's answer follows from the inputs it declared and the host state
/// recorded beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandPolicy {
    /// Run the command again and compare its output.
    Rerun,
    /// Compare the program that would answer, and trust the declared inputs for
    /// the rest.
    TrustDeclaredInputs,
}

/// Everything one analysis learned from outside itself, in the order it asked.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct AnalysisObservations {
    entries: Vec<Observation>,
    /// Set when analysis read something this ledger cannot describe. Such an
    /// analysis is never stored, because there would be no way to find out
    /// later whether the answer still holds.
    #[serde(default, skip_serializing_if = "is_false")]
    incomplete: bool,
}

impl AnalysisObservations {
    #[must_use]
    pub fn is_complete(&self) -> bool {
        !self.incomplete
    }

    #[must_use]
    pub fn entries(&self) -> &[Observation] {
        &self.entries
    }

    fn record(&mut self, observation: Observation) {
        // A target that asks the same question twice records it once. Order is
        // otherwise preserved, which keeps a record readable next to the impl
        // that produced it.
        if self.entries.contains(&observation) {
            return;
        }
        self.entries.push(observation);
    }

    /// Fold another ledger into this one, keeping it free of duplicates and
    /// incomplete if either was.
    pub fn absorb(&mut self, other: Self) {
        self.incomplete |= other.incomplete;
        for entry in other.entries {
            self.record(entry);
        }
    }

    /// Add one answer directly, for tests that need a ledger without running an
    /// implementation to produce it.
    pub fn push_for_test(&mut self, observation: Observation) {
        self.record(observation);
    }

    /// Mark this ledger as one that cannot be checked, for tests covering what
    /// happens to such an analysis.
    pub fn mark_incomplete_for_test(&mut self) {
        self.incomplete = true;
    }
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
    /// This target's own observations. The host cache below is shared with
    /// every sibling target so one expensive lookup serves all of them; the
    /// ledger is not, because "what did this target read" has to be answerable
    /// per target or a replay would revalidate the whole workspace.
    pub observations: AnalysisObservations,
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
            observations: AnalysisObservations::default(),
            host_cache,
        }
    }
}

/// Record one answer against the target currently being analysed.
pub(super) fn observe(observation: Observation) {
    with_store_mut(|store| {
        if let Some(store) = store {
            store.observations.record(observation);
        }
    });
}

/// Mark the analysis in progress as one that cannot be replayed, for a global
/// whose answer the ledger has no way to describe.
///
/// Nothing calls this today, because every global that can return host state
/// records it. It exists so that adding such a global is a one-line change
/// rather than a silent hole, and `every_global_is_classified` fails until the
/// author has decided which of the two it is.
#[allow(dead_code)]
pub(super) fn observe_unrecordable() {
    with_store_mut(|store| {
        if let Some(store) = store {
            store.observations.incomplete = true;
        }
    });
}

/// Key for cached command results.
///
/// Environment values and the working directory are included so commands with
/// different host discovery contexts get distinct cache slots.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CommandKey {
    argv: Vec<String>,
    env: BTreeMap<String, String>,
    cwd: Option<PathBuf>,
    merge_stderr: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct CachedToolCommand {
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub merge_stderr: bool,
    pub output: String,
}

/// Key for a cached glob expansion.
///
/// The package anchors the patterns, and the exclude set changes which
/// subtrees the walk prunes, so both belong in the key alongside the
/// patterns themselves.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GlobKey {
    /// Which global produced this expansion. Two globals can read the same
    /// tree with different rules, so their answers must not be interchangeable.
    kind: &'static str,
    /// One engine can analyse targets from more than one workspace, where the
    /// same package name and patterns describe different files.
    workspace_root: PathBuf,
    package: String,
    patterns: Vec<String>,
    excludes: Vec<String>,
}

type HostEnvLookup = dyn Fn(&str) -> Option<String> + Send + Sync;

/// A lookup that is computed at most once for its key, however many callers ask
/// at once.
///
/// A plain map of answers is not enough when the callers run in parallel: they
/// all miss together, all compute, and all store the same answer. For a
/// `PATH` lookup that is waste; for spawning a discovery process or hashing a
/// compiler it is the difference between one and one-per-thread. The outer lock
/// only guards handing out the slot, so callers with different keys never wait
/// on each other, and callers with the same key wait for the first one's answer
/// instead of repeating its work.
struct SingleFlight<K, V> {
    slots: Mutex<BTreeMap<K, Arc<Mutex<Option<V>>>>>,
}

impl<K: Ord + Clone, V: Clone> SingleFlight<K, V> {
    fn new() -> Self {
        Self {
            slots: Mutex::new(BTreeMap::new()),
        }
    }

    fn get_or_compute<F>(&self, key: K, compute: F) -> Result<V>
    where
        F: FnOnce() -> Result<V>,
    {
        let slot = {
            let mut slots = self
                .slots
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(
                slots
                    .entry(key)
                    .or_insert_with(|| Arc::new(Mutex::new(None))),
            )
        };
        let mut answer = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(value) = answer.as_ref() {
            return Ok(value.clone());
        }
        // A failure is deliberately not remembered: it may be a transient
        // condition, and the next caller should find out for itself.
        let value = compute()?;
        *answer = Some(value.clone());
        Ok(value)
    }

    /// Every answer given so far, for callers that persist what was learned.
    fn answers(&self) -> Vec<(K, V)> {
        self.slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter_map(|(key, slot)| {
                let answer = slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                answer.as_ref().map(|value| (key.clone(), value.clone()))
            })
            .collect()
    }

    fn insert(&self, key: K, value: V) {
        self.slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, Arc::new(Mutex::new(Some(value))));
    }
}

pub(super) struct HostCache {
    which: Arc<SingleFlight<String, Option<String>>>,
    commands: Arc<SingleFlight<CommandKey, String>>,
    environment: Arc<Mutex<BTreeMap<String, Option<String>>>>,
    paths: Arc<Mutex<BTreeSet<PathBuf>>>,
    globs: Arc<SingleFlight<GlobKey, Arc<Vec<String>>>>,
    host_file_digests: Arc<SingleFlight<PathBuf, String>>,
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
            environment: Arc::clone(&self.environment),
            paths: Arc::clone(&self.paths),
            globs: Arc::clone(&self.globs),
            host_file_digests: Arc::clone(&self.host_file_digests),
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
            which: Arc::new(SingleFlight::new()),
            commands: Arc::new(SingleFlight::new()),
            environment: Arc::new(Mutex::new(BTreeMap::new())),
            paths: Arc::new(Mutex::new(BTreeSet::new())),
            globs: Arc::new(SingleFlight::new()),
            host_file_digests: Arc::new(SingleFlight::new()),
            host_env: Arc::new(host_env),
            tool_paths: Arc::new(BTreeMap::new()),
        }
    }

    pub(super) fn with_tool_paths(tool_paths: BTreeMap<String, String>) -> Self {
        Self::with_tool_cache(tool_paths, Vec::new())
    }

    pub(super) fn with_tool_cache(
        tool_paths: BTreeMap<String, String>,
        commands: Vec<CachedToolCommand>,
    ) -> Self {
        let cache = Self {
            tool_paths: Arc::new(tool_paths),
            ..Self::default()
        };
        for command in commands {
            cache.commands.insert(
                CommandKey {
                    argv: command.argv,
                    env: command.env,
                    cwd: command.cwd,
                    merge_stderr: command.merge_stderr,
                },
                command.output,
            );
        }
        cache
    }

    #[cfg(test)]
    pub(super) fn with_env(vars: BTreeMap<String, String>) -> Self {
        let vars = Arc::new(vars);
        Self::with_env_lookup(move |name| vars.get(name).cloned())
    }

    pub(super) fn env(&self, name: &str) -> Option<String> {
        let value = (self.host_env)(name);
        self.environment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name.to_string(), value.clone());
        value
    }

    pub(super) fn observed_environment(&self) -> BTreeMap<String, Option<String>> {
        self.environment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(super) fn observe_path(&self, path: &Path) {
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
        };
        self.paths
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(path);
    }

    /// Expand a glob through a per-run memo.
    ///
    /// Every target in a package globs the same source patterns, often
    /// several times across its compile, build-script, and data inputs, and
    /// one expansion walks the whole package tree. Repeating that walk is
    /// pure cost: a target's outputs land under its build directory inside
    /// `.once`, which the walk prunes, so nothing a build produces can enter
    /// a glob's answer partway through the run. Sibling targets are analysed
    /// concurrently, so an expansion that did observe a tree moving under it
    /// was already timing-dependent; memoising makes it settle on one answer
    /// instead. A tree that genuinely changes mid-build is caught where it
    /// should be, by the input digests and the action verification.
    pub(super) fn globs<F>(
        &self,
        kind: &'static str,
        workspace_root: &Path,
        package: &str,
        patterns: &[String],
        excludes: &[String],
        expand: F,
    ) -> Result<Arc<Vec<String>>>
    where
        F: FnOnce() -> Result<Vec<String>>,
    {
        let key = GlobKey {
            kind,
            workspace_root: workspace_root.to_path_buf(),
            package: package.to_string(),
            patterns: patterns.to_vec(),
            excludes: excludes.to_vec(),
        };
        // Sibling targets globbing different packages do not wait on each
        // other; siblings globbing the same one wait for a single walk.
        self.globs.get_or_compute(key, || Ok(Arc::new(expand()?)))
    }

    /// Hash a host file through a per-run memo.
    ///
    /// A toolchain binary named by several targets is the same file for all of
    /// them, and hashing something the size of a compiler once per target
    /// dominates graph loading.
    pub(super) fn host_file_digest<F>(&self, path: &Path, hash: F) -> Result<String>
    where
        F: FnOnce() -> Result<String>,
    {
        self.host_file_digests
            .get_or_compute(path.to_path_buf(), hash)
    }

    pub(super) fn observed_paths(&self) -> BTreeSet<PathBuf> {
        self.paths
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Resolve `name` on `PATH`, caching the result.
    ///
    /// The lock is released before the filesystem walk so concurrent
    /// `host_which` calls for different binaries don't serialise on
    /// each other.
    pub(super) fn which(&self, name: &str) -> Result<Option<String>> {
        if let Some(path) = self.tool_paths.get(name) {
            self.observe_path(Path::new(path));
            return Ok(Some(path.clone()));
        }
        let resolved = self.which.get_or_compute(name.to_string(), || {
            Ok(which_on_path(name).map(|path| path.display().to_string()))
        })?;
        // Recorded on every call, not only the first: the resolved path is part
        // of what each target observed, and a memoised answer must still leave
        // that trace.
        if let Some(path) = &resolved {
            self.observe_path(Path::new(path));
        }
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
        cwd: Option<&Path>,
        merge_stderr: bool,
    ) -> Result<String> {
        let key = CommandKey {
            argv: argv.to_vec(),
            env: env.clone(),
            cwd: cwd.map(Path::to_path_buf),
            merge_stderr,
        };
        self.commands
            .get_or_compute(key, || self.run_command(argv, env, cwd, merge_stderr))
    }

    /// Spawn one host command and return its captured output.
    fn run_command(
        &self,
        argv: &[String],
        env: &BTreeMap<String, String>,
        cwd: Option<&Path>,
        merge_stderr: bool,
    ) -> Result<String> {
        let mut iter = argv.iter();
        let program = iter
            .next()
            .ok_or_else(|| anyhow!("host_command requires a non-empty argv"))?;
        let command_args: Vec<&String> = iter.collect();
        let resolved_program = self.tool_paths.get(program).unwrap_or(program);
        if Path::new(resolved_program).components().count() > 1 {
            self.observe_path(Path::new(resolved_program));
        }
        let mut command = Command::new(resolved_program);
        command.args(&command_args);
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        for (key, value) in env {
            command.env(key, value);
        }
        let (status, stdout, stderr) = capture_host_command_output(&mut command)
            .with_context(|| format!("running `{program}`"))?;
        if !status.success() {
            let rendered_args = command_args
                .iter()
                .map(|arg| arg.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            return Err(anyhow!(
                "`{program} {}` exited with {}: {}",
                rendered_args,
                status,
                String::from_utf8_lossy(&stderr).trim()
            ));
        }
        // Some tools (kotlinc, older javac) print their version banner to
        // stderr. `merge_stderr` folds it in so version probes need no
        // host shell `2>&1`.
        let mut combined =
            String::from_utf8(stdout).map_err(|error| anyhow!("non-utf8 stdout: {error}"))?;
        if merge_stderr {
            let stderr =
                String::from_utf8(stderr).map_err(|error| anyhow!("non-utf8 stderr: {error}"))?;
            combined.push_str(&stderr);
        }
        Ok(combined)
    }

    /// A cheap description of the program a command name resolves to.
    ///
    /// Enough to tell one build of a compiler from another without reading it:
    /// size with the inode change time, which the operating system stamps on
    /// every write and no process can set.
    pub(super) fn program_identity(&self, program: &str) -> Option<String> {
        let resolved = self.tool_paths.get(program).cloned().or_else(|| {
            if Path::new(program).components().count() > 1 {
                Some(program.to_string())
            } else {
                self.which(program).ok().flatten()
            }
        })?;
        let metadata = std::fs::metadata(&resolved).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            Some(format!(
                "{resolved}:{}:{}.{}",
                metadata.len(),
                metadata.ctime(),
                metadata.ctime_nsec()
            ))
        }
        #[cfg(not(unix))]
        {
            let modified = metadata
                .modified()
                .ok()?
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap_or_default();
            Some(format!(
                "{resolved}:{}:{}.{}",
                metadata.len(),
                modified.as_secs(),
                modified.subsec_nanos()
            ))
        }
    }

    pub(super) fn cacheable_tool_commands(&self) -> Vec<CachedToolCommand> {
        self.commands
            .answers()
            .into_iter()
            .filter(|(key, _)| self.is_cacheable_tool_command(key))
            .map(|(key, output)| CachedToolCommand {
                argv: key.argv,
                env: key.env,
                cwd: key.cwd,
                merge_stderr: key.merge_stderr,
                output,
            })
            .collect()
    }

    fn is_cacheable_tool_command(&self, key: &CommandKey) -> bool {
        if !key.env.is_empty() || key.cwd.is_some() {
            return false;
        }
        let Some(program) = key.argv.first() else {
            return false;
        };
        if !self.tool_paths.contains_key(program)
            && !self.tool_paths.values().any(|path| path == program)
        {
            return false;
        }
        let args = key
            .argv
            .iter()
            .skip(1)
            .map(String::as_str)
            .collect::<Vec<_>>();
        matches!(
            args.as_slice(),
            ["--version" | "-version" | "version"] | ["--version", "--verbose"]
        )
    }
}

fn capture_host_command_output(command: &mut Command) -> Result<(ExitStatus, Vec<u8>, Vec<u8>)> {
    let stdout = tempfile::tempfile().context("creating host command stdout staging file")?;
    let stderr = tempfile::tempfile().context("creating host command stderr staging file")?;
    command.stdout(Stdio::from(stdout.try_clone()?));
    command.stderr(Stdio::from(stderr.try_clone()?));
    let status = command.status()?;
    let stdout = read_host_command_output(stdout)?;
    let stderr = read_host_command_output(stderr)?;
    Ok((status, stdout, stderr))
}

fn read_host_command_output(mut file: std::fs::File) -> Result<Vec<u8>> {
    let len = file.metadata()?.len();
    if len > HOST_COMMAND_OUTPUT_LIMIT {
        anyhow::bail!("host command output exceeds the 16 mebibyte analysis limit");
    }
    file.rewind()?;
    let mut bytes = Vec::with_capacity(
        usize::try_from(len).context("host command output exceeds addressable memory")?,
    );
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
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

#[cfg(test)]
mod single_flight_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use anyhow::anyhow;

    use super::SingleFlight;

    /// The property the whole thing exists for: many callers asking the same
    /// question at the same time produce one answer and one computation, not one
    /// each. Without it a graph analysed across eighteen threads spawns a
    /// discovery process eighteen times before any of them records the result.
    #[test]
    fn concurrent_callers_asking_the_same_thing_compute_it_once() {
        let flight = std::sync::Arc::new(SingleFlight::<String, usize>::new());
        let computations = std::sync::Arc::new(AtomicUsize::new(0));
        let start = std::sync::Arc::new(std::sync::Barrier::new(16));

        let threads = (0..16)
            .map(|_| {
                let flight = std::sync::Arc::clone(&flight);
                let computations = std::sync::Arc::clone(&computations);
                let start = std::sync::Arc::clone(&start);
                std::thread::spawn(move || {
                    start.wait();
                    flight
                        .get_or_compute("same".to_string(), || {
                            computations.fetch_add(1, Ordering::SeqCst);
                            Ok(7)
                        })
                        .unwrap()
                })
            })
            .collect::<Vec<_>>();
        let answers = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(answers, vec![7; 16]);
        assert_eq!(computations.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn different_keys_do_not_wait_on_each_other() {
        let flight = SingleFlight::<String, usize>::new();

        let first = flight.get_or_compute("a".to_string(), || {
            // Asking a different question from inside one computation would
            // deadlock if the outer map were held across the computation.
            flight.get_or_compute("b".to_string(), || Ok(2))
        });

        assert_eq!(first.unwrap(), 2);
        assert_eq!(
            flight.get_or_compute("b".to_string(), || Ok(99)).unwrap(),
            2
        );
    }

    /// A failure leaves no answer behind, so the next caller finds out for
    /// itself rather than inheriting a transient condition forever.
    #[test]
    fn a_failed_computation_is_not_remembered() {
        let flight = SingleFlight::<String, usize>::new();

        assert!(flight
            .get_or_compute("k".to_string(), || Err(anyhow!("no")))
            .is_err());

        assert_eq!(flight.get_or_compute("k".to_string(), || Ok(5)).unwrap(), 5);
    }
}
