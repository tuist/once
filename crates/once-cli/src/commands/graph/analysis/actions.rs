use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use fd_lock::RwLock;
use once_cas::{ActionResult, CacheProvider, Digest};
use once_core::{
    validate_action_contract_with_options, workspace_mise_command, workspace_prepared_tool_env,
    workspace_tool_env_with_executables, Action, ActionContractOptions, ActionOutputObserver,
    ArchiveEntry, ArchiveEntryKind, ArchiveFormat, CopyPathMode, EvidenceCacheState,
    EvidenceRecord, EvidenceSubject, FilesystemOperation, InputDigestBuilder,
    InputFingerprintComponent, InputFingerprintManifest, NetworkPolicy, OutputSymlinkMode,
    PreparePathMode, ResourcePool, ResourceRequest, SandboxMode, WorkspacePath,
};
use once_frontend::analysis::{
    AnalysisResult, DeclaredAction, DeclaredActionOperation, DeclaredArchiveEntryKind,
    DeclaredArchiveFormat, DeclaredArgFile, DeclaredArgFileFormat, DeclaredCopyPathMode,
    DeclaredPreparePathMode,
};
use once_frontend::GraphTarget;
use serde::Serialize;

use super::output_state;
use super::source_digest_cache::SourceDigestCache;
use super::{AvailableInput, BuildOutcome, PerActionOutcome};

pub(super) mod legacy_outputs;
mod pending_outputs;
mod preparation;
mod recovery;
mod runner;

use self::legacy_outputs::capture_step;
mod scheduling;
mod uncached;
mod uncached_process;
mod worker_lease;

use uncached::run_uncached_action_admitted;
#[cfg(test)]
use uncached::{capture_uncached_outputs, capture_uncached_outputs_admitted, run_uncached_action};

pub(super) use self::recovery::{Producers, Repair};
pub(super) use self::runner::run_declared_actions_with;
pub(super) use self::worker_lease::WorkerLease;

const FAILURE_OUTPUT_LIMIT: usize = 16 * 1024;
const EVIDENCE_FLUSH_BATCH_SIZE: usize = 128;

struct DeclaredActionRun<'a> {
    workspace: &'a Path,
    cache: &'a CacheProvider,
    module_source_digest: Digest,
    target_id: &'a str,
    capability: &'a str,
    index: usize,
    declared: DeclaredAction,
    input_action_digests: &'a [(String, Digest)],
    available_inputs: &'a BTreeMap<String, AvailableInput>,
    source_digest_cache: Option<&'a SourceDigestCache>,
    prior_cached_results: PendingResults<'a>,
    record_success_evidence: bool,
    sandbox: SandboxMode,
    resources: &'a Arc<ResourcePool>,
    output_observer: Option<&'a dyn ActionOutputObserver>,
    producers: &'a Producers,
    worker: Option<&'a WorkerLease<'a>>,
}

struct DeclaredActionOutcome {
    digest: Digest,
    input_digest: Option<Digest>,
    input_fingerprint: InputFingerprintManifest,
    cache_state: EvidenceCacheState,
    result: ActionResult,
    evidence_record: Option<EvidenceRecord>,
    restored_outputs: pending_outputs::Restoration,
    /// Wall time spent hashing inputs and building the action digest
    /// (the "prepare" phase). Rendered as
    /// a distinct sub-span so cache-hit runs still show what
    /// consumed the wall clock instead of a 1 ms dash.
    prepare_ms: i64,
    /// Wall time spent inside cache probe + (on a miss) command
    /// execution. On a hit this is dominated by the remote `GetActionResult`
    /// round-trip; on a miss it's the command itself plus output upload.
    execute_ms: i64,
}

struct DeclaredActionsState {
    action_digests: Vec<Digest>,
    prior_actions_digest: Option<Digest>,
    input_digests: Vec<Digest>,
    input_fingerprints: Vec<InputFingerprintManifest>,
    available_inputs: BTreeMap<String, AvailableInput>,
    aggregate_cache_state: Option<EvidenceCacheState>,
    outputs: Vec<String>,
    result: ActionResult,
    cached_results: Vec<ActionResult>,
    /// Cache hits whose outputs are intermediate: restored only before an
    /// action that may read them without declaring them runs.
    pending_intermediate: Vec<ActionResult>,
    intermediate_outputs: BTreeSet<String>,
    evidence_records: Vec<EvidenceRecord>,
    per_action_outcomes: Vec<PerActionOutcome>,
}

impl DeclaredActionsState {
    fn new(available_inputs: &BTreeMap<String, AvailableInput>) -> Self {
        Self {
            action_digests: Vec::new(),
            prior_actions_digest: None,
            input_digests: Vec::new(),
            input_fingerprints: Vec::new(),
            available_inputs: available_inputs.clone(),
            aggregate_cache_state: None,
            outputs: Vec::new(),
            result: ActionResult {
                exit_code: 0,
                stdout: None,
                stderr: None,
                outputs: BTreeMap::new(),
            },
            cached_results: Vec::new(),
            pending_intermediate: Vec::new(),
            intermediate_outputs: BTreeSet::new(),
            evidence_records: Vec::new(),
            per_action_outcomes: Vec::new(),
        }
    }

    fn input_action_digests(
        &self,
        declared: &DeclaredAction,
        _dependency_digests: &[(String, Digest)],
    ) -> Vec<(String, Digest)> {
        let mut input_digests = Vec::new();
        if declared.depends_on_prior_actions {
            if let Some(digest) = self.prior_actions_digest {
                input_digests.push(("same-target:prior-actions".to_string(), digest));
            }
        }
        input_digests
    }

    fn record(
        &mut self,
        outcome: DeclaredActionOutcome,
        streams: bool,
        per_action: Option<PerActionOutcome>,
        policy: &CachedResultPolicy,
    ) {
        pending_outputs::mark_restored(self, &outcome.restored_outputs);
        self.per_action_outcomes.extend(per_action);
        self.prior_actions_digest = Some(extend_prior_actions_digest(
            self.prior_actions_digest,
            self.action_digests.len(),
            outcome.digest,
        ));
        self.action_digests.push(outcome.digest);
        output_state::prune_replaced_descendants(
            &mut self.available_inputs,
            &outcome.result.outputs,
        );
        output_state::prune_replaced_descendants(&mut self.result.outputs, &outcome.result.outputs);
        self.available_inputs
            .extend(outcome.result.outputs.iter().map(|(path, digest)| {
                (
                    path.clone(),
                    AvailableInput {
                        blob_digest: *digest,
                        same_target: true,
                        materialized: outcome.cache_state != EvidenceCacheState::Hit,
                    },
                )
            }));
        self.input_digests.extend(outcome.input_digest);
        self.aggregate_cache_state = Some(
            self.aggregate_cache_state
                .map_or(outcome.cache_state, |current| {
                    aggregate_declared_action_cache_state(current, outcome.cache_state)
                }),
        );
        if streams {
            self.result.stdout = outcome.result.stdout;
            self.result.stderr = outcome.result.stderr;
        }
        self.result.outputs.extend(
            outcome
                .result
                .outputs
                .iter()
                .map(|(path, digest)| (path.clone(), *digest)),
        );
        for path in outcome.result.outputs.keys() {
            if policy.intermediate {
                self.intermediate_outputs.insert(path.clone());
            } else {
                self.intermediate_outputs.remove(path);
            }
        }
        if outcome.cache_state != EvidenceCacheState::Hit {
            output_state::forget_replaced_materializations(
                &mut self.available_inputs,
                &policy.touched,
                &outcome.result.outputs,
            );
        }
        track_cached_result(&mut self.cached_results, &outcome, policy, false);
        track_cached_result(&mut self.pending_intermediate, &outcome, policy, true);
        self.input_fingerprints.push(outcome.input_fingerprint);
        self.evidence_records.extend(outcome.evidence_record);
    }

    /// The pending results the scheduler restores to disk before running an
    /// action under `policy`.
    pub(super) fn pending(&self) -> PendingResults<'_> {
        PendingResults {
            restored_at_end: &self.cached_results,
            intermediate: &self.pending_intermediate,
        }
    }

    fn take_evidence_records(&mut self) -> Vec<EvidenceRecord> {
        std::mem::take(&mut self.evidence_records)
    }

    fn finish(mut self, target_id: &str, provider: serde_json::Value) -> BuildOutcome {
        deduplicate_outputs(&mut self.outputs);
        let cache_state = self
            .aggregate_cache_state
            .unwrap_or(EvidenceCacheState::Hit);
        let input_digest = compose_target_input_digest(&self.input_digests);
        let input_fingerprint =
            compose_target_input_fingerprint(input_digest, self.input_fingerprints);
        BuildOutcome {
            provider: Arc::new(provider),
            action_digest: compose_target_action_digest(target_id, &self.action_digests),
            input_digest,
            input_fingerprint,
            available_inputs: self.available_inputs,
            outputs: self.outputs,
            cache_tag: cache_state.as_str(),
            cache_state,
            result: self.result,
            cached_results: self.cached_results,
            intermediate_outputs: self.intermediate_outputs,
            per_action_outcomes: self.per_action_outcomes,
        }
    }
}

struct DeclaredActionContext<'a> {
    workspace: &'a Path,
    cache: &'a CacheProvider,
    source_digest_cache: Option<&'a SourceDigestCache>,
    available_inputs: &'a BTreeMap<String, AvailableInput>,
    prior_cached_results: PendingResults<'a>,
    target_id: &'a str,
    capability: &'a str,
    index: usize,
    identifier: &'a str,
    argv: &'a [String],
    arg_files: &'a [DeclaredArgFile],
    input_fingerprint: InputFingerprintManifest,
    record_success_evidence: bool,
    resources: &'a Arc<ResourcePool>,
    output_observer: Option<&'a dyn ActionOutputObserver>,
    producers: &'a Producers,
    worker: Option<&'a WorkerLease<'a>>,
}

