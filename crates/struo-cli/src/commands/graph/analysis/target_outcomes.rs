//! Remember what building one target produced, so a target nothing touched is
//! not walked again.
//!
//! An invocation that recompiles one file still visits every target the root
//! depends on: reads its stored analysis, checks the answers that analysis got,
//! digests its declared inputs, composes an action digest, and asks the store
//! whether that action already ran. Each step is small and there are hundreds
//! of them, and for all but the handful of targets the edit reached, every one
//! of them arrives at the answer it arrived at last time.
//!
//! What makes skipping the visit sound is that a target's outcome follows from
//! its definition, its dependencies' outcomes, and the files it reads. The
//! first two are folded into the name below, so a dependency that rebuilt
//! changes the name of everything above it. The third is settled by the
//! filesystem watcher: it reports every workspace path that changed since the
//! record was written, and a record names both the files its actions declared
//! and the patterns its analysis expanded, so a file edited, created, or
//! deleted under either is a target that has to be visited.
//!
//! Without a watcher there is nothing to settle the third with, so every target
//! is visited. An outcome that must run on every invocation, because one of its
//! actions declined to be cached, is never recorded at all.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use struo_cas::{ActionResult, Digest};
use struo_core::{EvidenceCacheState, FingerprintStore, SandboxMode};
use struo_frontend::analysis::{AnalysisObservations, DeclaredInputs, Observation, TransitiveSets};
use struo_frontend::GraphTarget;

use super::source_digest_cache::KnownChanges;
use super::{AvailableInput, BuildOutcome};
use crate::commands::change_tracker::ChangePosition;

// Older outcomes kept every record in one file, which each build read whole.
const SCHEMA: &str = "struo.target-outcomes.v10";
const INDEX: &str = "index.json";
const RECORDS: &str = "records";

/// A pattern set one target's analysis expanded, and where it was anchored.
///
/// The kind is recorded because it decides how a changed path is matched: a
/// walk is handed a directory and owns everything under it, while a glob owns
/// what its patterns select.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct Expansion {
    kind: String,
    package: String,
    patterns: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Record {
    /// Names the build: same name, same target definition and same dependency
    /// outcomes.
    key: String,
    /// Workspace paths the target's actions declared as inputs, excluding
    /// dependency outputs, which the name already accounts for.
    sources: DeclaredInputs,
    observations: AnalysisObservations,
    /// What its analysis expanded, so a file that appears where one of these
    /// patterns would have selected it counts as a change.
    expansions: Vec<Expansion>,
    outputs: Vec<String>,
    provider: JsonValue,
    action_digest: Digest,
    input_digest: Option<Digest>,
    input_fingerprint: Option<JsonValue>,
    /// How the outputs the target left available differ from what its
    /// dependencies made available. The key covers the dependencies' paths and
    /// digests, so a record whose key matches is rebuilt from the same base,
    /// and the record grows with the target's own outputs rather than with
    /// everything beneath it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    added_inputs: BTreeMap<String, AvailableInput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    removed_inputs: Vec<String>,
    cache_state: EvidenceCacheState,
    result: ActionResult,
    cached_results: Vec<ActionResult>,
    #[serde(default)]
    intermediate_outputs: BTreeSet<String>,
    per_action_outcomes: Vec<super::PerActionOutcome>,
}

/// Which record each target's outcome is stored under, as of one watcher
/// position. Records live in their own files, named by their contents, so
/// reading the index costs one small file however large the graph's records
/// grow, and a build reads only the records of the targets it visits.
#[derive(Deserialize)]
struct Index {
    schema: String,
    position: ChangePosition,
    targets: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct IndexRef<'a> {
    schema: &'a str,
    position: &'a ChangePosition,
    targets: &'a BTreeMap<String, String>,
}

