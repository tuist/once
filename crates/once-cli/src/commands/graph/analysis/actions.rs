use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use once_cas::{ActionResult, CacheProvider, Digest};
use once_core::{
    resolve_execution_argv, resolve_execution_env, validate_action_contract_with_options,
    workspace_mise_command, workspace_prepared_tool_env, workspace_tool_env_with_executables,
    Action, ActionContractOptions, ArchiveEntry, ArchiveEntryKind, ArchiveFormat, CopyPathMode,
    EvidenceCacheState, EvidenceRecord, EvidenceSubject, FilesystemOperation, InputDigestBuilder,
    InputFingerprintComponent, InputFingerprintManifest, OutputSymlinkMode, PreparePathMode,
    ResourcePool, ResourceRequest, SandboxMode, WorkspacePath,
};
use once_frontend::analysis::{
    AnalysisResult, DeclaredAction, DeclaredActionOperation, DeclaredArchiveEntryKind,
    DeclaredArchiveFormat, DeclaredArgFile, DeclaredArgFileFormat, DeclaredCopyPathMode,
    DeclaredPreparePathMode,
};
use once_frontend::GraphTarget;
use serde::Serialize;
use tokio::process::Command;

use super::source_digest_cache::SourceDigestCache;
use super::{AvailableInput, BuildOutcome};

const FAILURE_OUTPUT_LIMIT: usize = 16 * 1024;

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
    prior_cached_results: &'a [ActionResult],
    record_success_evidence: bool,
    sandbox: SandboxMode,
    resources: &'a Arc<ResourcePool>,
}

struct DeclaredActionOutcome {
    digest: Digest,
    input_digest: Option<Digest>,
    input_fingerprint: InputFingerprintManifest,
    cache_state: EvidenceCacheState,
    result: ActionResult,
    evidence_record: Option<EvidenceRecord>,
}

struct DeclaredActionsState {
    action_digests: Vec<Digest>,
    input_digests: Vec<Digest>,
    input_fingerprints: Vec<InputFingerprintManifest>,
    available_inputs: BTreeMap<String, AvailableInput>,
    aggregate_cache_state: Option<EvidenceCacheState>,
    outputs: Vec<String>,
    result: ActionResult,
    cached_results: Vec<ActionResult>,
    evidence_records: Vec<EvidenceRecord>,
}

impl DeclaredActionsState {
    fn new(available_inputs: &BTreeMap<String, AvailableInput>) -> Self {
        Self {
            action_digests: Vec::new(),
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
            evidence_records: Vec::new(),
        }
    }

    fn input_action_digests(
        &self,
        declared: &DeclaredAction,
        dependency_digests: &[(String, Digest)],
    ) -> Vec<(String, Digest)> {
        let mut input_digests = dependency_digests.to_vec();
        if declared.depends_on_prior_actions {
            input_digests.extend(
                self.action_digests
                    .iter()
                    .enumerate()
                    .map(|(index, digest)| (format!("same-target:{index}"), *digest)),
            );
        }
        input_digests
    }

    fn record(&mut self, outcome: DeclaredActionOutcome, streams: bool) {
        self.action_digests.push(outcome.digest);
        self.available_inputs
            .extend(outcome.result.outputs.iter().map(|(path, digest)| {
                (
                    path.clone(),
                    AvailableInput {
                        blob_digest: *digest,
                        producer_action_digest: outcome.digest,
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
        track_cached_result(&mut self.cached_results, &outcome);
        self.input_fingerprints.push(outcome.input_fingerprint);
        self.evidence_records.extend(outcome.evidence_record);
    }

    fn finish(mut self, target_id: &str, provider: serde_json::Value) -> BuildOutcome {
        deduplicate_outputs(&mut self.outputs);
        let cache_state = self
            .aggregate_cache_state
            .unwrap_or(EvidenceCacheState::Miss);
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
        }
    }
}

struct DeclaredActionContext<'a> {
    workspace: &'a Path,
    cache: &'a CacheProvider,
    source_digest_cache: Option<&'a SourceDigestCache>,
    available_inputs: &'a BTreeMap<String, AvailableInput>,
    prior_cached_results: &'a [ActionResult],
    target_id: &'a str,
    capability: &'a str,
    index: usize,
    identifier: &'a str,
    argv: &'a [String],
    arg_files: &'a [DeclaredArgFile],
    input_fingerprint: InputFingerprintManifest,
    record_success_evidence: bool,
    resources: &'a Arc<ResourcePool>,
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
    dep_action_digests: &[(String, Digest)],
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
    let mut validations = Vec::new();
    for (index, declared) in actions.into_iter().enumerate() {
        let is_selected = selected_index.is_none_or(|selected| selected == index);
        let mut input_action_digests = dep_action_digests.to_vec();
        if declared.depends_on_prior_actions {
            input_action_digests.extend(
                action_digests
                    .iter()
                    .enumerate()
                    .map(|(prior_index, digest)| (format!("same-target:{prior_index}"), *digest)),
            );
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

/// Materialise each declared action through the action cache, then
/// fold the analysis provider directly into the build outcome.
///
/// Returns a boxed future intentionally because the concrete future
/// captures declared action state and cache execution state. Boxing at
/// this boundary keeps parent graph futures small enough for
/// `clippy::large_futures` and centralizes the allocation.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) fn run_declared_actions<'a>(
    workspace: &'a Path,
    cache: &'a CacheProvider,
    module_source_digest: Digest,
    target: &'a GraphTarget,
    capability: &'a str,
    analysis: AnalysisResult,
    dep_action_digests: &'a [(String, Digest)],
    dependency_inputs: &'a BTreeMap<String, AvailableInput>,
    tool_paths: &'a BTreeMap<String, String>,
    source_digest_cache: Option<&'a SourceDigestCache>,
    sandbox: SandboxMode,
    resources: &'a Arc<ResourcePool>,
) -> Pin<Box<dyn Future<Output = Result<BuildOutcome>> + Send + 'a>> {
    Box::pin(async move {
        let AnalysisResult {
            mut actions,
            provider,
            ..
        } = analysis;
        expose_target_tools(workspace, target, &mut actions, Some(tool_paths)).await?;
        tracing::trace!(
            target = %target.label.id,
            declared_actions = actions.len(),
            dep_action_digests = dep_action_digests.len(),
            "running declared graph actions"
        );
        let mut state = DeclaredActionsState::new(dependency_inputs);
        // A single-action target is fully represented by the caller's
        // capability-level record. Multi-action targets need per-action
        // success evidence so individual streams and outputs stay visible.
        let record_success_evidence = actions.len() > 1;

        for (index, declared) in actions.into_iter().enumerate() {
            state.outputs.extend(declared.outputs.iter().cloned());
            let input_action_digests = state.input_action_digests(&declared, dep_action_digests);
            let outcome = Box::pin(run_declared_action(DeclaredActionRun {
                workspace,
                cache,
                module_source_digest,
                target_id: &target.label.id,
                capability,
                index,
                declared,
                input_action_digests: &input_action_digests,
                available_inputs: &state.available_inputs,
                source_digest_cache,
                prior_cached_results: &state.cached_results,
                record_success_evidence,
                sandbox,
                resources,
            }))
            .await;
            let outcome = match outcome {
                Ok(outcome) => outcome,
                Err(error) => {
                    crate::commands::evidence::append_records(workspace, &state.evidence_records)
                        .await;
                    return Err(error);
                }
            };
            state.record(outcome, !record_success_evidence);
        }

        crate::commands::evidence::append_records(workspace, &state.evidence_records).await;
        Ok(state.finish(&target.label.id, provider))
    })
}

fn track_cached_result(cached_results: &mut Vec<ActionResult>, outcome: &DeclaredActionOutcome) {
    if outcome.cache_state == EvidenceCacheState::Hit {
        if !outcome.result.outputs.is_empty() {
            cached_results.push(outcome.result.clone());
        }
        return;
    }
    cached_results.clear();
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

fn host_executable_path(executable: &str) -> Option<PathBuf> {
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
        .find(|candidate| candidate.is_file())
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
    } = run;
    let identifier_for_error = declared
        .identifier
        .clone()
        .unwrap_or_else(|| "<anonymous>".to_string());
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
    materialize_declared_arg_files(workspace, &declared.arg_files).with_context(|| {
        format!("writing arg files for action {index} for {target_id} ({identifier_for_error})")
    })?;
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
    };

    if cacheable {
        run_cacheable_declared_action(context, action, &declared).await
    } else {
        materialize_prior_cached_results(workspace, cache, prior_cached_results).await?;
        materialize_available_inputs(workspace, cache, &declared, available_inputs)
            .await
            .with_context(|| {
                format!(
                "materializing inputs for action {index} for {target_id} ({identifier_for_error})"
            )
            })?;
        prepare_declared_command_paths(workspace, &declared)
            .await
            .with_context(|| {
                format!("preparing action {index} for {target_id} ({identifier_for_error})")
            })?;
        run_uncacheable_declared_action(context, action, declared.inherit_parent_env).await
    }
}

async fn prepare_declared_command_paths(workspace: &Path, declared: &DeclaredAction) -> Result<()> {
    if declared.operation.is_some() {
        return Ok(());
    }
    for path in &declared.clean_paths {
        let path = workspace_path(path, "clean_paths entry")?;
        remove_declared_path_if_exists(&path.resolve(workspace), path.as_str()).await?;
    }
    for path in &declared.create_dirs {
        let path = workspace_path(path, "create_dirs entry")?;
        tokio::fs::create_dir_all(path.resolve(workspace))
            .await
            .with_context(|| format!("creating declared command directory `{}`", path.as_str()))?;
    }
    Ok(())
}

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
    let outcome = resolve_cacheable_declared_action(&context, &action, declared).await?;
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
        anyhow::bail!(
            "{}",
            declared_action_failure_message(DeclaredActionFailure {
                cache: context.cache,
                identifier: context.identifier,
                index: context.index,
                target: context.target_id,
                exit_code,
                argv: context.argv,
                arg_files: context.arg_files,
                result: &outcome.result,
            })
            .await
        );
    }
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
    })
}