impl DeclaredActionContext<'_> {
    fn repair(&self) -> Repair<'_> {
        Repair {
            workspace: self.workspace,
            cache: self.cache,
            source_digest_cache: self.source_digest_cache,
            resources: self.resources,
            producers: self.producers,
        }
    }

    /// Take a worker lane once the action is admitted, so a lane is never
    /// held by an action still queued for capacity or for its lock.
    async fn claim_worker(&self) {
        if let Some(worker) = self.worker {
            worker.claim().await;
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
struct ActionExecutionFailure {
    message: String,
    exit_code: i32,
    was_cached: bool,
}

struct DeclaredActionFailure<'a> {
    cache: &'a CacheProvider,
    identifier: &'a str,
    index: usize,
    target: &'a str,
    exit_code: i32,
    argv: &'a [String],
    arg_files: &'a [DeclaredArgFile],
    result: &'a ActionResult,
}

#[derive(Debug, Serialize)]
pub(crate) struct DeclaredActionValidation {
    pub(crate) index: usize,
    pub(crate) identifier: String,
    pub(crate) valid: bool,
    pub(crate) exit_code: i32,
    pub(crate) diagnostics: Vec<once_frontend::Diagnostic>,
    pub(crate) limitations: Vec<String>,
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) async fn validate_declared_actions(
    workspace: &Path,
    cache: &CacheProvider,
    module_source_digest: Digest,
    target: &GraphTarget,
    analysis: AnalysisResult,
    _dep_action_digests: &[(String, Digest)],
    selected_index: Option<usize>,
) -> Result<Vec<DeclaredActionValidation>> {
    let mut actions = analysis.actions;
    expose_target_tools(workspace, target, &mut actions, None).await?;
    if let Some(index) = selected_index {
        if index >= actions.len() {
            anyhow::bail!(
                "action index {index} is out of range for target `{}` with {} actions",
                target.label.id,
                actions.len()
            );
        }
    }

    let mut action_digests = Vec::new();
    let mut prior_actions_digest = None;
    let mut validations = Vec::new();
    for (index, declared) in actions.into_iter().enumerate() {
        let is_selected = selected_index.is_none_or(|selected| selected == index);
        if matches!(
            declared.operation,
            Some(DeclaredActionOperation::ExpandActions { .. })
        ) {
            if !is_selected {
                continue;
            }
            validations.push(DeclaredActionValidation {
                index,
                identifier: declared.identifier.unwrap_or_default(),
                valid: true,
                exit_code: 0,
                diagnostics: Vec::new(),
                limitations: vec![
                    "Deferred actions require their planning inputs to be built first.".to_string(),
                ],
            });
            continue;
        }
        let mut input_action_digests = Vec::new();
        if declared.depends_on_prior_actions {
            if let Some(digest) = prior_actions_digest {
                input_action_digests.push(("same-target:prior-actions".to_string(), digest));
            }
        }
        materialize_declared_arg_files(workspace, &declared.arg_files).with_context(|| {
            format!(
                "writing argument files for action {index} for {}",
                target.label.id
            )
        })?;
        // An isolated contract probe never runs prior actions or dependency
        // targets, so any input those steps would have produced is absent from
        // the workspace. Such an action cannot be hashed or staged here; record
        // it as skipped with a limitation instead of aborting the whole command.
        let missing_inputs = missing_declared_inputs(workspace, &declared);
        if !missing_inputs.is_empty() {
            if is_selected {
                let identifier = declared
                    .identifier
                    .clone()
                    .unwrap_or_else(|| format!("action-{index}"));
                validations.push(DeclaredActionValidation {
                    index,
                    identifier,
                    valid: true,
                    exit_code: 0,
                    diagnostics: Vec::new(),
                    limitations: vec![format!(
                        "action not validated in isolation: consumes inputs produced by prior actions or dependencies ({})",
                        missing_inputs.join(", ")
                    )],
                });
            }
            continue;
        }
        let action = declared_to_action(
            workspace,
            &declared,
            module_source_digest,
            &input_action_digests,
            SandboxMode::Inputs,
        )?;
        prior_actions_digest = Some(extend_prior_actions_digest(
            prior_actions_digest,
            action_digests.len(),
            action.digest(),
        ));
        action_digests.push(action.digest());
        if !is_selected {
            continue;
        }

        let create_dirs = declared
            .create_dirs
            .iter()
            .map(|path| workspace_path(path, "create_dirs entry"))
            .collect::<Result<Vec<_>>>()?;
        let report = validate_action_contract_with_options(
            &action,
            workspace,
            cache,
            &ActionContractOptions { create_dirs },
        )
        .await
        .with_context(|| format!("validating action {index} for target `{}`", target.label.id))?;
        let identifier = declared
            .identifier
            .clone()
            .unwrap_or_else(|| format!("action-{index}"));
        let mut diagnostics = report
            .diagnostics
            .into_iter()
            .map(|diagnostic| {
                let attribute = match diagnostic.operation {
                    FilesystemOperation::Read
                    | FilesystemOperation::Modify
                    | FilesystemOperation::Delete
                    | FilesystemOperation::Access => "inputs",
                    FilesystemOperation::Write => "outputs",
                };
                let mut converted = once_frontend::Diagnostic::new(
                    diagnostic.code,
                    format!("action `{identifier}` ({index}): {}", diagnostic.message),
                )
                .with_target(target.label.id.clone())
                .with_attribute(attribute);
                for repair in diagnostic.repairs {
                    converted = converted.with_repair(repair);
                }
                converted
            })
            .collect::<Vec<_>>();
        if report.exit_code != 0 && diagnostics.is_empty() {
            diagnostics.push(
                once_frontend::Diagnostic::new(
                    "action_execution_failed",
                    format!(
                        "action `{identifier}` ({index}) exited with code {} before its filesystem contract could be validated",
                        report.exit_code
                    ),
                )
                .with_target(target.label.id.clone())
                .with_repair(
                    "Inspect the action output, repair the command failure, and validate again",
                ),
            );
        }
        validations.push(DeclaredActionValidation {
            index,
            identifier,
            valid: report.valid,
            exit_code: report.exit_code,
            diagnostics,
            limitations: report.limitations,
        });
    }
    Ok(validations)
}

#[cfg(test)]
pub(super) use runner::run_declared_actions;

fn extend_prior_actions_digest(
    prior: Option<Digest>,
    index: usize,
    action_digest: Digest,
) -> Digest {
    let mut builder = InputDigestBuilder::new(b"once.prior_actions.v1\0");
    if let Some(prior) = prior {
        builder.push_keyed(b"prior", &prior);
    }
    builder.push_keyed(format!("action:{index}").as_bytes(), &action_digest);
    builder.finish()
}

/// How an action's result affects the cached outputs still waiting to be put on
/// disk when the target finishes.
pub(super) struct CachedResultPolicy {
    /// The action's outputs are only read by later actions of the target.
    pub(super) intermediate: bool,
    /// The action restored every waiting output before it ran, if it ran.
    pub(super) restores_prior: bool,
    /// Paths the action writes, cleans, removes, or creates when it runs.
    pub(super) touched: Vec<TouchedPath>,
}

/// Cache hits whose outputs aren't on disk yet.
#[derive(Clone, Copy, Default)]
pub(super) struct PendingResults<'a> {
    /// Restored when the target finishes.
    pub(super) restored_at_end: &'a [ActionResult],
    /// Intermediate outputs, restored only before an action that may read
    /// them without declaring them.
    pub(super) intermediate: &'a [ActionResult],
}

impl<'a> PendingResults<'a> {
    fn all(self) -> impl DoubleEndedIterator<Item = &'a ActionResult> {
        self.restored_at_end.iter().chain(self.intermediate)
    }
}

/// A path an action changes when it runs.
pub(super) enum TouchedPath {
    /// Written, cleaned, or removed: whatever was at or under it is replaced.
    Replaced(String),
    /// Created as a directory: what is already under it stays.
    Created(String),
}

impl TouchedPath {
    /// Whether restoring `pending` after the action ran would undo some of
    /// what the action did.
    fn conflicts_with(&self, pending: &str) -> bool {
        match self {
            Self::Replaced(path) => paths_overlap(pending, path),
            Self::Created(path) => pending == path || contains(pending, path),
        }
    }
}

impl CachedResultPolicy {
    pub(super) fn for_action(declared: &DeclaredAction, batch_size: usize) -> Self {
        Self {
            intermediate: declared.intermediate,
            restores_prior: batch_size == 1 && declared.depends_on_prior_actions,
            touched: touched_paths(declared),
        }
    }
}

/// Paths an action writes, cleans, removes, or creates when it runs.
fn touched_paths(declared: &DeclaredAction) -> Vec<TouchedPath> {
    let mut paths = declared
        .outputs
        .iter()
        .chain(&declared.clean_paths)
        .chain(&declared.stdout)
        .chain(&declared.stderr)
        .chain(declared.arg_files.iter().map(|file| &file.path))
        .cloned()
        .map(TouchedPath::Replaced)
        .chain(
            declared
                .create_dirs
                .iter()
                .cloned()
                .map(TouchedPath::Created),
        )
        .collect::<Vec<_>>();
    match &declared.operation {
        Some(DeclaredActionOperation::PreparePath {
            path,
            mode: DeclaredPreparePathMode::Remove,
        }) => paths.push(TouchedPath::Replaced(path.clone())),
        Some(DeclaredActionOperation::PreparePath {
            path,
            mode: DeclaredPreparePathMode::Directory,
        }) => paths.push(TouchedPath::Created(path.clone())),
        _ => {}
    }
    paths
}