/// Outcomes remembered for one capability of one workspace.
pub(super) struct TargetOutcomes {
    workspace: PathBuf,
    fingerprints: FingerprintStore,
    sets: TransitiveSets,
    directory: PathBuf,
    /// The record name of each target as of when this invocation started, or
    /// empty when there was nothing usable to read.
    stored: BTreeMap<String, String>,
    /// The record names this invocation reused or wrote, written back as the
    /// index on the way out.
    learned: std::sync::Mutex<BTreeMap<String, String>>,
    /// Records nobody indexes and older than this are left over from earlier
    /// invocations; newer ones may belong to a build still running.
    opened_at: SystemTime,
    enabled: bool,
    /// Whether a watcher can supply the position a save needs. Without one,
    /// records written during the build would never be indexed or removed.
    persisted: bool,
}

impl TargetOutcomes {
    /// Open the outcomes for one capability, keeping the records only when they
    /// describe the same moment the caller's account of the window does.
    ///
    /// Both are written at the end of the same build, so they normally agree.
    /// When they do not, the window says nothing about the age of these records
    /// and there is no way to tell what has moved since, so they are dropped
    /// rather than trusted.
    pub(super) fn open(
        workspace: &Path,
        capability: &str,
        sandbox: SandboxMode,
        configuration_suffix: &str,
        window_from: Option<&ChangePosition>,
        sets: TransitiveSets,
    ) -> Self {
        let name = Digest::of_bytes(
            format!("{capability}\u{0}{sandbox:?}\u{0}{configuration_suffix}").as_bytes(),
        );
        let directory = workspace
            .join(".struo")
            .join("target-outcomes")
            .join(name.to_hex());
        let stored = std::fs::read(directory.join(INDEX))
            .ok()
            .and_then(|raw| serde_json::from_slice::<Index>(&raw).ok())
            .filter(|index| index.schema == SCHEMA)
            .filter(|index| window_from == Some(&index.position))
            .map(|index| index.targets)
            .unwrap_or_default();
        Self {
            workspace: workspace.to_path_buf(),
            fingerprints: FingerprintStore::new(workspace),
            sets,
            directory,
            stored,
            learned: std::sync::Mutex::new(BTreeMap::new()),
            opened_at: SystemTime::now(),
            enabled: enabled(),
            persisted: cfg!(unix) && crate::commands::change_tracker::tracker_enabled(),
        }
    }

    fn record_path(&self, name: &str) -> PathBuf {
        self.directory.join(RECORDS).join(format!("{name}.json"))
    }

    /// The record stored for `target_id`. A missing or unreadable record is a
    /// target to visit again, never a reason to distrust the others.
    fn load(&self, target_id: &str) -> Option<Record> {
        let name = self.stored.get(target_id)?;
        let raw = std::fs::read(self.record_path(name)).ok()?;
        let mut record: Record = serde_json::from_slice(&raw).ok()?;
        self.sets
            .resolve_inputs(&self.workspace, &mut record.sources)
            .ok()?;
        Some(record)
    }

    /// The outcome recorded for `target`, when the build it describes would
    /// arrive at the same place.
    pub(super) fn reuse_with_inputs(
        &self,
        target: &GraphTarget,
        key: &str,
        changes: &KnownChanges,
        dependency_inputs: &BTreeMap<String, AvailableInput>,
        producers: &super::actions::Producers,
        observations_hold: impl FnOnce(&AnalysisObservations) -> bool,
    ) -> Option<BuildOutcome> {
        if dependency_inputs
            .iter()
            .any(|(path, input)| producers.has_fingerprint(path, input.blob_digest))
        {
            tracing::debug!(target = %target.label.id, "fingerprint-only dependency prevents target-outcome reuse");
            return None;
        }
        let record = self.reusable(target, key, changes)?;
        if !observations_hold(&record.observations) {
            tracing::debug!(target = %target.label.id, "host observations prevent target-outcome reuse");
            return None;
        }
        record.into_outcome(&self.fingerprints, dependency_inputs)
    }