async fn resolve_cacheable_declared_action(
    context: &DeclaredActionContext<'_>,
    action: &Action,
    declared: &DeclaredAction,
) -> Result<once_core::Outcome> {
    // clean_paths and create_dirs model the command's own filesystem
    // setup, so they must only run when the command actually executes.
    // A cache hit does not run command setup. Removing a clean_paths entry on
    // a hit could delete an unrelated path with no command left to recreate it.
    let cached = context
        .cache
        .get_action_result(&action.digest())
        .await
        .with_context(|| {
            format!(
                "probing cache for action {} for {} ({})",
                context.index, context.target_id, context.identifier
            )
        })?;
    let action_digest = action.digest();
    let outcome = if let Some(result) = cached {
        if action_result_blobs_present(
            &result,
            context.workspace,
            context.cache,
            context.source_digest_cache,
        )
        .await?
        {
            once_core::Outcome {
                action: action_digest,
                result,
                cache: once_core::CacheState::Hit,
            }
        } else {
            execute_declared_cache_miss(context, action, declared).await?
        }
    } else {
        execute_declared_cache_miss(context, action, declared).await?
    };
    if outcome.cache == once_core::CacheState::Miss {
        if let Some(cache) = context.source_digest_cache {
            cache.record_outputs(&outcome.result, context.workspace);
        }
    }
    Ok(outcome)
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

async fn execute_declared_cache_miss(
    context: &DeclaredActionContext<'_>,
    action: &Action,
    declared: &DeclaredAction,
) -> Result<once_core::Outcome> {
    let _permit = context
        .resources
        .acquire(action.resource_request().clone())
        .await;
    materialize_prior_cached_results(
        context.workspace,
        context.cache,
        context.prior_cached_results,
    )
    .await
    .with_context(|| {
        format!(
            "materializing prior outputs for action {} for {} ({})",
            context.index, context.target_id, context.identifier
        )
    })?;
    materialize_available_inputs(
        context.workspace,
        context.cache,
        declared,
        context.available_inputs,
    )
    .await
    .with_context(|| {
        format!(
            "materializing inputs for action {} for {} ({})",
            context.index, context.target_id, context.identifier
        )
    })?;
    prepare_declared_command_paths(context.workspace, declared)
        .await
        .with_context(|| {
            format!(
                "preparing action {} for {} ({})",
                context.index, context.target_id, context.identifier
            )
        })?;
    let result = once_core::run_uncached(action, context.workspace, context.cache, false)
        .await
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

async fn materialize_prior_cached_results(
    workspace: &Path,
    cache: &CacheProvider,
    results: &[ActionResult],
) -> Result<()> {
    for result in results {
        once_core::materialize_outputs(result, workspace, cache)
            .await
            .map_err(anyhow::Error::from)?;
    }
    Ok(())
}

async fn materialize_available_inputs(
    workspace: &Path,
    cache: &CacheProvider,
    declared: &DeclaredAction,
    available_inputs: &BTreeMap<String, AvailableInput>,
) -> Result<()> {
    let declared_inputs = declared
        .inputs
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let outputs = available_inputs
        .iter()
        .filter(|(path, input)| {
            !input.same_target && !input.materialized && declared_inputs.contains(path.as_str())
        })
        .map(|(path, input)| (path.clone(), input.blob_digest))
        .collect::<BTreeMap<_, _>>();
    if outputs.is_empty() {
        return Ok(());
    }
    once_core::materialize_outputs(
        &ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs,
        },
        workspace,
        cache,
    )
    .await
    .map_err(anyhow::Error::from)
}

async fn run_uncacheable_declared_action(
    context: DeclaredActionContext<'_>,
    action: Action,
    inherit_parent_env: bool,
) -> Result<DeclaredActionOutcome> {
    let action_digest = action.digest();
    let _permit = context
        .resources
        .acquire(action.resource_request().clone())
        .await;
    let result = run_uncached_action(
        &action,
        context.workspace,
        context.cache,
        inherit_parent_env,
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
        anyhow::bail!(
            "{}",
            declared_action_failure_message(DeclaredActionFailure {
                cache: context.cache,
                identifier: context.identifier,
                index: context.index,
                target: context.target_id,
                exit_code,
                argv: context.argv,
                arg_files: context.arg_files,
                result: &result,
            })
            .await
        );
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

async fn run_uncached_action(
    action: &Action,
    workspace: &Path,
    cache: &CacheProvider,
    inherit_parent_env: bool,
) -> Result<ActionResult> {
    match action {
        Action::RunCommand {
            argv,
            env,
            cwd,
            timeout_ms,
            outputs,
            stdout_path,
            stderr_path,
            sandbox,
            ..
        } => {
            if *sandbox != SandboxMode::Off {
                if inherit_parent_env {
                    anyhow::bail!(
                        "inherit_parent_env is available only when the action sandbox is off"
                    );
                }
                return once_core::run_uncached(action, workspace, cache, false)
                    .await
                    .map_err(Into::into);
            }
            let argv = resolve_execution_argv(argv, workspace);
            let env = resolve_execution_env(env, workspace);
            let (program, rest) = argv
                .split_first()
                .ok_or_else(|| anyhow::anyhow!("declared action has empty argv"))?;
            let mut command = Command::new(program);
            command.args(rest);
            if !inherit_parent_env {
                command.env_clear();
            }
            for (key, value) in &env {
                command.env(key, value);
            }
            command.stdin(Stdio::null());
            command.current_dir(
                cwd.as_ref()
                    .map_or_else(|| workspace.to_path_buf(), |path| path.resolve(workspace)),
            );
            command.kill_on_drop(true);

            let mut result = if stdout_path.is_some() || stderr_path.is_some() {
                run_uncached_redirected(
                    command,
                    stdout_path.as_deref(),
                    stderr_path.as_deref(),
                    *timeout_ms,
                    workspace,
                    cache,
                )
                .await?
            } else {
                // Uncacheable commands discard stdout rather than buffering
                // it; only stderr is retained for failure diagnostics.
                command.stdout(Stdio::null());
                command.stderr(Stdio::piped());
                let mut child = command.spawn().context("spawning declared action")?;
                let stderr_pipe = child
                    .stderr
                    .take()
                    .context("capturing declared action stderr")?;
                let wait = async {
                    let stderr = cache.put_stream(stderr_pipe).await?;
                    let status = child.wait().await?;
                    Ok::<_, anyhow::Error>((status, stderr))
                };
                let (status, stderr) = match timeout_ms {
                    Some(ms) => tokio::time::timeout(Duration::from_millis(*ms), wait)
                        .await
                        .with_context(|| format!("declared action timed out after {ms}ms"))??,
                    None => wait.await?,
                };
                ActionResult {
                    exit_code: status.code().unwrap_or(-1),
                    stdout: None,
                    stderr: Some(stderr),
                    outputs: BTreeMap::new(),
                }
            };
            if action.accepts_exit_code(result.exit_code) {
                result.outputs = capture_uncached_outputs(outputs, workspace, cache).await?;
                result.exit_code = 0;
            }
            Ok(result)
        }
        Action::WriteFile { .. }
        | Action::CopyPath { .. }
        | Action::LinkPath { .. }
        | Action::MaterializeHostFile { .. }
        | Action::MaterializeHostTree { .. }
        | Action::PreparePath { .. }
        | Action::WriteTreeDigest { .. }
        | Action::WriteArchive { .. } => once_core::run_uncached(action, workspace, cache, false)
            .await
            .map_err(Into::into),
    }
}

/// Run an uncacheable command whose stdout and/or stderr are redirected
/// into declared output files. Redirected streams go straight to disk and
/// are captured later as ordinary outputs; a non-redirected stderr is
/// still piped and retained for failure diagnostics.
async fn run_uncached_redirected(
    mut command: Command,
    stdout_path: Option<&WorkspacePath>,
    stderr_path: Option<&WorkspacePath>,
    timeout_ms: Option<u64>,
    workspace: &Path,
    cache: &CacheProvider,
) -> Result<ActionResult> {
    let stdout_file = match stdout_path {
        Some(path) => Some(open_redirect_file(path, workspace)?),
        None => None,
    };
    let stderr_file = match stderr_path {
        Some(path) => Some(if stdout_path == Some(path) {
            stdout_file
                .as_ref()
                .expect("stdout redirect open when stderr merges into it")
                .try_clone()
                .with_context(|| format!("cloning redirect handle for `{}`", path.as_str()))?
        } else {
            open_redirect_file(path, workspace)?
        }),
        None => None,
    };
    // A redirected stdout goes to its file; otherwise it is discarded.
    command.stdout(stdout_file.map_or_else(Stdio::null, Stdio::from));
    let capture_stderr = stderr_file.is_none();
    command.stderr(stderr_file.map_or_else(Stdio::piped, Stdio::from));

    let mut child = command
        .spawn()
        .context("spawning redirected declared action")?;
    let stderr_pipe = child.stderr.take();
    let wait = async {
        let stderr_blob = match stderr_pipe {
            Some(pipe) => Some(cache.put_stream(pipe).await?),
            None => None,
        };
        let status = child.wait().await?;
        Ok::<_, anyhow::Error>((status, stderr_blob))
    };
    let (status, stderr_blob) = match timeout_ms {
        Some(ms) => tokio::time::timeout(Duration::from_millis(ms), wait)
            .await
            .with_context(|| format!("declared action timed out after {ms}ms"))??,
        None => wait.await?,
    };
    debug_assert!(capture_stderr == stderr_blob.is_some());
    Ok(ActionResult {
        exit_code: status.code().unwrap_or(-1),
        stdout: None,
        stderr: stderr_blob,
        outputs: BTreeMap::new(),
    })
}

fn open_redirect_file(path: &WorkspacePath, workspace: &Path) -> Result<std::fs::File> {
    let absolute = path.resolve(workspace);
    if let Some(parent) = absolute.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("creating parent directory for redirect `{}`", path.as_str())
        })?;
    }
    std::fs::File::create(&absolute)
        .with_context(|| format!("creating redirect output `{}`", path.as_str()))
}