/// Whether `outer` is a directory above `inner`.
fn contains(outer: &str, inner: &str) -> bool {
    inner
        .strip_prefix(outer)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether one path is the other or contains it.
fn paths_overlap(left: &str, right: &str) -> bool {
    left == right || contains(left, right) || contains(right, left)
}

/// The waiting outputs that would undo part of what the action does if they
/// were restored after it ran.
fn overlapping_results<'a>(
    results: impl Iterator<Item = &'a ActionResult>,
    touched: &[TouchedPath],
) -> Vec<ActionResult> {
    results
        .filter_map(|result| {
            let outputs = result
                .outputs
                .iter()
                .filter(|(path, _)| touched.iter().any(|touched| touched.conflicts_with(path)))
                .map(|(path, digest)| (path.clone(), *digest))
                .collect::<BTreeMap<_, _>>();
            (!outputs.is_empty()).then_some(ActionResult {
                exit_code: 0,
                stdout: None,
                stderr: None,
                outputs,
            })
        })
        .collect()
}

/// Keep one list of pending cache hits current after `outcome`: the outputs
/// restored when the target finishes, or, with `intermediate`, the ones
/// restored only for an action that may read them undeclared.
fn track_cached_result(
    cached_results: &mut Vec<ActionResult>,
    outcome: &DeclaredActionOutcome,
    policy: &CachedResultPolicy,
    intermediate: bool,
) {
    if outcome.cache_state != EvidenceCacheState::Hit {
        if policy.restores_prior {
            cached_results.clear();
            return;
        }
        // The action restored the waiting outputs it overlaps before it ran
        // and then changed them, so they must not be restored again over
        // what it left.
        for result in cached_results.iter_mut() {
            result.outputs.retain(|path, _| {
                !policy
                    .touched
                    .iter()
                    .any(|touched| touched.conflicts_with(path))
            });
        }
    }
    // A later output replaces an earlier one at the same path, so restoring
    // the earlier result when the target finishes would put stale bytes back.
    for result in cached_results.iter_mut() {
        output_state::prune_replaced_descendants(&mut result.outputs, &outcome.result.outputs);
        result
            .outputs
            .retain(|path, _| !outcome.result.outputs.contains_key(path));
    }
    cached_results.retain(|result| !result.outputs.is_empty());
    if outcome.cache_state == EvidenceCacheState::Hit
        && policy.intermediate == intermediate
        && !outcome.result.outputs.is_empty()
    {
        cached_results.push(outcome.result.clone());
    }
}

fn deduplicate_outputs(outputs: &mut Vec<String>) {
    outputs.sort();
    outputs.dedup();
}

async fn expose_target_tools(
    workspace: &Path,
    target: &GraphTarget,
    actions: &mut [DeclaredAction],
    resolved_paths: Option<&BTreeMap<String, String>>,
) -> Result<()> {
    let tools = target
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<BTreeSet<_>>();
    if tools.is_empty() {
        return Ok(());
    }
    let executables = target
        .tools
        .iter()
        .flat_map(|tool| tool.executables.iter().map(String::as_str))
        .collect::<BTreeSet<_>>();
    let Some(prefix) = workspace_mise_command(workspace)
        .await
        .context("building graph tool execution command")?
    else {
        return Ok(());
    };
    let tools = tools.into_iter().collect::<Vec<_>>();
    let executables = executables.into_iter().collect::<Vec<_>>();
    let tool_env = if let Some(resolved_paths) = resolved_paths {
        let paths = executables
            .iter()
            .filter_map(|executable| {
                resolved_paths
                    .get(*executable)
                    .map(PathBuf::from)
                    .or_else(|| host_executable_path(executable))
            })
            .collect::<Vec<_>>();
        workspace_prepared_tool_env(workspace, &tools, &paths)
            .context("building prepared graph tool execution environment")?
    } else {
        workspace_tool_env_with_executables(workspace, &tools, &executables, &[])
            .await
            .context("building graph tool execution environment")?
    };
    apply_tool_execution(&prefix, &tool_env, actions);
    Ok(())
}

pub(super) fn host_executable_path(executable: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    let path_ext = env::var_os("PATHEXT");
    host_executable_path_in(executable, &paths, path_ext.as_deref())
}

fn host_executable_path_in(
    executable: &str,
    paths: &OsStr,
    path_ext: Option<&OsStr>,
) -> Option<PathBuf> {
    let candidates = if cfg!(windows) && Path::new(executable).extension().is_none() {
        let extensions = path_ext
            .and_then(OsStr::to_str)
            .unwrap_or(".COM;.EXE;.BAT;.CMD");
        extensions
            .split(';')
            .map(str::trim)
            .filter(|extension| !extension.is_empty())
            .flat_map(|extension| {
                [
                    format!("{executable}{extension}"),
                    format!("{executable}{}", extension.to_ascii_lowercase()),
                ]
            })
            .collect::<Vec<_>>()
    } else {
        vec![executable.to_string()]
    };
    env::split_paths(paths)
        .flat_map(|directory| {
            candidates
                .iter()
                .map(move |candidate| directory.join(candidate))
        })
        .find(|candidate| candidate.is_file() && !once_core::is_mise_shim(candidate))
}

fn apply_tool_execution(
    prefix: &[String],
    tool_env: &BTreeMap<String, String>,
    actions: &mut [DeclaredAction],
) {
    for action in actions {
        if action.operation.is_some() {
            continue;
        }
        let mut argv = prefix.to_vec();
        argv.append(&mut action.argv);
        action.argv = argv;
        for (key, value) in tool_env {
            // The tool environment now contributes a curated PATH. A target
            // kind may already have set a PATH the action needs at execution
            // time, such as the Windows rustc runtime and proc-macro directories
            // that let rustc load its DLLs, or a JDK bin directory. Merge the two
            // so those entries survive instead of being overwritten, keeping the
            // action's own directories ahead of the mise-derived ones.
            if key == "PATH" {
                if let Some(existing) = action.env.get(key) {
                    let merged = merge_paths(existing, value);
                    action.env.insert(key.clone(), merged);
                    continue;
                }
            }
            action.env.insert(key.clone(), value.clone());
        }
    }
}

/// Merge two `PATH` values, keeping `primary` entries first and appending the
/// entries from `secondary` that are not already present. This preserves the
/// directories a target kind places on an action's PATH when the tool
/// environment contributes its own PATH.
fn merge_paths(primary: &str, secondary: &str) -> String {
    if primary.is_empty() {
        return secondary.to_string();
    }
    if secondary.is_empty() {
        return primary.to_string();
    }
    let mut seen = BTreeSet::new();
    let mut ordered = Vec::new();
    for entry in env::split_paths(primary).chain(env::split_paths(secondary)) {
        if seen.insert(entry.clone()) {
            ordered.push(entry);
        }
    }
    match env::join_paths(ordered) {
        Ok(joined) => joined.to_string_lossy().into_owned(),
        // join_paths only fails when an entry contains the platform path
        // separator, which a real PATH directory cannot. Keep the action's own
        // PATH rather than dropping it.
        Err(_) => primary.to_string(),
    }
}

fn aggregate_declared_action_cache_state(
    current: EvidenceCacheState,
    next: EvidenceCacheState,
) -> EvidenceCacheState {
    match (current, next) {
        // An uncacheable action (Bypass) makes the whole target uncacheable:
        // a target that must re-run any action cannot be reported as a
        // reused cache hit, so Bypass dominates the aggregate.
        (EvidenceCacheState::Bypass, _) | (_, EvidenceCacheState::Bypass) => {
            EvidenceCacheState::Bypass
        }
        (EvidenceCacheState::Miss, _) | (_, EvidenceCacheState::Miss) => EvidenceCacheState::Miss,
        (EvidenceCacheState::Hit, EvidenceCacheState::Hit) => EvidenceCacheState::Hit,
    }
}

#[allow(clippy::too_many_lines)]
async fn run_declared_action(run: DeclaredActionRun<'_>) -> Result<DeclaredActionOutcome> {
    let DeclaredActionRun {
        workspace,
        cache,
        module_source_digest,
        target_id,
        capability,
        index,
        declared,
        input_action_digests,
        available_inputs,
        source_digest_cache,
        prior_cached_results,
        record_success_evidence,
        sandbox,
        resources,
        output_observer,
        producers,
        worker,
    } = run;
    let identifier_for_error = declared
        .identifier
        .clone()
        .unwrap_or_else(|| "<anonymous>".to_string());
    if let Some(observer) = output_observer {
        let message = format!("\n$ {target_id} ({identifier_for_error})\n");
        observer.observe(once_core::ActionOutputStream::Stdout, message.as_bytes());
    }
    let cacheable = declared.cacheable;
    tracing::trace!(
        target = %target_id,
        action_index = index,
        identifier = %identifier_for_error,
        cacheable,
        inputs = declared.inputs.len(),
        outputs = declared.outputs.len(),
        "preparing declared graph action"
    );
    let prepare_started = std::time::Instant::now();
    let preparation = producers
        .executors(resources)?
        .acquire_materialization()
        .await;
    if let Some(inputs) = unmaterialized_inputs_for(&declared, available_inputs) {
        producers
            .verify_fingerprints(workspace, &inputs, &preparation)
            .await
            .with_context(|| {
                format!(
                    "validating inputs for action {index} for {target_id} ({identifier_for_error})"
                )
            })?;
    }
    let prepared = prepare_declared_action_with_inputs(
        workspace,
        &declared,
        module_source_digest,
        input_action_digests,
        available_inputs,
        source_digest_cache,
        sandbox,
    )
    .with_context(|| format!("building action {index} for {target_id} ({identifier_for_error})"))?;
    drop(preparation);
    let prepare_ms = i64::try_from(prepare_started.elapsed().as_millis()).unwrap_or(i64::MAX);
    let action = prepared.action;
    let context = DeclaredActionContext {
        workspace,
        cache,
        source_digest_cache,
        available_inputs,
        prior_cached_results,
        target_id,
        capability,
        index,
        identifier: &identifier_for_error,
        argv: &declared.argv,
        arg_files: &declared.arg_files,
        input_fingerprint: prepared.input_fingerprint,
        record_success_evidence,
        resources,
        output_observer,
        producers,
        worker,
    };

    let execute_started = std::time::Instant::now();
    let outcome = if cacheable {
        run_cacheable_declared_action(context, action, &declared).await
    } else {
        let restored_outputs = preparation::restore(&context, &declared).await?;
        run_uncacheable_declared_action(context, action, &declared)
            .await
            .map(|mut outcome| {
                outcome.restored_outputs = restored_outputs;
                outcome
            })
    };
    let execute_ms = i64::try_from(execute_started.elapsed().as_millis()).unwrap_or(i64::MAX);
    outcome.map(|mut o| {
        o.prepare_ms = prepare_ms;
        o.execute_ms = execute_ms;
        o
    })
}