    #[cfg(test)]
    fn reuse(
        &self,
        target: &GraphTarget,
        key: &str,
        changes: &KnownChanges,
    ) -> Option<BuildOutcome> {
        self.reusable(target, key, changes)
            .and_then(|record| record.into_outcome(&self.fingerprints, &BTreeMap::new()))
    }

    fn reusable(&self, target: &GraphTarget, key: &str, changes: &KnownChanges) -> Option<Record> {
        if !self.enabled {
            return None;
        }
        let KnownChanges::Since { sources, outputs } = changes else {
            return None;
        };
        let Some(record) = self.load(&target.label.id) else {
            tracing::trace!(target = %target.label.id, "no recorded outcome for this target");
            return None;
        };
        if super::has_uncached_actions(record.cache_state, &record.per_action_outcomes) {
            tracing::trace!(target = %target.label.id, "recorded outcome contains mandatory uncached work");
            return None;
        }
        if record.key != key {
            tracing::trace!(target = %target.label.id, "target or a dependency of it moved");
            return None;
        }
        if !self
            .sets
            .provider_available(&self.workspace, &record.provider)
        {
            tracing::trace!(target = %target.label.id, "a transitive set the provider names is gone");
            return None;
        }
        if record.reads_any_of(sources) {
            tracing::trace!(target = %target.label.id, "a file this target reads moved");
            return None;
        }
        if touches_any_of(outputs, &record.outputs) {
            tracing::trace!(target = %target.label.id, "an output of this target moved");
            return None;
        }
        if record
            .result
            .outputs
            .keys()
            .filter(|output| !record.intermediate_outputs.contains(*output))
            .any(|output| self.workspace.join(output).symlink_metadata().is_err())
        {
            tracing::trace!(target = %target.label.id, "a recorded action output is missing");
            return None;
        }
        tracing::trace!(target = %target.label.id, "reused a recorded target outcome");
        Some(record)
    }

    /// Remember what building `target` produced.
    ///
    /// An outcome that has to run again on every invocation is not remembered:
    /// the point of a record is to skip work, and skipping that work is exactly
    /// what its target asked not to happen.
    #[cfg(test)]
    fn record(
        &self,
        target: &GraphTarget,
        key: String,
        observations: &AnalysisObservations,
        declared_inputs: &DeclaredInputs,
        outcome: &BuildOutcome,
    ) {
        self.record_with_inputs(
            target,
            key,
            observations,
            declared_inputs,
            &BTreeMap::new(),
            outcome,
        );
    }