async fn capture_uncached_outputs(
    outputs: &[WorkspacePath],
    workspace: &Path,
    cache: &CacheProvider,
) -> Result<BTreeMap<String, Digest>> {
    let mut captured = BTreeMap::new();
    for output in outputs {
        let absolute = output.resolve(workspace);
        let metadata = match tokio::fs::symlink_metadata(&absolute).await {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                anyhow::bail!(
                    "declared action completed without producing output `{}`",
                    output.as_str()
                );
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("reading declared action output `{}`", output.as_str())
                });
            }
        };
        if metadata.file_type().is_symlink() {
            let target = tokio::fs::read_link(&absolute).await.with_context(|| {
                format!("reading declared output symlink `{}`", output.as_str())
            })?;
            let manifest = format!("once.symlink_output.v1\n{}\n", target.to_string_lossy());
            captured.insert(
                output.as_str().to_string(),
                cache.put_blob(manifest.as_bytes()).await?,
            );
            continue;
        }
        if metadata.is_dir() {
            let manifest = tokio::task::spawn_blocking({
                let absolute = absolute.clone();
                move || directory_manifest_bytes(&absolute)
            })
            .await
            .context("joining declared directory output capture")??;
            captured.insert(
                output.as_str().to_string(),
                cache.put_blob(&manifest).await?,
            );
            continue;
        }
        let file = tokio::fs::File::open(&absolute)
            .await
            .with_context(|| format!("opening declared action output `{}`", output.as_str()))?;
        captured.insert(output.as_str().to_string(), cache.put_stream(file).await?);
    }
    Ok(captured)
}

fn directory_manifest_bytes(root: &Path) -> Result<Vec<u8>> {
    let mut entries = Vec::new();
    collect_directory_manifest(root, root, &mut entries)?;
    let mut manifest = b"once.directory_output.v1\n".to_vec();
    for entry in entries {
        manifest.extend_from_slice(entry.as_bytes());
        manifest.push(b'\n');
    }
    Ok(manifest)
}

fn collect_directory_manifest(root: &Path, dir: &Path, entries: &mut Vec<String>) -> Result<()> {
    let mut children = std::fs::read_dir(dir)
        .with_context(|| format!("reading declared directory output `{}`", dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("reading declared directory output `{}`", dir.display()))?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let path = child.path();
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path.as_path())
            .to_string_lossy()
            .replace('\\', "/");
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("reading declared output metadata `{}`", path.display()))?;
        if metadata.is_dir() {
            entries.push(format!("dir\t{relative}"));
            collect_directory_manifest(root, &path, entries)?;
        } else if metadata.is_file() {
            let file = std::fs::File::open(&path)
                .with_context(|| format!("reading declared output file `{}`", path.display()))?;
            let digest = Digest::of_reader(std::io::BufReader::new(file))
                .with_context(|| format!("hashing declared output file `{}`", path.display()))?;
            entries.push(format!("file\t{relative}\t{digest}"));
        } else if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(&path)
                .with_context(|| format!("reading declared output symlink `{}`", path.display()))?;
            entries.push(format!(
                "symlink\t{relative}\t{}",
                Digest::of_bytes(target.to_string_lossy().as_bytes())
            ));
        }
    }
    Ok(())
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

fn operation_to_action(operation: DeclaredActionOperation, input_digest: Digest) -> Result<Action> {
    Ok(match operation {
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
            format,
        } => archive_to_action(
            entries,
            &output,
            sha256_output.as_deref(),
            format,
            input_digest,
        )?,
    })
}