#[cfg(test)]
async fn prepare_declared_command_paths(workspace: &Path, declared: &DeclaredAction) -> Result<()> {
    if declared.operation.is_some() {
        return Ok(());
    }
    prepare_command_setup(workspace, &declared.clean_paths, &declared.create_dirs).await
}

#[cfg(test)]
async fn prepare_command_setup(
    workspace: &Path,
    clean_paths: &[String],
    create_dirs: &[String],
) -> Result<()> {
    for path in clean_paths {
        let path = workspace_path(path, "clean_paths entry")?;
        remove_declared_path_if_exists(&path.resolve(workspace), path.as_str()).await?;
    }
    for path in create_dirs {
        let path = workspace_path(path, "create_dirs entry")?;
        tokio::fs::create_dir_all(path.resolve(workspace))
            .await
            .with_context(|| format!("creating declared command directory `{}`", path.as_str()))?;
    }
    Ok(())
}

async fn prepare_command_setup_admitted(
    workspace: &Path,
    cache: &CacheProvider,
    clean_paths: &[String],
    create_dirs: &[String],
    lease: &once_core::ExecutionPermit,
) -> Result<()> {
    let paths = clean_paths
        .iter()
        .map(|path| (path, once_core::PreparePathMode::Remove))
        .chain(
            create_dirs
                .iter()
                .map(|path| (path, once_core::PreparePathMode::Directory)),
        );
    for (path, mode) in paths {
        let action = Action::PreparePath {
            path: workspace_path(path, "command preparation path")?,
            mode,
            input_digest: None,
        };
        once_core::run_uncached_admitted(&action, workspace, cache, false, None, lease.clone())
            .await?;
    }
    Ok(())
}

#[cfg(test)]
async fn remove_declared_path_if_exists(abs: &Path, label: &str) -> Result<()> {
    let metadata = match tokio::fs::symlink_metadata(abs).await {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(source).with_context(|| format!("reading declared command path `{label}`"));
        }
    };
    let result = if metadata.is_dir() && !metadata.file_type().is_symlink() {
        tokio::fs::remove_dir_all(abs).await
    } else {
        tokio::fs::remove_file(abs).await
    };
    result.with_context(|| format!("removing declared command path `{label}`"))
}

async fn run_cacheable_declared_action(
    context: DeclaredActionContext<'_>,
    action: Action,
    declared: &DeclaredAction,
) -> Result<DeclaredActionOutcome> {
    let (outcome, restored_outputs) =
        resolve_cacheable_declared_action(&context, &action, declared).await?;
    let exit_code = outcome.result.exit_code;
    if exit_code != 0 {
        record_declared_action_evidence(
            &context,
            &action,
            outcome.action,
            EvidenceCacheState::from(outcome.cache),
            &outcome.result,
        )
        .await;
        return Err(ActionExecutionFailure {
            message: declared_action_failure_message(DeclaredActionFailure {
                cache: context.cache,
                identifier: context.identifier,
                index: context.index,
                target: context.target_id,
                exit_code,
                argv: context.argv,
                arg_files: context.arg_files,
                result: &outcome.result,
            })
            .await,
            exit_code,
            was_cached: matches!(outcome.cache, once_core::CacheState::Hit),
        }
        .into());
    }
    record_producer(&context, &action, declared, &outcome.result);
    let cache_tag = crate::commands::util::cache_tag(outcome.cache);
    let cache_state = EvidenceCacheState::from(outcome.cache);
    tracing::debug!(
        target = %context.target_id,
        action_index = context.index,
        identifier = %context.identifier,
        cache = cache_tag,
        action_digest = %outcome.action,
        "completed cacheable declared graph action"
    );
    let evidence_record = context
        .record_success_evidence
        .then(|| {
            declared_action_evidence_record(
                context.target_id,
                context.capability,
                &action,
                outcome.action,
                &context.input_fingerprint,
                cache_state,
                &outcome.result,
            )
        })
        .flatten();
    Ok(DeclaredActionOutcome {
        digest: outcome.action,
        input_digest: action.input_digest(),
        input_fingerprint: context.input_fingerprint,
        cache_state,
        result: outcome.result,
        evidence_record,
        restored_outputs,
        // The outer `run_declared_action` fills these in after
        // observing prepare + execute spans from the outside.
        prepare_ms: 0,
        execute_ms: 0,
    })
}

/// Remember how this action produced its outputs, so a later restore that
/// finds their bytes gone can run it again.
fn record_producer(
    context: &DeclaredActionContext<'_>,
    action: &Action,
    declared: &DeclaredAction,
    result: &ActionResult,
) {
    if result.outputs.is_empty() {
        return;
    }
    let mut inputs = prior_results_for(context.prior_cached_results, declared)
        .into_iter()
        .flat_map(|result| result.outputs)
        .collect::<BTreeMap<_, _>>();
    inputs.extend(
        declared
            .inputs
            .iter()
            .filter_map(|input| enclosing_available_output(context.available_inputs, input))
            .map(|(path, input)| (path.clone(), input.blob_digest)),
    );
    if declared.depends_on_prior_actions {
        inputs.extend(
            context
                .available_inputs
                .iter()
                .filter(|(_, input)| input.same_target)
                .map(|(path, input)| (path.clone(), input.blob_digest)),
        );
    }
    inputs.retain(|path, _| !result.outputs.contains_key(path));
    context.producers.record(recovery::Producer {
        target_id: context.target_id.to_string(),
        capability: context.capability.to_string(),
        index: context.index,
        identifier: context.identifier.to_string(),
        report: recovery::ProducerReport {
            reported: !declared
                .operation
                .as_ref()
                .is_some_and(DeclaredActionOperation::is_bookkeeping),
            identifier: declared.identifier.clone(),
            display_name: declared.display_name.clone(),
            source_files: declared.source_files.clone(),
            presentation: declared.presentation.clone(),
            history: declared.history.clone(),
        },
        action: action.clone(),
        setup: declared
            .operation
            .is_none()
            .then(|| recovery::CommandSetup {
                arg_files: declared.arg_files.clone(),
                clean_paths: declared.clean_paths.clone(),
                create_dirs: declared.create_dirs.clone(),
            }),
        inputs,
        outputs: result.outputs.clone(),
    });
}

async fn resolve_cacheable_declared_action(
    context: &DeclaredActionContext<'_>,
    action: &Action,
    declared: &DeclaredAction,
) -> Result<(once_core::Outcome, pending_outputs::Restoration)> {
    // clean_paths and create_dirs model the command's own filesystem
    // setup, so they must only run when the command actually executes.
    // A cache hit does not run command setup. Removing a clean_paths entry on
    // a hit could delete an unrelated path with no command left to recreate it.
    let action_digest = action.digest();
    // Old consumers could cache manifest text under these unchanged input digests.
    let reusable =
        !legacy_outputs::reads_fingerprints(declared, context.available_inputs, context.producers);
    if !reusable {
        tracing::debug!(target = %context.target_id, action_index = context.index, %action_digest,
            "fingerprint-only input prevents action-cache reuse");
    }
    if reusable {
        if let Some(outcome) = probe_cached_action(context, action_digest).await? {
            context.claim_worker().await;
            return Ok((outcome, pending_outputs::Restoration::default()));
        }
    }
    // Inputs go on disk before admission: restoring one can mean running its
    // producer again, which needs capacity of its own.
    let restored_outputs = preparation::restore(context, declared).await?;
    let outcome = recovery::with_action_lock_and_permit(
        context.workspace,
        action_digest,
        context.producers.executors(context.resources)?,
        action.resource_request().clone(),
        |contended, lease| async move {
            // Another process held the action, so it may have produced the
            // result while this one waited.
            if contended && reusable {
                if let Some(outcome) = probe_cached_action(context, action_digest).await? {
                    context.claim_worker().await;
                    return Ok(outcome);
                }
            }
            context.claim_worker().await;
            execute_declared_cache_miss(context, action, declared, lease).await
        },
    )
    .await
    .with_context(|| {
        format!(
            "running action {} for {} ({})",
            context.index, context.target_id, context.identifier
        )
    })?;
    if outcome.cache == once_core::CacheState::Miss {
        if let Some(cache) = context.source_digest_cache {
            cache.record_outputs(&outcome.result, context.workspace);
        }
    }
    Ok((outcome, restored_outputs))
}