    /// Remember what building `target` on top of `dependency_inputs` produced.
    pub(super) fn record_with_inputs(
        &self,
        target: &GraphTarget,
        key: String,
        observations: &AnalysisObservations,
        declared_inputs: &DeclaredInputs,
        dependency_inputs: &BTreeMap<String, AvailableInput>,
        outcome: &BuildOutcome,
    ) {
        if !self.enabled
            || !self.persisted
            || !observations.is_complete()
            || super::has_uncached_actions(outcome.cache_state, &outcome.per_action_outcomes)
        {
            return;
        }
        let same = |left: &AvailableInput, right: &AvailableInput| {
            left.blob_digest == right.blob_digest && left.same_target == right.same_target
        };
        let input_fingerprint = match outcome
            .input_fingerprint
            .as_ref()
            .map(|fingerprint| self.fingerprints.encode(fingerprint))
            .transpose()
        {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                tracing::debug!(error = %error, "could not store target fingerprint");
                return;
            }
        };
        let record = Record {
            key,
            observations: observations.clone(),
            sources: declared_inputs.clone(),
            expansions: observations
                .entries()
                .iter()
                .filter_map(|observation| match observation {
                    Observation::Paths {
                        expansion,
                        package,
                        patterns,
                        ..
                    } => Some(Expansion {
                        kind: expansion.clone(),
                        package: package.clone(),
                        patterns: patterns.clone(),
                    }),
                    _ => None,
                })
                .collect(),
            outputs: outcome.outputs.clone(),
            provider: outcome.provider.as_ref().clone(),
            action_digest: outcome.action_digest,
            input_digest: outcome.input_digest,
            input_fingerprint,
            added_inputs: outcome
                .available_inputs
                .iter()
                .filter(|(path, input)| {
                    dependency_inputs
                        .get(*path)
                        .is_none_or(|base| !same(base, input))
                })
                .map(|(path, input)| (path.clone(), *input))
                .collect(),
            removed_inputs: dependency_inputs
                .keys()
                .filter(|path| !outcome.available_inputs.contains_key(*path))
                .cloned()
                .collect(),
            cache_state: outcome.cache_state,
            result: outcome.result.clone(),
            cached_results: outcome.cached_results.clone(),
            intermediate_outputs: outcome.intermediate_outputs.clone(),
            per_action_outcomes: outcome.per_action_outcomes.clone(),
        };
        let Some(name) = self.write_record(&record) else {
            return;
        };
        self.learned
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(target.label.id.clone(), name);
    }

    /// Write `record` under the name of its contents and return that name, so
    /// the build holds a name rather than the record until it saves. An
    /// unchanged record is already on disk and is not written again.
    fn write_record(&self, record: &Record) -> Option<String> {
        let raw = serde_json::to_vec(record).ok()?;
        let name = Digest::of_bytes(&raw).to_hex();
        let path = self.record_path(&name);
        if path.is_file() {
            return Some(name);
        }
        std::fs::create_dir_all(path.parent()?).ok()?;
        // Named by process so two builds writing the same record concurrently
        // cannot read each other's half-written file.
        let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
        if std::fs::write(&temporary, raw).is_ok() && std::fs::rename(&temporary, &path).is_ok() {
            return Some(name);
        }
        let _ = std::fs::remove_file(&temporary);
        None
    }

    /// Carry a record this invocation reused into what gets written back, so a
    /// target skipped today is still skippable tomorrow.
    pub(super) fn carry_forward(&self, target_id: &str) {
        let Some(name) = self.stored.get(target_id) else {
            return;
        };
        self.learned
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(target_id.to_string(), name.clone());
    }

    /// Write back what this invocation knows, against the position it describes.
    ///
    /// Only what this invocation actually visited or reused is kept. A target
    /// that fell out of the graph leaves with it, and so does its record file,
    /// so the directory tracks the graph rather than accumulating every target
    /// the workspace ever had.
    pub(super) fn save(&self, position: Option<&ChangePosition>) {
        let (true, Some(position)) = (self.enabled, position) else {
            return;
        };
        let targets = self
            .learned
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if targets.is_empty() {
            return;
        }
        let index = IndexRef {
            schema: SCHEMA,
            position,
            targets: &targets,
        };
        let Ok(raw) = serde_json::to_vec(&index) else {
            return;
        };
        let path = self.directory.join(INDEX);
        let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
        if !std::fs::write(&temporary, raw)
            .is_ok_and(|()| std::fs::rename(&temporary, &path).is_ok())
        {
            let _ = std::fs::remove_file(&temporary);
            return;
        }
        let indexed = targets
            .values()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        self.remove_unindexed_records(&indexed);
        // Earlier versions kept every record in one file beside this directory.
        let _ = std::fs::remove_file(self.directory.with_extension("json"));
    }

    fn remove_unindexed_records(&self, indexed: &BTreeSet<&str>) {
        let Ok(entries) = std::fs::read_dir(self.directory.join(RECORDS)) else {
            return;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();
            if file_name
                .strip_suffix(".json")
                .is_some_and(|name| indexed.contains(name))
            {
                continue;
            }
            let stale = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .is_ok_and(|modified| modified < self.opened_at);
            if stale {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

impl Record {
    fn into_outcome(
        self,
        fingerprints: &FingerprintStore,
        dependency_inputs: &BTreeMap<String, AvailableInput>,
    ) -> Option<BuildOutcome> {
        let input_fingerprint = self
            .input_fingerprint
            .map(|stored| fingerprints.decode(stored))
            .transpose()
            .ok()?;
        let mut available_inputs = dependency_inputs.clone();
        for path in &self.removed_inputs {
            available_inputs.remove(path);
        }
        available_inputs.extend(self.added_inputs);
        // Whether an output is on disk described the disk of the build that
        // wrote the record. A reader restores what it needs instead, which
        // costs nothing for an output that is still in place.
        for input in available_inputs.values_mut() {
            input.materialized = false;
        }
        Some(BuildOutcome {
            provider: std::sync::Arc::new(self.provider),
            action_digest: self.action_digest,
            input_digest: self.input_digest,
            input_fingerprint,
            available_inputs,
            outputs: self.outputs,
            // A reused record means nothing ran, which is a hit however the
            // build that produced it went. Replaying the producing run's state
            // would report a miss for work that did not happen, and tell the
            // evidence the same untruth.
            cache_tag: EvidenceCacheState::Hit.as_str(),
            cache_state: EvidenceCacheState::Hit,
            result: self.result,
            cached_results: self.cached_results,
            intermediate_outputs: self.intermediate_outputs,
            per_action_outcomes: self
                .per_action_outcomes
                .into_iter()
                .map(|mut action| {
                    action.cache_state = EvidenceCacheState::Hit;
                    action.duration_ms = 0;
                    action
                })
                .collect(),
        })
    }

    /// Whether any changed path is one this target read, or one an expansion it
    /// ran would have selected.
    fn reads_any_of(&self, changed: &BTreeSet<String>) -> bool {
        if changed.is_empty() {
            return false;
        }
        if self
            .sources
            .iter()
            .filter(|path| !path.starts_with(".struo/out/"))
            .any(|known| {
                changed.contains(known)
                    || changed
                        .range(format!("{known}/")..)
                        .next()
                        .is_some_and(|path| path.starts_with(&format!("{known}/")))
            })
        {
            return true;
        }
        // The same test the analysis replay uses, so the two cannot disagree
        // about whether a change reaches an expansion.
        self.expansions.iter().any(|expansion| {
            struo_frontend::analysis::expansion_could_differ(
                changed,
                &expansion.kind,
                &expansion.package,
                &expansion.patterns,
            )
        })
    }
}

/// Whether any changed path is one of `paths` or sits inside one of them.
fn touches_any_of(changed: &BTreeSet<String>, paths: &[String]) -> bool {
    changed.iter().any(|path| {
        paths
            .iter()
            .any(|known| path == known || path.starts_with(&format!("{known}/")))
    })
}

// A producer's action can stay unchanged while its output contents change.
pub(super) fn key(
    analysis_key: &str,
    dependency_action_digests: &[(String, Digest)],
    dependency_inputs: &BTreeMap<String, AvailableInput>,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"struo.target.outcomes.key.v2\0");
    hash_field(&mut hasher, analysis_key.as_bytes());
    let mut ordered = dependency_action_digests.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.as_bytes().cmp(right.1.as_bytes()))
    });
    hasher.update(&(ordered.len() as u64).to_le_bytes());
    for (label, digest) in ordered {
        hash_field(&mut hasher, label.as_bytes());
        hasher.update(digest.as_bytes());
    }
    hasher.update(&(dependency_inputs.len() as u64).to_le_bytes());
    for (path, input) in dependency_inputs {
        hash_field(&mut hasher, path.as_bytes());
        hasher.update(input.blob_digest.as_bytes());
    }
    Digest::from_bytes(*hasher.finalize().as_bytes()).to_hex()
}

fn hash_field(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// Turn off reusing recorded outcomes, to confirm a build visited its targets
/// rather than reading what they produced last time.
fn enabled() -> bool {
    !matches!(
        std::env::var("STRUO_TARGET_OUTCOMES").ok().as_deref(),
        Some("0" | "false")
    )
}

#[cfg(test)]
mod tests;