fn archive_to_action(
    entries: Vec<once_frontend::analysis::DeclaredArchiveEntry>,
    output: &str,
    sha256_output: Option<&str>,
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
                },
                source: entry
                    .source
                    .as_deref()
                    .map(|source| workspace_path(source, "write_archive entry source"))
                    .transpose()?,
                path: entry.path,
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
        format: match format {
            DeclaredArchiveFormat::Tar => ArchiveFormat::Tar,
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

fn ensure_output_parent_dirs(workspace: &Path, outputs: &[WorkspacePath]) -> Result<()> {
    for output in outputs {
        let absolute = output.resolve(workspace);
        if let Some(parent) = absolute.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("creating parent directory for output `{}`", output.as_str())
            })?;
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

fn compose_input_fingerprint_with_available(
    workspace: &Path,
    declared: &DeclaredAction,
    module_source_digest: Digest,
    dep_action_digests: &[(String, Digest)],
    available_inputs: &BTreeMap<String, AvailableInput>,
    source_digest_cache: Option<&SourceDigestCache>,
) -> Result<InputFingerprintManifest> {
    let mut builder = InputDigestBuilder::new(b"once.declared_action.input.v2\0");
    builder.push_keyed_component(
        "module",
        "target-kind-source",
        b"rules",
        &module_source_digest,
    );
    push_declared_action_metadata(&mut builder, declared)?;

    let mut sorted_inputs = declared
        .inputs
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    sorted_inputs.sort_unstable();
    sorted_inputs.dedup();
    for input in &sorted_inputs {
        if let Some(available) = available_inputs.get(*input) {
            let digest = if available.same_target {
                &available.blob_digest
            } else {
                &available.producer_action_digest
            };
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

    let mut dep_order = (0..dep_action_digests.len()).collect::<Vec<_>>();
    dep_order.sort_unstable_by(|&a, &b| dep_action_digests[a].0.cmp(&dep_action_digests[b].0));
    for index in dep_order {
        let (label, digest) = &dep_action_digests[index];
        let key = format!("dep:{label}");
        builder.push_keyed_component("dependency", label, key.as_bytes(), digest);
    }
    Ok(builder.finish_with_fingerprint())
}

fn push_declared_action_metadata(
    builder: &mut InputDigestBuilder,
    declared: &DeclaredAction,
) -> Result<()> {
    if let Some(identity) = &declared.toolchain_identity {
        builder.push_bytes_component("toolchain", "identity", identity.as_bytes());
    }
    if let Some(identifier) = &declared.identifier {
        builder.push_bytes_component("action", "identifier", identifier.as_bytes());
    }
    if let Some(operation) = &declared.operation {
        let encoded =
            serde_json::to_vec(operation).context("serializing declared action operation")?;
        builder.push_bytes_component("command", "operation", &encoded);
    }
    for arg in &declared.argv {
        builder.push_bytes(arg.as_bytes());
    }
    if !declared.argv.is_empty() {
        let encoded =
            serde_json::to_vec(&declared.argv).context("serializing declared action arguments")?;
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
    for (key, value) in &declared.env {
        builder.push_bytes(key.as_bytes());
        builder.push_bytes(value.as_bytes());
    }
    if !declared.env.is_empty() {
        let encoded =
            serde_json::to_vec(&declared.env).context("serializing declared environment")?;
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn capability_outputs_are_sorted_and_deduplicated() {
        let mut outputs = vec![
            ".once/out/Root/run".to_string(),
            ".once/out/Root/run/stdout.log".to_string(),
            ".once/out/Root/run".to_string(),
        ];

        deduplicate_outputs(&mut outputs);

        assert_eq!(
            outputs,
            [".once/out/Root/run", ".once/out/Root/run/stdout.log"]
        );
    }

    #[test]
    fn tool_execution_wraps_command_actions_only() {
        let command: DeclaredAction = serde_json::from_value(serde_json::json!({
            "argv": ["rustc", "--version"],
            "outputs": []
        }))
        .unwrap();
        let portable: DeclaredAction = serde_json::from_value(serde_json::json!({
            "operation": {
                "kind": "write_file",
                "path": "out.txt",
                "bytes": [111, 107]
            },
            "outputs": ["out.txt"]
        }))
        .unwrap();
        let mut actions = vec![command, portable];

        apply_tool_execution(
            &[
                "/once/mise".to_string(),
                "exec".to_string(),
                "--".to_string(),
            ],
            &BTreeMap::from([
                ("MISE_ENABLE_TOOLS".to_string(), "rust".to_string()),
                ("PATH".to_string(), "/opt/rust/bin:/usr/bin".to_string()),
            ]),
            &mut actions,
        );

        assert_eq!(
            actions[0].argv,
            ["/once/mise", "exec", "--", "rustc", "--version"]
        );
        assert_eq!(
            actions[0].env.get("MISE_ENABLE_TOOLS").map(String::as_str),
            Some("rust")
        );
        assert_eq!(
            actions[0].env.get("PATH").map(String::as_str),
            Some("/opt/rust/bin:/usr/bin")
        );
        assert!(actions[1].argv.is_empty());
        assert!(actions[1].env.is_empty());
    }

    #[test]
    fn tool_execution_merges_action_path_ahead_of_tool_path() {
        let command: DeclaredAction = serde_json::from_value(serde_json::json!({
            "argv": ["rustc", "--version"],
            "env": {"PATH": "/target/runtime:/opt/rust/bin"},
            "outputs": []
        }))
        .unwrap();
        let mut actions = vec![command];

        apply_tool_execution(
            &[
                "/once/mise".to_string(),
                "exec".to_string(),
                "--".to_string(),
            ],
            &BTreeMap::from([("PATH".to_string(), "/opt/rust/bin:/usr/bin".to_string())]),
            &mut actions,
        );

        // The action's own directory survives, the shared entry is not
        // duplicated, and the mise directory is appended.
        assert_eq!(
            actions[0].env.get("PATH").map(String::as_str),
            Some("/target/runtime:/opt/rust/bin:/usr/bin")
        );
    }

    #[test]
    fn host_executable_fallback_resolves_the_relevant_path_entry() {
        let directory = tempfile::tempdir().unwrap();
        let filename = if cfg!(windows) {
            "fallback-tool.exe"
        } else {
            "fallback-tool"
        };
        let executable = directory.path().join(filename);
        std::fs::write(&executable, b"tool").unwrap();
        let paths = env::join_paths([directory.path()]).unwrap();

        assert_eq!(
            host_executable_path_in("fallback-tool", &paths, Some(OsStr::new(".COM;.EXE;.CMD"))),
            Some(executable)
        );
    }

    fn module_digest() -> Digest {
        Digest::of_bytes(b"modules")
    }

    fn test_resources() -> &'static Arc<ResourcePool> {
        static RESOURCES: std::sync::LazyLock<Arc<ResourcePool>> = std::sync::LazyLock::new(|| {
            Arc::new(ResourcePool::new(once_core::ResourceLimits::new(256, 0)))
        });
        &RESOURCES
    }

    #[test]
    fn missing_declared_inputs_reports_only_absent_paths() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("present.rs"), b"fn main() {}").unwrap();
        let declared = DeclaredAction {
            operation: None,
            argv: vec!["tool".to_string()],
            arg_files: Vec::new(),
            inputs: vec![
                "present.rs".to_string(),
                ".once/out/x/generated.rlib".to_string(),
            ],
            outputs: Vec::new(),
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: true,
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            toolchain_identity: None,
            identifier: None,
        };

        assert_eq!(
            missing_declared_inputs(workspace.path(), &declared),
            vec![".once/out/x/generated.rlib".to_string()]
        );
    }

    #[tokio::test]
    async fn same_target_outputs_materialize_before_a_later_miss() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let blob = cache.put_blob(b"generated").await.unwrap();
        let path = ".once/tmp/analysis/generated.txt";
        let result = ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs: BTreeMap::from([(path.to_string(), blob)]),
        };

        materialize_prior_cached_results(workspace.path(), &cache, &[result])
            .await
            .unwrap();

        assert_eq!(
            std::fs::read(workspace.path().join(path)).unwrap(),
            b"generated"
        );
    }

    #[test]
    fn declared_action_uses_direct_argv_and_creates_output_parents() {
        let workspace = tempfile::tempdir().unwrap();
        let declared = DeclaredAction {
            operation: None,
            argv: vec!["tool".to_string(), "--version".to_string()],
            arg_files: Vec::new(),
            inputs: Vec::new(),
            outputs: vec![
                ".once/out/x/A.out".to_string(),
                ".once/out/x/sub/B.meta".to_string(),
            ],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: vec![".once/tmp/work".to_string()],
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: true,
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            toolchain_identity: None,
            identifier: None,
        };

        let action = declared_to_action(
            workspace.path(),
            &declared,
            module_digest(),
            &[],
            SandboxMode::default(),
        )
        .unwrap();

        assert!(workspace.path().join(".once/out/x").is_dir());
        assert!(workspace.path().join(".once/out/x/sub").is_dir());
        let Action::RunCommand { argv, inputs, .. } = action else {
            panic!("command declaration should lower to RunCommand");
        };
        assert_eq!(argv, vec!["tool".to_string(), "--version".to_string()]);
        assert!(inputs
            .iter()
            .any(|input| input.as_str() == ".once/tmp/work"));
    }

    #[tokio::test]
    async fn declared_command_setup_cleans_paths_and_creates_dirs() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(workspace.path().join(".once/out/tree/sub")).unwrap();
        std::fs::write(
            workspace.path().join(".once/out/tree/sub/stale.txt"),
            b"stale",
        )
        .unwrap();
        std::fs::write(workspace.path().join(".once/out/stale-file.txt"), b"stale").unwrap();
        let declared = DeclaredAction {
            operation: None,
            argv: vec!["tool".to_string()],
            arg_files: Vec::new(),
            inputs: Vec::new(),
            outputs: vec![".once/out/tree".to_string()],
            stdout: None,
            stderr: None,
            clean_paths: vec![
                ".once/out/tree".to_string(),
                ".once/out/stale-file.txt".to_string(),
            ],
            create_dirs: vec![".once/out/tree".to_string(), ".once/tmp/home".to_string()],
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: true,
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            toolchain_identity: None,
            identifier: None,
        };

        prepare_declared_command_paths(workspace.path(), &declared)
            .await
            .unwrap();

        assert!(workspace.path().join(".once/out/tree").is_dir());
        assert!(!workspace.path().join(".once/out/tree/sub").exists());
        assert!(!workspace.path().join(".once/out/stale-file.txt").exists());
        assert!(workspace.path().join(".once/tmp/home").is_dir());
    }

    #[test]
    fn materialize_declared_arg_files_writes_line_delimited_args() {
        let workspace = tempfile::tempdir().unwrap();
        let arg_files = vec![DeclaredArgFile {
            path: ".once/out/tool/args.txt".to_string(),
            format: DeclaredArgFileFormat::LineDelimited,
            args: vec!["--flag".to_string(), "value with spaces".to_string()],
        }];

        materialize_declared_arg_files(workspace.path(), &arg_files).unwrap();

        assert_eq!(
            std::fs::read_to_string(workspace.path().join(".once/out/tool/args.txt")).unwrap(),
            "--flag\nvalue with spaces\n"
        );
    }

    #[test]
    fn materialize_declared_arg_files_rejects_newline_args() {
        let workspace = tempfile::tempdir().unwrap();
        let arg_files = vec![DeclaredArgFile {
            path: ".once/out/tool/args.txt".to_string(),
            format: DeclaredArgFileFormat::LineDelimited,
            args: vec!["value\n--flag".to_string()],
        }];

        let err = materialize_declared_arg_files(workspace.path(), &arg_files)
            .unwrap_err()
            .to_string();

        assert!(err.contains("contains an argument with a newline"), "{err}");
    }

    #[tokio::test]
    async fn declared_action_failure_message_appends_captured_output() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let stdout = cache.put_blob(b"visible stdout").await.unwrap();
        let stderr = cache.put_blob(b"visible stderr").await.unwrap();
        let result = ActionResult {
            exit_code: 7,
            stdout: Some(stdout),
            stderr: Some(stderr),
            outputs: BTreeMap::new(),
        };

        let message = declared_action_failure_message(DeclaredActionFailure {
            cache: &cache,
            identifier: "target:action",
            index: 2,
            target: "target",
            exit_code: 7,
            argv: &[],
            arg_files: &[],
            result: &result,
        })
        .await;

        assert!(message.contains("target:action (2) failed for target with exit code 7"));
        assert!(message.contains("stdout:\nvisible stdout"));
        assert!(message.contains("stderr:\nvisible stderr"));
    }

    #[tokio::test]
    async fn declared_action_failure_message_truncates_large_output() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let mut bytes = b"drop-me".to_vec();
        bytes.extend(std::iter::repeat_n(b'x', FAILURE_OUTPUT_LIMIT));
        let stdout = cache.put_blob(&bytes).await.unwrap();
        let result = ActionResult {
            exit_code: 1,
            stdout: Some(stdout),
            stderr: None,
            outputs: BTreeMap::new(),
        };

        let message = declared_action_failure_message(DeclaredActionFailure {
            cache: &cache,
            identifier: "id",
            index: 0,
            target: "target",
            exit_code: 1,
            argv: &[],
            arg_files: &[],
            result: &result,
        })
        .await;

        assert!(message.contains("last 16384 bytes of stdout:\n"));
        assert!(!message.contains("drop-me"));
        assert!(message.ends_with(&"x".repeat(FAILURE_OUTPUT_LIMIT)));
    }

    #[tokio::test]
    async fn declared_action_failure_message_appends_arg_file_context() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let result = ActionResult {
            exit_code: 1,
            stdout: None,
            stderr: None,
            outputs: BTreeMap::new(),
        };
        let mut response_args = vec![
            "--name".to_string(),
            "app".to_string(),
            "--input".to_string(),
            ".once/out/deps/dep.bin".to_string(),
            "--search".to_string(),
            ".once/out/deps".to_string(),
        ];
        response_args.extend((0..60).map(|index| format!("arg-{index}")));
        response_args.push("src/input.txt".to_string());
        let arg_files = vec![DeclaredArgFile {
            path: ".once/tmp/analysis/app/tool.args".to_string(),
            format: DeclaredArgFileFormat::LineDelimited,
            args: response_args,
        }];

        let command_argv = vec![
            "tool".to_string(),
            "--config".to_string(),
            ".once/out/deps/config.json".to_string(),
            "@.once/tmp/analysis/app/tool.args".to_string(),
        ];
        let message = declared_action_failure_message(DeclaredActionFailure {
            cache: &cache,
            identifier: "id",
            index: 0,
            target: "target",
            exit_code: 1,
            argv: &command_argv,
            arg_files: &arg_files,
            result: &result,
        })
        .await;

        assert!(message.contains("argv:"));
        assert!(message.contains("first args:\n  tool"));
        assert!(message.contains(".once/out/deps/config.json"));
        assert!(message.contains("arg files:"));
        assert!(message.contains(".once/tmp/analysis/app/tool.args [line-delimited]"));
        assert!(message.contains("first args:\n  --name"));
        assert!(message.contains("last args:"));
        assert!(message.contains("src/input.txt"));
        assert!(!message.contains("extern args:"));
        assert!(!message.contains("dependency search dirs:"));
    }

    #[tokio::test]
    async fn append_captured_output_ignores_missing_digest_and_missing_blob() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let missing = Digest::of_bytes(b"missing");
        let mut message = "base".to_string();

        append_captured_output(&cache, &mut message, "stdout", None).await;
        append_captured_output(&cache, &mut message, "stdout", Some(&missing)).await;

        assert_eq!(message, "base");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uncached_action_executes_each_time() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let action = Action::RunCommand {
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf x >> counter".to_string(),
            ],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: Vec::new(),
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
            remote: None,
        };

        run_uncached_action(&action, workspace.path(), &cache, false)
            .await
            .unwrap();
        run_uncached_action(&action, workspace.path(), &cache, false)
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(workspace.path().join("counter")).unwrap(),
            "xx"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uncached_action_can_inherit_parent_environment() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let output = WorkspacePath::try_from(".once/out/home.txt").unwrap();
        let action = Action::RunCommand {
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf %s \"$HOME\" > .once/out/home.txt".to_string(),
            ],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: vec![output],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
            remote: None,
        };
        std::fs::create_dir_all(workspace.path().join(".once/out")).unwrap();

        run_uncached_action(&action, workspace.path(), &cache, true)
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(workspace.path().join(".once/out/home.txt")).unwrap(),
            std::env::var("HOME").unwrap()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uncached_action_succeeds_when_declared_output_exists() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let action = Action::RunCommand {
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf ok > .once/out/result.txt".to_string(),
            ],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: vec![WorkspacePath::try_from(".once/out/result.txt").unwrap()],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
            remote: None,
        };
        std::fs::create_dir_all(workspace.path().join(".once/out")).unwrap();

        let result = run_uncached_action(&action, workspace.path(), &cache, false)
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.outputs.contains_key(".once/out/result.txt"));
        assert_eq!(
            std::fs::read_to_string(workspace.path().join(".once/out/result.txt")).unwrap(),
            "ok"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uncached_action_discards_stdout_without_buffering_it() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let action = Action::RunCommand {
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf noisy-stdout".to_string(),
            ],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: Vec::new(),
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
            remote: None,
        };

        let result = run_uncached_action(&action, workspace.path(), &cache, false)
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uncached_action_streams_large_stderr_into_the_cache() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let action = Action::RunCommand {
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "dd if=/dev/zero bs=1048576 count=8 1>&2 2>/dev/null".to_string(),
            ],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: Vec::new(),
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: Some(30_000),
            success_exit_codes: vec![0],
            remote: None,
        };

        let result = run_uncached_action(&action, workspace.path(), &cache, false)
            .await
            .unwrap();
        let stderr = cache
            .get_blob(result.stderr.as_ref().expect("stderr digest"))
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(stderr.len(), 8 * 1024 * 1024);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uncached_action_redirects_merged_streams_to_declared_file() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let log = WorkspacePath::try_from(".once/out/run/log.txt").unwrap();
        let action = Action::RunCommand {
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf out; printf err >&2".to_string(),
            ],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: vec![log.clone()],
            stdout_path: Some(Box::new(log.clone())),
            stderr_path: Some(Box::new(log)),
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
            remote: None,
        };

        let result = run_uncached_action(&action, workspace.path(), &cache, false)
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        // Both streams landed in the declared file, not in the result.
        assert_eq!(result.stdout, None);
        assert_eq!(result.stderr, None);
        assert!(result.outputs.contains_key(".once/out/run/log.txt"));
        let on_disk =
            std::fs::read_to_string(workspace.path().join(".once/out/run/log.txt")).unwrap();
        assert!(on_disk.contains("out"), "log missing stdout: {on_disk:?}");
        assert!(on_disk.contains("err"), "log missing stderr: {on_disk:?}");
    }

    #[tokio::test]
    async fn capture_uncached_outputs_records_directory_tree_manifest() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        std::fs::create_dir_all(workspace.path().join(".once/out/tree/sub")).unwrap();
        std::fs::write(workspace.path().join(".once/out/tree/a.txt"), b"a").unwrap();
        std::fs::write(workspace.path().join(".once/out/tree/sub/b.txt"), b"b").unwrap();

        let outputs = capture_uncached_outputs(
            &[WorkspacePath::try_from(".once/out/tree").unwrap()],
            workspace.path(),
            &cache,
        )
        .await
        .unwrap();
        let digest = outputs.get(".once/out/tree").unwrap();
        let manifest = cache.get_blob(digest).await.unwrap();
        let manifest = String::from_utf8(manifest).unwrap();

        assert!(manifest.starts_with("once.directory_output.v1\n"));
        assert!(manifest.contains("dir\tsub\n"));
        assert!(manifest.contains("file\ta.txt\t"));
        assert!(manifest.contains("file\tsub/b.txt\t"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn capture_uncached_outputs_records_top_level_symlink_without_following_it() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        std::fs::create_dir_all(workspace.path().join(".once/out")).unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();
        std::os::unix::fs::symlink(outside.path(), workspace.path().join(".once/out/link"))
            .unwrap();

        let outputs = capture_uncached_outputs(
            &[WorkspacePath::try_from(".once/out/link").unwrap()],
            workspace.path(),
            &cache,
        )
        .await
        .unwrap();
        let digest = outputs.get(".once/out/link").unwrap();
        let manifest = cache.get_blob(digest).await.unwrap();
        let manifest = String::from_utf8(manifest).unwrap();

        assert!(manifest.starts_with("once.symlink_output.v1\n"));
        assert!(manifest.contains(&outside.path().to_string_lossy().to_string()));
        assert!(!manifest.contains("secret.txt"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn single_action_outcome_preserves_streams_for_capability_evidence() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let target = GraphTarget {
            label: once_frontend::TargetLabel {
                package: "tools".to_string(),
                name: "single".to_string(),
                id: "tools/single".to_string(),
            },
            kind: "demo_kind".to_string(),
            deps: Vec::new(),
            dependency_edges: BTreeMap::new(),
            srcs: Vec::new(),
            visibility: Vec::new(),
            attrs: BTreeMap::new(),
            capabilities: Vec::new(),
            providers: Vec::new(),
            tools: Vec::new(),
            diagnostics: Vec::new(),
        };
        let analysis = AnalysisResult {
            actions: vec![DeclaredAction {
                operation: None,
                argv: vec![
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "printf visible-stdout; printf visible-stderr >&2; printf ok > .once/out/one.txt"
                        .to_string(),
                ],
                arg_files: Vec::new(),
                inputs: Vec::new(),
                outputs: vec![".once/out/one.txt".to_string()],
                stdout: None,
                stderr: None,
                clean_paths: Vec::new(),
                create_dirs: Vec::new(),
                cwd: None,
                env: BTreeMap::new(),
                sandbox: None,
                success_exit_codes: vec![0],
                cacheable: true,
                inherit_parent_env: false,
                depends_on_prior_actions: true,
                toolchain_identity: None,
                identifier: Some("one".to_string()),
            }],
            provider: serde_json::json!({}),
            declared_outputs: Vec::new(),
        };

        let outcome = run_declared_actions(
            workspace.path(),
            &cache,
            module_digest(),
            &target,
            "build",
            analysis,
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
            SandboxMode::default(),
            test_resources(),
        )
        .await
        .unwrap();

        let stdout = cache
            .get_blob(&outcome.result.stdout.unwrap())
            .await
            .unwrap();
        let stderr = cache
            .get_blob(&outcome.result.stderr.unwrap())
            .await
            .unwrap();
        assert_eq!(stdout, b"visible-stdout");
        assert_eq!(stderr, b"visible-stderr");
        assert!(outcome.result.outputs.contains_key(".once/out/one.txt"));
        let records = once_core::EvidenceStore::open_workspace(workspace.path())
            .load()
            .await
            .unwrap();
        assert_eq!(records.len(), 0);
    }

    #[test]
    fn uncacheable_action_makes_whole_target_bypass() {
        use EvidenceCacheState::{Bypass, Hit, Miss};

        // A single uncacheable action forces the target to Bypass so a
        // target that must re-run work is never reported as a cache hit.
        assert_eq!(aggregate_declared_action_cache_state(Bypass, Hit), Bypass);
        assert_eq!(aggregate_declared_action_cache_state(Hit, Bypass), Bypass);
        assert_eq!(aggregate_declared_action_cache_state(Bypass, Miss), Bypass);
        assert_eq!(
            aggregate_declared_action_cache_state(Bypass, Bypass),
            Bypass
        );
        // Without a Bypass action, Miss dominates Hit.
        assert_eq!(aggregate_declared_action_cache_state(Hit, Miss), Miss);
        assert_eq!(aggregate_declared_action_cache_state(Hit, Hit), Hit);
    }

    fn cached_test_target() -> GraphTarget {
        GraphTarget {
            label: once_frontend::TargetLabel {
                package: "tools".to_string(),
                name: "cached".to_string(),
                id: "tools/cached".to_string(),
            },
            kind: "demo_kind".to_string(),
            deps: Vec::new(),
            dependency_edges: BTreeMap::new(),
            srcs: Vec::new(),
            visibility: Vec::new(),
            attrs: BTreeMap::new(),
            capabilities: Vec::new(),
            providers: Vec::new(),
            tools: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn cached_test_analysis(dependency_path: &str) -> AnalysisResult {
        AnalysisResult {
            actions: vec![DeclaredAction {
                operation: None,
                argv: vec![
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "printf ok > .once/out/out.txt".to_string(),
                ],
                arg_files: Vec::new(),
                inputs: vec![dependency_path.to_string()],
                outputs: vec![".once/out/out.txt".to_string()],
                stdout: None,
                stderr: None,
                clean_paths: vec![".once/out/side.txt".to_string()],
                create_dirs: vec![".once/out".to_string()],
                cwd: None,
                env: BTreeMap::new(),
                sandbox: None,
                success_exit_codes: vec![0],
                cacheable: true,
                inherit_parent_env: false,
                depends_on_prior_actions: true,
                toolchain_identity: None,
                identifier: Some("cached".to_string()),
            }],
            provider: serde_json::json!({}),
            declared_outputs: Vec::new(),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cache_hit_skips_command_setup_and_defers_output_materialization() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let source_digest_cache = SourceDigestCache::open(workspace.path());
        let dependency_path = ".once/out/dependency/input.txt";
        let dependency_blob = cache.put_blob(b"dependency").await.unwrap();
        let available_inputs = BTreeMap::from([(
            dependency_path.to_string(),
            AvailableInput {
                blob_digest: dependency_blob,
                producer_action_digest: Digest::of_bytes(b"dependency action"),
                same_target: false,
                materialized: false,
            },
        )]);
        let target = cached_test_target();
        let analysis = || cached_test_analysis(dependency_path);

        // First run misses the cache and executes the command.
        let first = run_declared_actions(
            workspace.path(),
            &cache,
            module_digest(),
            &target,
            "build",
            analysis(),
            &[],
            &available_inputs,
            &BTreeMap::new(),
            Some(&source_digest_cache),
            SandboxMode::default(),
            test_resources(),
        )
        .await
        .unwrap();
        assert_eq!(first.cache_tag, "miss");
        assert!(first.cached_results.is_empty());
        assert_eq!(
            first.available_inputs[dependency_path].blob_digest,
            dependency_blob
        );
        assert!(!first.available_inputs[dependency_path].same_target);
        assert!(
            first.available_inputs[".once/out/out.txt"].same_target,
            "the target's own output stays marked as same-target until a dependent consumes it"
        );
        let dependency = workspace.path().join(dependency_path);
        assert_eq!(std::fs::read(&dependency).unwrap(), b"dependency");
        std::fs::remove_file(&dependency).unwrap();

        // Stand in for content a prior uncached run left behind: it lives
        // under a clean path but is not one of the action's outputs, so a
        // cache hit would not restore it.
        let side = workspace.path().join(".once/out/side.txt");
        std::fs::write(&side, b"precious").unwrap();
        let output = workspace.path().join(".once/out/out.txt");
        std::fs::write(&output, b"stale").unwrap();

        // Second run hits the cache. The command never executes, so its
        // clean_paths must not delete the untracked file.
        let second = run_declared_actions(
            workspace.path(),
            &cache,
            module_digest(),
            &target,
            "build",
            analysis(),
            &[],
            &available_inputs,
            &BTreeMap::new(),
            Some(&source_digest_cache),
            SandboxMode::default(),
            test_resources(),
        )
        .await
        .unwrap();
        assert_eq!(second.cache_tag, "hit");
        assert_eq!(
            second
                .cached_results
                .iter()
                .flat_map(|result| result.outputs.keys())
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [".once/out/out.txt"]
        );
        assert_eq!(std::fs::read(&side).unwrap(), b"precious");
        assert_eq!(std::fs::read(&output).unwrap(), b"stale");
        assert!(!dependency.exists());

        for result in &second.cached_results {
            source_digest_cache
                .materialize_outputs(result, workspace.path(), &cache)
                .await
                .unwrap();
        }
        assert_eq!(std::fs::read(&output).unwrap(), b"ok");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uncacheable_action_materializes_dependency_inputs() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let dependency_path = ".once/out/dependency/input.txt";
        let dependency_blob = cache.put_blob(b"dependency").await.unwrap();
        let available_inputs = BTreeMap::from([(
            dependency_path.to_string(),
            AvailableInput {
                blob_digest: dependency_blob,
                producer_action_digest: Digest::of_bytes(b"dependency action"),
                same_target: false,
                materialized: false,
            },
        )]);
        let target = cached_test_target();
        let mut analysis = cached_test_analysis(dependency_path);
        analysis.actions[0].cacheable = false;
        analysis.actions[0].argv = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            format!("cat {dependency_path} > .once/out/out.txt"),
        ];

        let outcome = run_declared_actions(
            workspace.path(),
            &cache,
            module_digest(),
            &target,
            "build",
            analysis,
            &[],
            &available_inputs,
            &BTreeMap::new(),
            None,
            SandboxMode::default(),
            test_resources(),
        )
        .await
        .unwrap();

        assert_eq!(outcome.cache_tag, "bypass");
        assert_eq!(
            std::fs::read(workspace.path().join(".once/out/out.txt")).unwrap(),
            b"dependency"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn materialized_dependency_inputs_are_not_restored_again() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let dependency_path = ".once/out/dependency/input.txt";
        let dependency = workspace.path().join(dependency_path);
        std::fs::create_dir_all(dependency.parent().unwrap()).unwrap();
        std::fs::write(&dependency, b"ready").unwrap();
        let dependency_blob = cache.put_blob(b"stale").await.unwrap();
        let available_inputs = BTreeMap::from([(
            dependency_path.to_string(),
            AvailableInput {
                blob_digest: dependency_blob,
                producer_action_digest: Digest::of_bytes(b"dependency action"),
                same_target: false,
                materialized: true,
            },
        )]);
        let target = cached_test_target();
        let mut analysis = cached_test_analysis(dependency_path);
        analysis.actions[0].cacheable = false;
        analysis.actions[0].argv = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            format!("cat {dependency_path} > .once/out/out.txt"),
        ];

        run_declared_actions(
            workspace.path(),
            &cache,
            module_digest(),
            &target,
            "build",
            analysis,
            &[],
            &available_inputs,
            &BTreeMap::new(),
            None,
            SandboxMode::default(),
            test_resources(),
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read(&dependency).unwrap(), b"ready");
        assert_eq!(
            std::fs::read(workspace.path().join(".once/out/out.txt")).unwrap(),
            b"ready"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn incomplete_direct_cache_hit_falls_back_to_execution() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cache")));
        let target = cached_test_target();
        let analysis = || {
            let mut analysis = cached_test_analysis("unused");
            analysis.actions[0].inputs.clear();
            analysis
        };

        let first = run_declared_actions(
            workspace.path(),
            &cache,
            module_digest(),
            &target,
            "build",
            analysis(),
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
            SandboxMode::default(),
            test_resources(),
        )
        .await
        .unwrap();
        assert_eq!(first.cache_tag, "miss");

        std::fs::remove_dir_all(cache.root().join("cas")).unwrap();
        std::fs::remove_file(workspace.path().join(".once/out/out.txt")).unwrap();

        let second = run_declared_actions(
            workspace.path(),
            &cache,
            module_digest(),
            &target,
            "build",
            analysis(),
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
            SandboxMode::default(),
            test_resources(),
        )
        .await
        .unwrap();
        assert_eq!(second.cache_tag, "miss");
        assert_eq!(
            std::fs::read(workspace.path().join(".once/out/out.txt")).unwrap(),
            b"ok"
        );

        let third = run_declared_actions(
            workspace.path(),
            &cache,
            module_digest(),
            &target,
            "build",
            analysis(),
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
            SandboxMode::default(),
            test_resources(),
        )
        .await
        .unwrap();
        assert_eq!(third.cache_tag, "hit");
    }

    #[tokio::test]
    async fn action_result_completeness_checks_stream_and_output_blobs() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cache")));
        let present = cache.put_blob(b"present").await.unwrap();
        let missing = Digest::of_bytes(b"missing");
        let mut result = ActionResult {
            exit_code: 0,
            stdout: Some(present),
            stderr: None,
            outputs: BTreeMap::from([("out.txt".to_string(), present)]),
        };

        assert!(
            action_result_blobs_present(&result, workspace.path(), &cache, None)
                .await
                .unwrap()
        );
        result.stderr = Some(missing);
        assert!(
            !action_result_blobs_present(&result, workspace.path(), &cache, None)
                .await
                .unwrap()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn matching_indexed_output_does_not_require_cached_blob() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cache")));
        let output = workspace.path().join("out.txt");
        std::fs::write(&output, b"present").unwrap();
        let result = ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs: BTreeMap::from([(
                "out.txt".to_string(),
                Digest::of_bytes(b"absent from cache"),
            )]),
        };
        let digests = SourceDigestCache::open(workspace.path());
        digests.record_outputs(&result, workspace.path());

        assert!(
            action_result_blobs_present(&result, workspace.path(), &cache, Some(&digests))
                .await
                .unwrap()
        );
    }

    fn assert_declared_action_fingerprints(
        records: &[EvidenceRecord],
        aggregate: Option<InputFingerprintManifest>,
    ) {
        assert!(records
            .iter()
            .all(|record| record.input_fingerprint.is_some()));
        assert!(records.iter().any(|record| {
            record
                .input_fingerprint
                .as_ref()
                .is_some_and(|fingerprint| {
                    fingerprint
                        .components
                        .iter()
                        .any(|component| component.category == "dependency")
                })
        }));
        let aggregate = aggregate.expect("multi-action outcome should aggregate fingerprints");
        assert!(aggregate
            .components
            .iter()
            .any(|component| component.label.starts_with("action:1:")));
        let encoded = serde_json::to_string(&records).unwrap();
        assert!(!encoded.contains("printf one"));
        assert!(!encoded.contains("printf two"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn declared_actions_record_success_evidence_per_action() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let target = GraphTarget {
            label: once_frontend::TargetLabel {
                package: "tools".to_string(),
                name: "demo".to_string(),
                id: "tools/demo".to_string(),
            },
            kind: "demo_kind".to_string(),
            deps: Vec::new(),
            dependency_edges: BTreeMap::new(),
            srcs: Vec::new(),
            visibility: Vec::new(),
            attrs: BTreeMap::new(),
            capabilities: Vec::new(),
            providers: Vec::new(),
            tools: Vec::new(),
            diagnostics: Vec::new(),
        };
        let action = |name: &str| DeclaredAction {
            operation: None,
            argv: vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                format!("printf {name} > .once/out/{name}.txt"),
            ],
            arg_files: Vec::new(),
            inputs: Vec::new(),
            outputs: vec![format!(".once/out/{name}.txt")],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: false,
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            toolchain_identity: None,
            identifier: Some(name.to_string()),
        };
        let analysis = AnalysisResult {
            actions: vec![action("one"), action("two")],
            provider: serde_json::json!({}),
            declared_outputs: Vec::new(),
        };

        let outcome = run_declared_actions(
            workspace.path(),
            &cache,
            module_digest(),
            &target,
            "build",
            analysis,
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
            SandboxMode::default(),
            test_resources(),
        )
        .await
        .unwrap();

        assert_eq!(outcome.result.stdout, None);
        assert_eq!(outcome.result.stderr, None);
        assert_eq!(outcome.result.outputs.len(), 2);
        let records = once_core::EvidenceStore::open_workspace(workspace.path())
            .load()
            .await
            .unwrap();
        assert_eq!(records.len(), 2);
        assert!(records
            .iter()
            .all(|record| record.subject.matches("tools/demo:build")));
        assert!(records
            .iter()
            .any(|record| record.outputs.contains_key(".once/out/one.txt")));
        assert!(records
            .iter()
            .any(|record| record.outputs.contains_key(".once/out/two.txt")));
        assert_declared_action_fingerprints(&records, outcome.input_fingerprint);
    }

    #[cfg(unix)]
    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "inline DeclaredAction literals carry many required fields"
    )]
    async fn declared_action_can_skip_prior_same_target_digests() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let target = GraphTarget {
            label: once_frontend::TargetLabel {
                package: "tools".to_string(),
                name: "split".to_string(),
                id: "tools/split".to_string(),
            },
            kind: "demo_kind".to_string(),
            deps: Vec::new(),
            dependency_edges: BTreeMap::new(),
            srcs: Vec::new(),
            visibility: Vec::new(),
            attrs: BTreeMap::new(),
            capabilities: Vec::new(),
            providers: Vec::new(),
            tools: Vec::new(),
            diagnostics: Vec::new(),
        };

        let analysis = |first_value: &str| AnalysisResult {
            actions: vec![
                DeclaredAction {
                    operation: None,
                    argv: vec![
                        "/bin/sh".to_string(),
                        "-c".to_string(),
                        format!("printf {first_value} > .once/out/first.txt"),
                    ],
                    arg_files: Vec::new(),
                    inputs: Vec::new(),
                    outputs: vec![".once/out/first.txt".to_string()],
                    stdout: None,
                    stderr: None,
                    clean_paths: Vec::new(),
                    create_dirs: Vec::new(),
                    cwd: None,
                    env: BTreeMap::new(),
                    sandbox: None,
                    success_exit_codes: vec![0],
                    cacheable: true,
                    inherit_parent_env: false,
                    depends_on_prior_actions: true,
                    toolchain_identity: None,
                    identifier: Some("first".to_string()),
                },
                DeclaredAction {
                    operation: None,
                    argv: vec![
                        "/bin/sh".to_string(),
                        "-c".to_string(),
                        "printf second > .once/out/second.txt; printf run >> second_runs"
                            .to_string(),
                    ],
                    arg_files: Vec::new(),
                    inputs: Vec::new(),
                    outputs: vec![".once/out/second.txt".to_string()],
                    stdout: None,
                    stderr: None,
                    clean_paths: Vec::new(),
                    create_dirs: Vec::new(),
                    cwd: None,
                    env: BTreeMap::new(),
                    sandbox: None,
                    success_exit_codes: vec![0],
                    cacheable: true,
                    inherit_parent_env: false,
                    depends_on_prior_actions: false,
                    toolchain_identity: None,
                    identifier: Some("second".to_string()),
                },
            ],
            provider: serde_json::json!({}),
            declared_outputs: Vec::new(),
        };

        let first = run_declared_actions(
            workspace.path(),
            &cache,
            module_digest(),
            &target,
            "build",
            analysis("one"),
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
            SandboxMode::default(),
            test_resources(),
        )
        .await
        .unwrap();
        assert_eq!(first.cache_state, EvidenceCacheState::Miss);

        let second = run_declared_actions(
            workspace.path(),
            &cache,
            module_digest(),
            &target,
            "build",
            analysis("changed"),
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
            SandboxMode::default(),
            test_resources(),
        )
        .await
        .unwrap();

        assert_eq!(second.cache_state, EvidenceCacheState::Miss);
        assert_eq!(
            std::fs::read_to_string(workspace.path().join(".once/out/first.txt")).unwrap(),
            "changed"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.path().join(".once/out/second.txt")).unwrap(),
            "second"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("second_runs")).unwrap(),
            "run"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uncached_action_errors_when_declared_output_is_missing() {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(workspace.path().join("cas")));
        let action = Action::RunCommand {
            argv: vec!["/bin/sh".to_string(), "-c".to_string(), ":".to_string()],
            env: BTreeMap::new(),
            cwd: None,
            input_digest: None,
            inputs: vec![],
            outputs: vec![WorkspacePath::try_from(".once/out/missing.txt").unwrap()],
            stdout_path: None,
            stderr_path: None,
            output_symlink_mode: OutputSymlinkMode::default(),
            resources: ResourceRequest::default(),
            sandbox: SandboxMode::default(),
            timeout_ms: None,
            success_exit_codes: vec![0],
            remote: None,
        };

        let err = run_uncached_action(&action, workspace.path(), &cache, false)
            .await
            .unwrap_err()
            .to_string();

        assert!(
            err.contains(
                "declared action completed without producing output `.once/out/missing.txt`"
            ),
            "{err}"
        );
    }

    #[test]
    fn input_digest_changes_with_toolchain_identity() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("input.txt"), b"content").unwrap();
        let declared = DeclaredAction {
            operation: None,
            argv: vec!["tool".to_string()],
            arg_files: Vec::new(),
            inputs: vec!["input.txt".to_string()],
            outputs: vec![".once/out/A.a".to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: true,
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            toolchain_identity: Some("id-1".to_string()),
            identifier: None,
        };
        let one = compose_input_digest(workspace.path(), &declared, module_digest(), &[]).unwrap();
        let declared2 = DeclaredAction {
            toolchain_identity: Some("id-2".to_string()),
            ..declared
        };
        let two = compose_input_digest(workspace.path(), &declared2, module_digest(), &[]).unwrap();
        assert_ne!(one, two);
    }

    #[test]
    fn input_fingerprint_explains_inputs_without_exposing_values() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("input.txt"), b"content").unwrap();
        let declared = DeclaredAction {
            operation: None,
            argv: vec!["tool".to_string(), "--token=command-secret".to_string()],
            arg_files: Vec::new(),
            inputs: vec!["input.txt".to_string()],
            outputs: vec![".once/out/A.a".to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::from([("TOKEN".to_string(), "environment-secret".to_string())]),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: true,
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            toolchain_identity: Some("toolchain-secret".to_string()),
            identifier: Some("compile".to_string()),
        };
        let dependency = Digest::of_bytes(b"dependency");
        let fingerprint = compose_input_fingerprint_with_available(
            workspace.path(),
            &declared,
            module_digest(),
            &[("core".to_string(), dependency)],
            &BTreeMap::new(),
            None,
        )
        .unwrap();
        let digest = compose_input_digest(
            workspace.path(),
            &declared,
            module_digest(),
            &[("core".to_string(), dependency)],
        )
        .unwrap();

        assert_eq!(fingerprint.input_digest, digest);
        for (category, label) in [
            ("module", "target-kind-source"),
            ("toolchain", "identity"),
            ("action", "identifier"),
            ("command", "arguments"),
            ("environment", "declared"),
            ("source", "input.txt"),
            ("dependency", "core"),
        ] {
            assert!(fingerprint
                .components
                .iter()
                .any(|component| { component.category == category && component.label == label }));
        }
        let encoded = serde_json::to_string(&fingerprint).unwrap();
        assert!(!encoded.contains("command-secret"));
        assert!(!encoded.contains("environment-secret"));
        assert!(!encoded.contains("toolchain-secret"));
    }

    #[test]
    fn input_digest_changes_with_command_setup_paths() {
        let workspace = tempfile::tempdir().unwrap();
        let declared = DeclaredAction {
            operation: None,
            argv: vec!["tool".to_string()],
            arg_files: Vec::new(),
            inputs: Vec::new(),
            outputs: vec![".once/out/A.a".to_string()],
            stdout: None,
            stderr: None,
            clean_paths: vec![".once/out/A.a".to_string()],
            create_dirs: vec![".once/tmp/home".to_string()],
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: true,
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            toolchain_identity: None,
            identifier: None,
        };
        let one = compose_input_digest(workspace.path(), &declared, module_digest(), &[]).unwrap();
        let declared2 = DeclaredAction {
            stdout: None,
            stderr: None,
            clean_paths: vec![".once/out/B.a".to_string()],
            ..declared
        };
        let two = compose_input_digest(workspace.path(), &declared2, module_digest(), &[]).unwrap();

        assert_ne!(one, two);
    }

    #[test]
    fn input_digest_changes_with_declared_arg_files() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("input.txt"), b"content").unwrap();
        let declared = DeclaredAction {
            operation: None,
            argv: vec!["tool".to_string(), "@.once/out/args.rsp".to_string()],
            arg_files: vec![DeclaredArgFile {
                path: ".once/out/args.rsp".to_string(),
                format: DeclaredArgFileFormat::LineDelimited,
                args: vec!["--cfg".to_string(), "feature=\"alloc\"".to_string()],
            }],
            inputs: vec!["input.txt".to_string()],
            outputs: vec![".once/out/A.a".to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: true,
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            toolchain_identity: None,
            identifier: None,
        };
        let one = compose_input_digest(workspace.path(), &declared, module_digest(), &[]).unwrap();
        let declared2 = DeclaredAction {
            arg_files: vec![DeclaredArgFile {
                path: ".once/out/args.rsp".to_string(),
                format: DeclaredArgFileFormat::LineDelimited,
                args: vec!["--cfg".to_string(), "feature=\"std\"".to_string()],
            }],
            ..declared
        };
        let two = compose_input_digest(workspace.path(), &declared2, module_digest(), &[]).unwrap();

        assert_ne!(one, two);
    }

    #[test]
    fn input_digest_changes_with_module_source_digest() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("input.txt"), b"content").unwrap();
        let declared = DeclaredAction {
            operation: None,
            argv: vec!["tool".to_string()],
            arg_files: Vec::new(),
            inputs: vec!["input.txt".to_string()],
            outputs: vec![".once/out/A.a".to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: true,
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            toolchain_identity: None,
            identifier: None,
        };
        let one = compose_input_digest(
            workspace.path(),
            &declared,
            Digest::of_bytes(b"modules-1"),
            &[],
        )
        .unwrap();
        let two = compose_input_digest(
            workspace.path(),
            &declared,
            Digest::of_bytes(b"modules-2"),
            &[],
        )
        .unwrap();

        assert_ne!(one, two);
    }

    #[test]
    fn same_target_generated_input_uses_its_content_digest() {
        let workspace = tempfile::tempdir().unwrap();
        let input = ".once/out/fingerprint.txt";
        let declared = DeclaredAction {
            operation: None,
            argv: vec!["tool".to_string()],
            arg_files: Vec::new(),
            inputs: vec![input.to_string()],
            outputs: vec![".once/out/result.txt".to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: true,
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            toolchain_identity: None,
            identifier: None,
        };
        let producer_action_digest = Digest::of_bytes(b"same producer action");
        let available = |content| {
            BTreeMap::from([(
                input.to_string(),
                AvailableInput {
                    blob_digest: Digest::of_bytes(content),
                    producer_action_digest,
                    same_target: true,
                    materialized: false,
                },
            )])
        };

        let first = compose_input_digest_with_available(
            workspace.path(),
            &declared,
            module_digest(),
            &[],
            &available(b"false"),
            None,
        )
        .unwrap();
        let second = compose_input_digest_with_available(
            workspace.path(),
            &declared,
            module_digest(),
            &[],
            &available(b"true"),
            None,
        )
        .unwrap();

        assert_ne!(first, second);
    }

    #[test]
    fn dependency_generated_input_uses_its_action_digest() {
        let workspace = tempfile::tempdir().unwrap();
        let input = ".once/out/dependency.txt";
        let declared = DeclaredAction {
            operation: None,
            argv: vec!["tool".to_string()],
            arg_files: Vec::new(),
            inputs: vec![input.to_string()],
            outputs: vec![".once/out/result.txt".to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: true,
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            toolchain_identity: None,
            identifier: None,
        };
        let producer_action_digest = Digest::of_bytes(b"same producer action");
        let available = |content| {
            BTreeMap::from([(
                input.to_string(),
                AvailableInput {
                    blob_digest: Digest::of_bytes(content),
                    producer_action_digest,
                    same_target: false,
                    materialized: false,
                },
            )])
        };

        let first = compose_input_digest_with_available(
            workspace.path(),
            &declared,
            module_digest(),
            &[],
            &available(b"first remote blob"),
            None,
        )
        .unwrap();
        let second = compose_input_digest_with_available(
            workspace.path(),
            &declared,
            module_digest(),
            &[],
            &available(b"second remote blob"),
            None,
        )
        .unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn input_digest_stable_under_dep_reordering() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("input.txt"), b"content").unwrap();
        let declared = DeclaredAction {
            operation: None,
            argv: vec!["tool".to_string()],
            arg_files: Vec::new(),
            inputs: vec!["input.txt".to_string()],
            outputs: vec![".once/out/A.a".to_string()],
            stdout: None,
            stderr: None,
            clean_paths: Vec::new(),
            create_dirs: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
            sandbox: None,
            success_exit_codes: vec![0],
            cacheable: true,
            inherit_parent_env: false,
            depends_on_prior_actions: true,
            toolchain_identity: None,
            identifier: None,
        };
        let a = compose_input_digest(
            workspace.path(),
            &declared,
            module_digest(),
            &[
                ("dep1".to_string(), Digest::of_bytes(b"d1")),
                ("dep2".to_string(), Digest::of_bytes(b"d2")),
            ],
        )
        .unwrap();
        let b = compose_input_digest(
            workspace.path(),
            &declared,
            module_digest(),
            &[
                ("dep2".to_string(), Digest::of_bytes(b"d2")),
                ("dep1".to_string(), Digest::of_bytes(b"d1")),
            ],
        )
        .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn target_action_digest_preserves_single_action_digest() {
        let action = Digest::of_bytes(b"action");

        assert_eq!(compose_target_action_digest("Root", &[action]), action);
    }

    #[test]
    fn target_action_digest_for_empty_actions_is_target_specific() {
        let root = compose_target_action_digest("Root", &[]);
        let same_root = compose_target_action_digest("Root", &[]);
        let other = compose_target_action_digest("Other", &[]);

        assert_eq!(root, same_root);
        assert_ne!(root, other);
    }

    #[test]
    fn target_action_digest_includes_all_declared_actions_in_order() {
        let first = Digest::of_bytes(b"first");
        let second = Digest::of_bytes(b"second");
        let changed_second = Digest::of_bytes(b"changed-second");

        let original = compose_target_action_digest("Root", &[first, second]);
        let changed = compose_target_action_digest("Root", &[first, changed_second]);
        let reordered = compose_target_action_digest("Root", &[second, first]);

        assert_ne!(original, changed);
        assert_ne!(original, reordered);
    }

    #[test]
    fn target_input_digest_handles_empty_single_and_multiple_inputs() {
        let first = Digest::of_bytes(b"first-input");
        let second = Digest::of_bytes(b"second-input");

        assert_eq!(compose_target_input_digest(&[]), None);
        assert_eq!(compose_target_input_digest(&[first]), Some(first));

        let original = compose_target_input_digest(&[first, second]).unwrap();
        let same = compose_target_input_digest(&[first, second]).unwrap();
        let reordered = compose_target_input_digest(&[second, first]).unwrap();

        assert_eq!(original, same);
        assert_ne!(original, reordered);
    }
}