/// Reuse only a fully available result. An unavailable cache skips reuse;
/// refusals, corruption, and local failures still stop execution.
async fn probe_cached_action(
    context: &DeclaredActionContext<'_>,
    action_digest: Digest,
) -> Result<Option<once_core::Outcome>> {
    let probe = || {
        format!(
            "probing cache for action {} for {} ({})",
            context.index, context.target_id, context.identifier
        )
    };
    let cached: once_cas::Result<_> = async {
        let Some(hit) = context.cache.lookup_action_result(&action_digest).await? else {
            return Ok(None);
        };
        let result = hit.result;
        if !hit.blobs_available
            && !action_result_blobs_present(
                &result,
                context.workspace,
                context.cache,
                context.source_digest_cache,
            )
            .await?
        {
            return Ok(None);
        }
        Ok(Some(once_core::Outcome {
            action: action_digest,
            result,
            cache: once_core::CacheState::Hit,
        }))
    }
    .await;
    match cached {
        Err(error) if error.is_unavailable() => {
            tracing::warn!(target = %context.target_id, action_index = context.index,
                %action_digest, error = %error,
                "cache unavailable; executing the action instead of reusing it");
            Ok(None)
        }
        result => result.with_context(probe),
    }
}

fn cacheable_action_lock(workspace: &Path, digest: Digest) -> Result<RwLock<std::fs::File>> {
    let directory = workspace.join(".once/locks/actions");
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("creating action lock directory `{}`", directory.display()))?;
    let path = directory.join(format!("{digest}.lock"));
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening action lock `{}`", path.display()))?;
    Ok(RwLock::new(file))
}

async fn action_result_blobs_present(
    result: &ActionResult,
    workspace: &Path,
    cache: &CacheProvider,
    source_digest_cache: Option<&SourceDigestCache>,
) -> once_cas::Result<bool> {
    for digest in result.stdout.iter().chain(result.stderr.iter()) {
        if !cache.has_blob(digest).await? {
            return Ok(false);
        }
    }
    for (relative, digest) in &result.outputs {
        // The question here is only whether this result can be reused, which
        // holds as soon as the bytes are available from somewhere. Ask in
        // increasing order of cost. The store on this machine answers with one
        // file existence check; describing the output on disk means walking it,
        // which for a directory output costs orders of magnitude more; asking a
        // remote tier costs a round trip.
        if cache.has_local_blob(digest).await? {
            continue;
        }
        if source_digest_cache
            .is_some_and(|digests| digests.output_matches(workspace, relative, *digest))
        {
            continue;
        }
        if !cache.has_blob(digest).await? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Run a cache miss whose inputs are on disk, holding its lock and permit.
async fn execute_declared_cache_miss(
    context: &DeclaredActionContext<'_>,
    action: &Action,
    declared: &DeclaredAction,
    lease: once_core::ExecutionPermit,
) -> Result<once_core::Outcome> {
    preparation::setup(context, declared, &lease).await?;
    let result = match once_core::run_uncached_admitted(
        action,
        context.workspace,
        context.cache,
        false,
        context.output_observer,
        lease,
    )
    .await
    {
        Ok(result) => Ok(result),
        Err(error) => Err(render_execution_error(error, context.cache).await),
    }
    .with_context(|| {
        format!(
            "executing action {} for {} ({})",
            context.index, context.target_id, context.identifier
        )
    })?;
    let action_digest = action.digest();
    if result.exit_code == 0 {
        context
            .cache
            .put_action_result(&action_digest, &result)
            .await
            .with_context(|| {
                format!(
                    "caching action {} for {} ({})",
                    context.index, context.target_id, context.identifier
                )
            })?;
    }
    Ok(once_core::Outcome {
        action: action_digest,
        result,
        cache: once_core::CacheState::Miss,
    })
}

/// The cached outputs still waiting to be put on disk that the action can
/// observe before it runs.
///
/// An action that depends on the ones before it may read any of them. One
/// that doesn't declares what it reads, which `unmaterialized_inputs_for`
/// selects, so only the outputs it is about to write over, clean, or remove
/// come back first, leaving the rest for the target's end.
fn prior_results_for(pending: PendingResults<'_>, declared: &DeclaredAction) -> Vec<ActionResult> {
    if declared.depends_on_prior_actions {
        pending.all().cloned().collect()
    } else {
        overlapping_results(pending.all(), &touched_paths(declared))
    }
}

/// The outputs of other actions the action declares reading that aren't on
/// disk yet.
fn unmaterialized_inputs_for(
    declared: &DeclaredAction,
    available_inputs: &BTreeMap<String, AvailableInput>,
) -> Option<ActionResult> {
    let outputs = declared
        .inputs
        .iter()
        .filter_map(|input| enclosing_available_output(available_inputs, input))
        .filter(|(_, input)| !input.materialized)
        .map(|(path, input)| (path.clone(), input.blob_digest))
        .collect::<BTreeMap<_, _>>();
    (!outputs.is_empty()).then_some(ActionResult {
        exit_code: 0,
        stdout: None,
        stderr: None,
        outputs,
    })
}

#[cfg(test)]
async fn materialize_prior_cached_results(
    workspace: &Path,
    cache: &CacheProvider,
    results: &[ActionResult],
    digests: Option<&SourceDigestCache>,
) -> Result<()> {
    let resources = Arc::new(ResourcePool::new(once_core::ResourceLimits::default()));
    let producers = Producers::default();
    let repair = Repair {
        workspace,
        cache,
        source_digest_cache: digests,
        resources: &resources,
        producers: &producers,
    };
    super::materialization::restore_recovering(results, repair).await
}

/// Put an action's outputs back on disk, skipping the ones already there.
///
/// Deciding "already there" is the whole cost: the restore primitive answers it
/// by reading every byte of every output, which for a link step means hashing
/// every library it links against. The recorded descriptions answer it from
/// metadata, and from the watcher when one is running.
async fn restore_outputs(
    workspace: &Path,
    cache: &CacheProvider,
    result: &ActionResult,
    source_digest_cache: Option<&SourceDigestCache>,
    lease: once_core::ExecutionPermit,
) -> Result<()> {
    match source_digest_cache {
        Some(digests) => digests
            .materialize_outputs(result, workspace, cache, Some(lease))
            .await
            .map_err(anyhow::Error::from),
        None => once_core::materialize_outputs_admitted(result, workspace, cache, lease)
            .await
            .map_err(anyhow::Error::from),
    }
}

async fn run_uncacheable_declared_action(
    context: DeclaredActionContext<'_>,
    action: Action,
    declared: &DeclaredAction,
) -> Result<DeclaredActionOutcome> {
    let action_digest = action.digest();
    let lease = context
        .producers
        .executors(context.resources)?
        .acquire(once_core::Executor::Local, action.resource_request())
        .await;
    context.claim_worker().await;
    preparation::setup(&context, declared, &lease).await?;
    let result = once_core::with_execution_permit(
        lease.clone(),
        Box::pin(run_uncached_action_admitted(
            &action,
            context.workspace,
            context.cache,
            declared.inherit_parent_env,
            Some(lease),
            Some((context.producers, context.identifier)),
        )),
    )
    .await
    .with_context(|| {
        format!(
            "executing action {} for {} ({})",
            context.index, context.target_id, context.identifier
        )
    })?;
    let exit_code = result.exit_code;
    if exit_code != 0 {
        record_declared_action_evidence(
            &context,
            &action,
            action_digest,
            EvidenceCacheState::Bypass,
            &result,
        )
        .await;
        return Err(ActionExecutionFailure {
            message: declared_action_failure_message(DeclaredActionFailure {
                cache: context.cache,
                identifier: context.identifier,
                index: context.index,
                target: context.target_id,
                exit_code,
                argv: context.argv,
                arg_files: context.arg_files,
                result: &result,
            })
            .await,
            exit_code,
            was_cached: false,
        }
        .into());
    }
    tracing::debug!(
        target = %context.target_id,
        action_index = context.index,
        identifier = %context.identifier,
        action_digest = %action_digest,
        "completed uncached declared graph action"
    );
    let evidence_record = context
        .record_success_evidence
        .then(|| {
            declared_action_evidence_record(
                context.target_id,
                context.capability,
                &action,
                action_digest,
                &context.input_fingerprint,
                EvidenceCacheState::Bypass,
                &result,
            )
        })
        .flatten();
    Ok(DeclaredActionOutcome {
        digest: action_digest,
        input_digest: action.input_digest(),
        input_fingerprint: context.input_fingerprint,
        cache_state: EvidenceCacheState::Bypass,
        result,
        evidence_record,
        restored_outputs: pending_outputs::Restoration::default(),
        prepare_ms: 0,
        execute_ms: 0,
    })
}

async fn record_declared_action_evidence(
    context: &DeclaredActionContext<'_>,
    action: &Action,
    action_digest: Digest,
    cache: EvidenceCacheState,
    result: &ActionResult,
) {
    if let Some(record) = declared_action_evidence_record(
        context.target_id,
        context.capability,
        action,
        action_digest,
        &context.input_fingerprint,
        cache,
        result,
    ) {
        crate::commands::evidence::append_records(context.workspace, &[record]).await;
    }
}

fn declared_action_evidence_record(
    target_id: &str,
    capability: &str,
    action: &Action,
    action_digest: Digest,
    input_fingerprint: &InputFingerprintManifest,
    cache: EvidenceCacheState,
    result: &ActionResult,
) -> Option<EvidenceRecord> {
    match EvidenceRecord::from_action_result_with_fingerprint(
        EvidenceSubject::target(target_id, capability),
        action_digest,
        action.input_digest(),
        Some(input_fingerprint.clone()),
        cache,
        result,
    ) {
        Ok(record) => Some(record),
        Err(error) => {
            tracing::warn!(
                %error,
                target = target_id,
                capability,
                "failed to construct evidence record"
            );
            None
        }
    }
}

async fn declared_action_failure_message(failure: DeclaredActionFailure<'_>) -> String {
    let DeclaredActionFailure {
        cache,
        identifier,
        index,
        target,
        exit_code,
        argv,
        arg_files,
        result,
    } = failure;
    let mut message =
        format!("{identifier} ({index}) failed for {target} with exit code {exit_code}");
    append_declared_argv(&mut message, argv);
    append_declared_arg_files(&mut message, arg_files);
    append_captured_output(cache, &mut message, "stdout", result.stdout.as_ref()).await;
    append_captured_output(cache, &mut message, "stderr", result.stderr.as_ref()).await;
    message
}

fn append_declared_argv(message: &mut String, argv: &[String]) {
    if argv.is_empty() {
        return;
    }

    message.push_str("\n\nargv:");
    append_arg_file_arg_list(message, "first args", argv.iter().take(32));
    let start = argv.len().saturating_sub(16);
    if start > 32 {
        append_arg_file_arg_list(message, "last args", argv.iter().skip(start));
    }
}

fn append_declared_arg_files(message: &mut String, arg_files: &[DeclaredArgFile]) {
    if arg_files.is_empty() {
        return;
    }

    message.push_str("\n\narg files:");
    for arg_file in arg_files {
        let _ = write!(
            message,
            "\n{} [{}], {} args",
            arg_file.path,
            declared_arg_file_format_name(arg_file.format),
            arg_file.args.len()
        );
        append_arg_file_arg_list(message, "first args", arg_file.args.iter().take(32));
        let start = arg_file.args.len().saturating_sub(16);
        if start > 32 {
            append_arg_file_arg_list(message, "last args", arg_file.args.iter().skip(start));
        }
    }
}

fn append_arg_file_arg_list<'a>(
    message: &mut String,
    label: &str,
    mut args: impl Iterator<Item = &'a String>,
) {
    let Some(first_arg) = args.next() else {
        return;
    };

    let _ = write!(message, "\n{label}:");
    let _ = write!(message, "\n  {first_arg}");
    for arg in args {
        let _ = write!(message, "\n  {arg}");
    }
}

async fn append_captured_output(
    cache: &CacheProvider,
    message: &mut String,
    name: &str,
    digest: Option<&Digest>,
) {
    let Some(digest) = digest else {
        return;
    };
    let (bytes, truncated) = match cache
        .read_blob_limited(digest, FAILURE_OUTPUT_LIMIT as u64, true)
        .await
    {
        Ok(output) => output,
        Err(err) => {
            tracing::warn!(
                output = name,
                digest = %digest,
                error = %err,
                "failed to read captured declared action output"
            );
            return;
        }
    };
    if bytes.is_empty() {
        return;
    }
    let prefix = if truncated {
        format!("last {FAILURE_OUTPUT_LIMIT} bytes of ")
    } else {
        String::new()
    };
    message.push_str("\n\n");
    message.push_str(&prefix);
    message.push_str(name);
    message.push_str(":\n");
    message.push_str(&String::from_utf8_lossy(&bytes));
}

async fn render_execution_error(error: once_core::Error, cache: &CacheProvider) -> anyhow::Error {
    let once_core::Error::RejectedZeroExitCode { stdout, stderr, .. } = &error else {
        return error.into();
    };
    let mut message = String::new();
    append_captured_output(cache, &mut message, "stdout", stdout.as_ref()).await;
    append_captured_output(cache, &mut message, "stderr", stderr.as_ref()).await;
    let error = anyhow::Error::new(error);
    if message.is_empty() {
        error
    } else {
        error.context(message.trim_start().to_owned())
    }
}

fn compose_target_action_digest(target_id: &str, action_digests: &[Digest]) -> Digest {
    match action_digests {
        [] => Digest::of_bytes(format!("empty:{target_id}").as_bytes()),
        [digest] => *digest,
        _ => {
            let mut builder = InputDigestBuilder::new(b"once.target.actions.v1\0");
            builder.push_bytes(target_id.as_bytes());
            for (index, digest) in action_digests.iter().enumerate() {
                let key = format!("action:{index}");
                builder.push_keyed(key.as_bytes(), digest);
            }
            builder.finish()
        }
    }
}

fn compose_target_input_digest(input_digests: &[Digest]) -> Option<Digest> {
    match input_digests {
        [] => None,
        [digest] => Some(*digest),
        _ => {
            let mut builder = InputDigestBuilder::new(b"once.target.inputs.v1\0");
            for (index, digest) in input_digests.iter().enumerate() {
                let key = format!("input:{index}");
                builder.push_keyed(key.as_bytes(), digest);
            }
            Some(builder.finish())
        }
    }
}

fn compose_target_input_fingerprint(
    input_digest: Option<Digest>,
    input_fingerprints: Vec<InputFingerprintManifest>,
) -> Option<InputFingerprintManifest> {
    let input_digest = input_digest?;
    match input_fingerprints.as_slice() {
        [fingerprint] if fingerprint.input_digest == input_digest => {
            input_fingerprints.into_iter().next()
        }
        [] | [_] => None,
        _ => {
            let components = input_fingerprints
                .into_iter()
                .enumerate()
                .flat_map(|(index, fingerprint)| {
                    fingerprint.components.into_iter().map(move |component| {
                        InputFingerprintComponent::new(
                            component.category,
                            format!("action:{index}:{}", component.label),
                            component.digest,
                        )
                    })
                })
                .collect();
            Some(InputFingerprintManifest::new(input_digest, components))
        }
    }
}

struct PreparedDeclaredAction {
    action: Action,
    input_fingerprint: InputFingerprintManifest,
}

fn declared_to_action(
    workspace: &Path,
    declared: &DeclaredAction,
    module_source_digest: Digest,
    dep_action_digests: &[(String, Digest)],
    sandbox_override: SandboxMode,
) -> Result<Action> {
    Ok(prepare_declared_action_with_inputs(
        workspace,
        declared,
        module_source_digest,
        dep_action_digests,
        &BTreeMap::new(),
        None,
        sandbox_override,
    )?
    .action)
}

fn prepare_declared_action_with_inputs(
    workspace: &Path,
    declared: &DeclaredAction,
    module_source_digest: Digest,
    dep_action_digests: &[(String, Digest)],
    available_inputs: &BTreeMap<String, AvailableInput>,
    source_digest_cache: Option<&SourceDigestCache>,
    sandbox_override: SandboxMode,
) -> Result<PreparedDeclaredAction> {
    let inherited;
    let declared = if declared.cacheable && declared.inherit_parent_env {
        anyhow::ensure!(
            effective_sandbox(declared.sandbox.as_deref(), sandbox_override)? == SandboxMode::Off,
            "inherit_parent_env requires sandbox = \"off\""
        );
        let mut effective = declared.clone();
        effective.env = env::vars_os()
            .map(|(key, value)| {
                Ok((
                    key.into_string().map_err(|_| {
                        anyhow::anyhow!("inherited environment names must be UTF-8")
                    })?,
                    value.into_string().map_err(|_| {
                        anyhow::anyhow!("inherited environment values must be UTF-8")
                    })?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        effective.env.extend(declared.env.clone());
        inherited = effective;
        &inherited
    } else {
        declared
    };
    let env_keys = declared.env.keys().cloned().collect::<Vec<_>>();
    tracing::trace!(
        identifier = ?declared.identifier,
        argv_len = declared.argv.len(),
        env_keys = ?env_keys,
        inputs = declared.inputs.len(),
        outputs = declared.outputs.len(),
        "declared graph action"
    );
    let input_fingerprint = compose_input_fingerprint_with_available(
        workspace,
        declared,
        module_source_digest,
        dep_action_digests,
        available_inputs,
        source_digest_cache,
    )?;
    let input_digest = input_fingerprint.input_digest;
    let inputs = declared_action_inputs(declared)?;
    let outputs = declared
        .outputs
        .iter()
        .map(|path| {
            WorkspacePath::try_from(path.as_str())
                .with_context(|| format!("invalid declared output path `{path}`"))
        })
        .collect::<Result<Vec<_>>>()?;
    ensure_output_parent_dirs(workspace, &outputs)?;
    let stdout_path = declared
        .stdout
        .as_deref()
        .map(|path| workspace_path(path, "run_action stdout path"))
        .transpose()?
        .map(Box::new);
    let stderr_path = declared
        .stderr
        .as_deref()
        .map(|path| workspace_path(path, "run_action stderr path"))
        .transpose()?
        .map(Box::new);
    let action = match &declared.operation {
        None => Action::RunCommand {
            argv: declared.argv.clone(),
            env: declared.env.clone(),
            cwd: declared
                .cwd
                .as_deref()
                .map(|cwd| workspace_path(cwd, "run_action cwd"))
                .transpose()?,
            input_digest: Some(input_digest),
            inputs,
            outputs,
            stdout_path,
            stderr_path,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: effective_sandbox(declared.sandbox.as_deref(), sandbox_override)?,
            network: effective_network(declared.network.as_deref())?,
            timeout_ms: None,
            success_exit_codes: declared.success_exit_codes.clone(),
            remote: None,
        },
        Some(operation) => operation_to_action(operation.clone(), input_digest)?,
    };
    Ok(PreparedDeclaredAction {
        action,
        input_fingerprint,
    })
}

fn materialize_declared_arg_files(workspace: &Path, arg_files: &[DeclaredArgFile]) -> Result<()> {
    for arg_file in arg_files {
        let path = workspace_path(&arg_file.path, "arg_files path")?;
        let absolute = path.resolve(workspace);
        if let Some(parent) = absolute.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("creating parent directory for arg file `{}`", path.as_str())
            })?;
        }
        let content = declared_arg_file_content(arg_file)?;
        std::fs::write(&absolute, content)
            .with_context(|| format!("writing arg file `{}`", path.as_str()))?;
    }
    Ok(())
}

fn declared_arg_file_content(arg_file: &DeclaredArgFile) -> Result<Vec<u8>> {
    match arg_file.format {
        DeclaredArgFileFormat::LineDelimited => declared_arg_file_lines(arg_file, |arg| {
            validate_arg_file_line(arg_file, arg)?;
            Ok(arg.to_string())
        }),
    }
}

fn declared_arg_file_lines(
    arg_file: &DeclaredArgFile,
    format: impl Fn(&str) -> Result<String>,
) -> Result<Vec<u8>> {
    let mut content = Vec::new();
    for arg in &arg_file.args {
        let line = format(arg)?;
        content.extend_from_slice(line.as_bytes());
        content.push(b'\n');
    }
    Ok(content)
}

fn validate_arg_file_line(arg_file: &DeclaredArgFile, arg: &str) -> Result<()> {
    if arg.contains('\n') || arg.contains('\r') {
        anyhow::bail!(
            "{} arg file `{}` contains an argument with a newline",
            declared_arg_file_format_name(arg_file.format),
            arg_file.path
        );
    }
    Ok(())
}

fn declared_arg_file_format_name(format: DeclaredArgFileFormat) -> &'static str {
    match format {
        DeclaredArgFileFormat::LineDelimited => "line-delimited",
    }
}

#[allow(clippy::too_many_lines)]
fn operation_to_action(operation: DeclaredActionOperation, input_digest: Digest) -> Result<Action> {
    Ok(match operation {
        DeclaredActionOperation::ExpandActions { .. } => {
            anyhow::bail!("deferred actions must be expanded before execution")
        }
        DeclaredActionOperation::WriteFile { path, bytes } => Action::WriteFile {
            path: workspace_path(&path, "write_path path")?,
            bytes,
            input_digest: Some(input_digest),
        },
        DeclaredActionOperation::CopyPath {
            sources,
            destination,
            mode,
        } => Action::CopyPath {
            sources: sources
                .iter()
                .map(|source| workspace_path(source, "copy_path source"))
                .collect::<Result<Vec<_>>>()?,
            destination: workspace_path(&destination, "copy_path destination")?,
            mode: match mode {
                DeclaredCopyPathMode::File => CopyPathMode::File,
                DeclaredCopyPathMode::Tree => CopyPathMode::Tree,
            },
            input_digest: Some(input_digest),
        },
        DeclaredActionOperation::LinkPath {
            source,
            destination,
        } => Action::LinkPath {
            source: workspace_path(&source, "link_path source")?,
            destination: workspace_path(&destination, "link_path destination")?,
            input_digest: Some(input_digest),
        },
        DeclaredActionOperation::MaterializeHostFile {
            source,
            source_sha256,
            destination,
        } => {
            let source = std::path::PathBuf::from(source);
            if !source.is_absolute() {
                anyhow::bail!(
                    "invalid materialize_host_file source `{}`: expected an absolute path",
                    source.display()
                );
            }
            Action::MaterializeHostFile {
                source,
                source_sha256,
                destination: workspace_path(&destination, "materialize_host_file destination")?,
                input_digest: Some(input_digest),
            }
        }
        DeclaredActionOperation::MaterializeHostTree {
            source,
            source_sha256,
            destination,
        } => {
            let source = std::path::PathBuf::from(source);
            if !source.is_absolute() {
                anyhow::bail!(
                    "invalid materialize_host_tree source `{}`: expected an absolute path",
                    source.display()
                );
            }
            Action::MaterializeHostTree {
                source,
                source_sha256,
                destination: workspace_path(&destination, "materialize_host_tree destination")?,
                input_digest: Some(input_digest),
            }
        }
        DeclaredActionOperation::PreparePath { path, mode } => Action::PreparePath {
            path: workspace_path(&path, "prepare_path path")?,
            mode: match mode {
                DeclaredPreparePathMode::Remove => PreparePathMode::Remove,
                DeclaredPreparePathMode::Directory => PreparePathMode::Directory,
            },
            input_digest: Some(input_digest),
        },
        DeclaredActionOperation::WriteTreeDigest {
            root,
            output,
            include_suffixes,
        } => Action::WriteTreeDigest {
            root: workspace_path(&root, "write_tree_digest root")?,
            output: workspace_path(&output, "write_tree_digest output")?,
            include_suffixes,
            input_digest: Some(input_digest),
        },
        DeclaredActionOperation::WriteArchive {
            entries,
            output,
            sha256_output,
            uncompressed_sha256_output,
            format,
        } => archive_to_action(
            entries,
            &output,
            sha256_output.as_deref(),
            uncompressed_sha256_output.as_deref(),
            format,
            input_digest,
        )?,
        DeclaredActionOperation::DownloadAndExtract {
            url,
            sha256,
            destination,
            authorization_env,
        } => Action::DownloadAndExtract {
            url,
            sha256,
            destination: workspace_path(&destination, "download_and_extract destination")?,
            authorization_env,
            input_digest: Some(input_digest),
        },
    })
}

fn archive_to_action(
    entries: Vec<once_frontend::analysis::DeclaredArchiveEntry>,
    output: &str,
    sha256_output: Option<&str>,
    uncompressed_sha256_output: Option<&str>,
    format: DeclaredArchiveFormat,
    input_digest: Digest,
) -> Result<Action> {
    let entries = entries
        .into_iter()
        .map(|entry| {
            Ok(ArchiveEntry {
                kind: match entry.kind {
                    DeclaredArchiveEntryKind::File => ArchiveEntryKind::File,
                    DeclaredArchiveEntryKind::Directory => ArchiveEntryKind::Directory,
                    DeclaredArchiveEntryKind::Tree => ArchiveEntryKind::Tree,
                    DeclaredArchiveEntryKind::Symlink => ArchiveEntryKind::Symlink,
                },
                source: entry
                    .source
                    .as_deref()
                    .map(|source| workspace_path(source, "write_archive entry source"))
                    .transpose()?,
                path: entry.path,
                target: entry.target,
                mode: entry.mode,
                directory_mode: entry.directory_mode,
                owner_id: entry.owner_id,
                group_id: entry.group_id,
                mtime: entry.mtime,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Action::WriteArchive {
        entries,
        output: workspace_path(output, "write_archive output")?,
        sha256_output: sha256_output
            .map(|path| workspace_path(path, "write_archive sha256_output"))
            .transpose()?,
        uncompressed_sha256_output: uncompressed_sha256_output
            .map(|path| workspace_path(path, "write_archive uncompressed_sha256_output"))
            .transpose()?,
        format: match format {
            DeclaredArchiveFormat::Tar => ArchiveFormat::Tar,
            DeclaredArchiveFormat::TarGz => ArchiveFormat::TarGz,
            DeclaredArchiveFormat::Zip => ArchiveFormat::Zip,
        },
        input_digest: Some(input_digest),
    })
}

fn workspace_path(path: &str, context: &str) -> Result<WorkspacePath> {
    WorkspacePath::try_from(path).with_context(|| format!("invalid {context} `{path}`"))
}

/// Declared inputs that do not exist in the workspace. During isolated action
/// validation these are outputs a prior action or dependency target would have
/// produced, which the probe never runs. Malformed paths are left out so
/// `declared_to_action` can surface them as the usual invalid-path error.
fn missing_declared_inputs(workspace: &Path, declared: &DeclaredAction) -> Vec<String> {
    declared
        .inputs
        .iter()
        .filter(|input| {
            WorkspacePath::try_from(input.as_str())
                .is_ok_and(|path| !path.resolve(workspace).exists())
        })
        .cloned()
        .collect()
}

fn declared_action_inputs(declared: &DeclaredAction) -> Result<Vec<WorkspacePath>> {
    let mut inputs = declared
        .inputs
        .iter()
        .map(|path| workspace_path(path, "run_action input"))
        .collect::<Result<Vec<_>>>()?;
    for arg_file in &declared.arg_files {
        inputs.push(workspace_path(&arg_file.path, "run_action arg file")?);
    }
    for path in &declared.create_dirs {
        // Action-private directories must exist inside isolated and remote
        // execution roots, while output directories must not be staged back
        // as inputs.
        if path == ".once/tmp" || path.starts_with(".once/tmp/") {
            inputs.push(workspace_path(path, "run_action create_dirs entry")?);
        }
    }
    inputs.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    inputs.dedup_by(|a, b| a.as_str() == b.as_str());
    Ok(inputs)
}

fn effective_sandbox(declared: Option<&str>, sandbox_override: SandboxMode) -> Result<SandboxMode> {
    let declared = match declared {
        Some(raw) => raw
            .parse::<SandboxMode>()
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("parsing sandbox policy `{raw}`"))?,
        None => SandboxMode::Off,
    };
    Ok(declared.stronger(sandbox_override))
}

fn effective_network(declared: Option<&str>) -> Result<NetworkPolicy> {
    match declared {
        Some(raw) => raw
            .parse::<NetworkPolicy>()
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("parsing network policy `{raw}`")),
        // Actions that do not declare a network policy default to
        // unrestricted. The reproducible-build effort meant to switch
        // this to `Deny`, but doing so broke every action that spawns
        // a subprocess needing a socketpair for stdout/stderr (the
        // Linux seccomp filter denies related syscalls) and every
        // action that in turn wraps itself in a macOS Seatbelt profile
        // (nested `sandbox_apply` calls are refused on recent runners).
        // Actions that genuinely need network isolation opt in with
        // `network = "deny"` explicitly.
        None => Ok(NetworkPolicy::Unrestricted),
    }
}

fn ensure_output_parent_dirs(workspace: &Path, outputs: &[WorkspacePath]) -> Result<()> {
    for output in outputs {
        let absolute = output.resolve(workspace);
        if let Some(parent) = absolute.parent() {
            let mut directories = parent
                .ancestors()
                .take_while(|directory| *directory != workspace)
                .collect::<Vec<_>>();
            directories.reverse();
            for directory in directories {
                if directory.is_dir() {
                    continue;
                }
                let blocker = std::fs::symlink_metadata(directory).ok();
                if blocker.as_ref().is_some_and(std::fs::Metadata::is_dir) {
                    continue;
                }
                if blocker.is_some() {
                    let path = output.as_str();
                    let managed = path.starts_with(".once/out/")
                        || path.contains("/.once/out/")
                        || path.starts_with(".once/tmp/")
                        || path.contains("/.once/tmp/");
                    if !managed {
                        anyhow::bail!(
                            "output `{}` has a non-directory parent `{}`",
                            output.as_str(),
                            directory.display()
                        );
                    }
                    std::fs::remove_file(directory).with_context(|| {
                        format!(
                            "removing stale non-directory parent for output `{}`",
                            output.as_str()
                        )
                    })?;
                }
                match std::fs::create_dir(directory) {
                    Ok(()) => {}
                    Err(error)
                        if error.kind() == std::io::ErrorKind::AlreadyExists
                            && directory.is_dir() => {}
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("creating parent directory for output `{}`", output.as_str())
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
fn compose_input_digest(
    workspace: &Path,
    declared: &DeclaredAction,
    module_source_digest: Digest,
    dep_action_digests: &[(String, Digest)],
) -> Result<Digest> {
    compose_input_digest_with_available(
        workspace,
        declared,
        module_source_digest,
        dep_action_digests,
        &BTreeMap::new(),
        None,
    )
}

#[cfg(test)]
fn compose_input_digest_with_available(
    workspace: &Path,
    declared: &DeclaredAction,
    module_source_digest: Digest,
    dep_action_digests: &[(String, Digest)],
    available_inputs: &BTreeMap<String, AvailableInput>,
    source_digest_cache: Option<&SourceDigestCache>,
) -> Result<Digest> {
    Ok(compose_input_fingerprint_with_available(
        workspace,
        declared,
        module_source_digest,
        dep_action_digests,
        available_inputs,
        source_digest_cache,
    )?
    .input_digest)
}

/// Find the action output a declared input comes from: an exact match, or the
/// closest enclosing directory output.
///
/// A rule may declare one file inside a directory that another action emits,
/// such as the build script inside a snapshot of a package's sources. That
/// file is only on disk once the producing action's outputs are materialized,
/// and a cache hit defers materialization until some later action misses. The
/// digest therefore has to come from the producer, not from the filesystem,
/// or hashing the input fails outright on a hit and disagrees with the miss
/// path when it succeeds.
fn resolve_available_input<'a>(
    available_inputs: &'a BTreeMap<String, AvailableInput>,
    input: &str,
) -> Option<&'a AvailableInput> {
    if let Some(available) = available_inputs.get(input) {
        return Some(available);
    }
    let mut candidate = input;
    while let Some((parent, _)) = candidate.rsplit_once('/') {
        if let Some(available) = available_inputs.get(parent) {
            return Some(available);
        }
        candidate = parent;
    }
    None
}

/// The output path whose materialization puts `input` on disk, paired with its
/// blob digest. Mirrors [`resolve_available_input`] for the staging step.
fn enclosing_available_output<'a>(
    available_inputs: &'a BTreeMap<String, AvailableInput>,
    input: &str,
) -> Option<(&'a String, &'a AvailableInput)> {
    if let Some((path, available)) = available_inputs.get_key_value(input) {
        return Some((path, available));
    }
    let mut candidate = input;
    while let Some((parent, _)) = candidate.rsplit_once('/') {
        if let Some((path, available)) = available_inputs.get_key_value(parent) {
            return Some((path, available));
        }
        candidate = parent;
    }
    None
}

fn compose_input_fingerprint_with_available(
    workspace: &Path,
    declared: &DeclaredAction,
    _module_source_digest: Digest,
    dep_action_digests: &[(String, Digest)],
    available_inputs: &BTreeMap<String, AvailableInput>,
    source_digest_cache: Option<&SourceDigestCache>,
) -> Result<InputFingerprintManifest> {
    let mut builder = InputDigestBuilder::new(b"once.declared_action.input.v5\0");
    push_declared_action_metadata(&mut builder, declared, workspace)?;

    let mut sorted_inputs = declared
        .inputs
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    sorted_inputs.sort_unstable();
    sorted_inputs.dedup();
    for input in &sorted_inputs {
        if let Some(available) = resolve_available_input(available_inputs, input) {
            let digest = &available.blob_digest;
            let category = if available.same_target {
                "generated-input"
            } else {
                "dependency-output"
            };
            builder.push_keyed_component(category, *input, input.as_bytes(), digest);
        } else {
            let digest = match source_digest_cache {
                Some(cache) => cache.digest(workspace, input),
                None => once_core::digest_source_path(workspace, input),
            }
            .with_context(|| format!("hashing declared input `{input}`"))?;
            builder.push_keyed_component("source", *input, input.as_bytes(), &digest);
        }
    }

    for (label, digest) in dep_action_digests {
        if label.starts_with("same-target:") {
            let key = format!("dep:{label}");
            builder.push_keyed_component("dependency", label, key.as_bytes(), digest);
        }
    }

    Ok(builder.finish_with_fingerprint())
}

fn push_declared_action_metadata(
    builder: &mut InputDigestBuilder,
    declared: &DeclaredAction,
    workspace: &Path,
) -> Result<()> {
    let identities = serde_json::to_vec(&(&declared.toolchain_identity, &declared.identifier))
        .context("serializing declared action identities")?;
    builder.push_bytes(&identities);
    if let Some(identity) = &declared.toolchain_identity {
        builder.record_bytes("toolchain", "identity", identity.as_bytes());
    }
    if let Some(identifier) = &declared.identifier {
        builder.record_bytes("action", "identifier", identifier.as_bytes());
    }
    if let Some(operation) = &declared.operation {
        let encoded =
            serde_json::to_vec(operation).context("serializing declared action operation")?;
        builder.push_bytes_component("command", "operation", &encoded);
    }
    let canonical_argv = declared
        .argv
        .iter()
        .map(|arg| canonical_action_value(arg, workspace))
        .collect::<Vec<_>>();
    for arg in &canonical_argv {
        builder.push_bytes(arg.as_bytes());
    }
    if !canonical_argv.is_empty() {
        let encoded =
            serde_json::to_vec(&canonical_argv).context("serializing declared action arguments")?;
        builder.record_bytes("command", "arguments", &encoded);
    }
    if let Some(stdout) = &declared.stdout {
        builder.push_keyed_component(
            "command",
            "stdout-path",
            b"stdout",
            &Digest::of_bytes(stdout.as_bytes()),
        );
    }
    if let Some(stderr) = &declared.stderr {
        builder.push_keyed_component(
            "command",
            "stderr-path",
            b"stderr",
            &Digest::of_bytes(stderr.as_bytes()),
        );
    }
    let encoded_arg_files =
        serde_json::to_vec(&declared.arg_files).context("serializing declared action arg files")?;
    builder.push_bytes(&encoded_arg_files);
    if !declared.arg_files.is_empty() {
        builder.record_bytes("command", "argument-files", &encoded_arg_files);
    }
    let canonical_env = declared
        .env
        .iter()
        .map(|(key, value)| (key, canonical_action_value(value, workspace)))
        .collect::<BTreeMap<_, _>>();
    for (key, value) in &canonical_env {
        builder.push_bytes(key.as_bytes());
        builder.push_bytes(value.as_bytes());
        builder.record_bytes("environment", format!("variable:{key}"), value.as_bytes());
    }
    if !canonical_env.is_empty() {
        let encoded =
            serde_json::to_vec(&canonical_env).context("serializing declared environment")?;
        builder.record_bytes("environment", "declared", &encoded);
    }
    for path in &declared.clean_paths {
        builder.push_bytes(b"clean_path");
        builder.push_bytes(path.as_bytes());
    }
    for path in &declared.create_dirs {
        builder.push_bytes(b"create_dir");
        builder.push_bytes(path.as_bytes());
    }
    if !declared.clean_paths.is_empty() || !declared.create_dirs.is_empty() {
        let encoded = serde_json::to_vec(&(&declared.clean_paths, &declared.create_dirs))
            .context("serializing declared path setup")?;
        builder.record_bytes("command", "path-setup", &encoded);
    }
    if let Some(cwd) = &declared.cwd {
        builder.push_bytes(b"cwd").push_bytes(cwd.as_bytes());
        builder.record_bytes("command", "working-directory", cwd.as_bytes());
    }
    Ok(())
}

fn canonical_action_value(value: &str, workspace: &Path) -> String {
    let workspace = workspace.to_string_lossy();
    if workspace.is_empty() {
        return value.to_owned();
    }
    value.replace(workspace.as_ref(), "{{once.execution_root}}")
}

#[cfg(test)]
mod tests;
