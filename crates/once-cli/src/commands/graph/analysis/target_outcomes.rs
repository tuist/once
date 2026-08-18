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

use once_cas::{ActionResult, Digest};
use once_core::{EvidenceCacheState, InputFingerprintManifest, SandboxMode};
use once_frontend::analysis::{AnalysisObservations, Observation};
use once_frontend::GraphTarget;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use super::source_digest_cache::KnownChanges;
use super::{AvailableInput, BuildOutcome};
use crate::commands::change_tracker::ChangePosition;

const SCHEMA: &str = "once.target-outcomes.v2";

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
    sources: Vec<String>,
    /// What its analysis expanded, so a file that appears where one of these
    /// patterns would have selected it counts as a change.
    expansions: Vec<Expansion>,
    outputs: Vec<String>,
    provider: JsonValue,
    action_digest: Digest,
    input_digest: Option<Digest>,
    input_fingerprint: Option<InputFingerprintManifest>,
    available_inputs: BTreeMap<String, AvailableInput>,
    cache_state: EvidenceCacheState,
    result: ActionResult,
    cached_results: Vec<ActionResult>,
}

#[derive(Deserialize, Serialize)]
struct Stored {
    schema: String,
    /// Watcher position the records describe.
    position: ChangePosition,
    targets: BTreeMap<String, Record>,
}

/// Outcomes remembered for one capability of one workspace.
pub(super) struct TargetOutcomes {
    path: PathBuf,
    /// What the file held when this invocation started, or empty when there was
    /// nothing usable to read.
    stored: BTreeMap<String, Record>,
    /// What this invocation learned, written back on the way out.
    learned: std::sync::Mutex<BTreeMap<String, Record>>,
    enabled: bool,
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
    ) -> Self {
        let name = Digest::of_bytes(
            format!("{capability}\u{0}{sandbox:?}\u{0}{configuration_suffix}").as_bytes(),
        );
        let path = workspace
            .join(".once")
            .join("target-outcomes")
            .join(format!("{name}.json"));
        let stored = std::fs::read(&path)
            .ok()
            .and_then(|raw| serde_json::from_slice::<Stored>(&raw).ok())
            .filter(|stored| stored.schema == SCHEMA)
            .filter(|stored| window_from == Some(&stored.position))
            .map(|stored| stored.targets)
            .unwrap_or_default();
        Self {
            path,
            stored,
            learned: std::sync::Mutex::new(BTreeMap::new()),
            enabled: enabled(),
        }
    }

    /// The outcome recorded for `target`, when the build it describes would
    /// arrive at the same place.
    pub(super) fn reuse(
        &self,
        target: &GraphTarget,
        key: &str,
        changes: &KnownChanges,
    ) -> Option<BuildOutcome> {
        if !self.enabled {
            return None;
        }
        let KnownChanges::Since { sources, outputs } = changes else {
            return None;
        };
        let Some(record) = self.stored.get(&target.label.id) else {
            tracing::trace!(target = %target.label.id, "no recorded outcome for this target");
            return None;
        };
        if record.key != key {
            tracing::trace!(target = %target.label.id, "target or a dependency of it moved");
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
        tracing::trace!(target = %target.label.id, "reused a recorded target outcome");
        Some(BuildOutcome {
            provider: std::sync::Arc::new(record.provider.clone()),
            action_digest: record.action_digest,
            input_digest: record.input_digest,
            input_fingerprint: record.input_fingerprint.clone(),
            available_inputs: record.available_inputs.clone(),
            outputs: record.outputs.clone(),
            // A reused record means nothing ran, which is a hit however the
            // build that produced it went. Replaying the producing run's state
            // would report a miss for work that did not happen, and tell the
            // evidence the same untruth.
            cache_tag: EvidenceCacheState::Hit.as_str(),
            cache_state: EvidenceCacheState::Hit,
            result: record.result.clone(),
            cached_results: record.cached_results.clone(),
        })
    }

    /// Remember what building `target` produced.
    ///
    /// An outcome that has to run again on every invocation is not remembered:
    /// the point of a record is to skip work, and skipping that work is exactly
    /// what its target asked not to happen.
    pub(super) fn record(
        &self,
        target: &GraphTarget,
        key: String,
        observations: &AnalysisObservations,
        declared_inputs: &BTreeSet<String>,
        outcome: &BuildOutcome,
    ) {
        if !self.enabled || outcome.cache_state == EvidenceCacheState::Bypass {
            return;
        }
        let out_prefix = ".once/out/";
        let record = Record {
            key,
            sources: declared_inputs
                .iter()
                .filter(|input| !input.starts_with(out_prefix))
                .cloned()
                .collect(),
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
            input_fingerprint: outcome.input_fingerprint.clone(),
            available_inputs: outcome.available_inputs.clone(),
            cache_state: outcome.cache_state,
            result: outcome.result.clone(),
            cached_results: outcome.cached_results.clone(),
        };
        self.learned
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(target.label.id.clone(), record);
    }

    /// Carry a record this invocation reused into what gets written back, so a
    /// target skipped today is still skippable tomorrow.
    pub(super) fn carry_forward(&self, target_id: &str) {
        let Some(record) = self.stored.get(target_id) else {
            return;
        };
        self.learned
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(target_id.to_string(), record.clone());
    }

    /// Write back what this invocation knows, against the position it describes.
    ///
    /// Only what this invocation actually visited or reused is kept. A target
    /// that fell out of the graph leaves with it, so the file tracks the graph
    /// rather than accumulating every target the workspace ever had.
    pub(super) fn save(&self, position: Option<&ChangePosition>) {
        let (true, Some(position)) = (self.enabled, position) else {
            return;
        };
        let targets = self
            .learned
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if targets.is_empty() {
            return;
        }
        let stored = Stored {
            schema: SCHEMA.to_string(),
            position: position.clone(),
            targets,
        };
        let Ok(raw) = serde_json::to_vec(&stored) else {
            return;
        };
        if let Some(parent) = self.path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        let temporary = self
            .path
            .with_extension(format!("json.tmp-{}", std::process::id()));
        if std::fs::write(&temporary, raw).is_ok()
            && std::fs::rename(&temporary, &self.path).is_err()
        {
            let _ = std::fs::remove_file(&temporary);
        }
    }
}

impl Record {
    /// Whether any changed path is one this target read, or one an expansion it
    /// ran would have selected.
    fn reads_any_of(&self, changed: &BTreeSet<String>) -> bool {
        if changed.is_empty() {
            return false;
        }
        if touches_any_of(changed, &self.sources) {
            return true;
        }
        // The same test the analysis replay uses, so the two cannot disagree
        // about whether a change reaches an expansion.
        self.expansions.iter().any(|expansion| {
            once_frontend::analysis::expansion_could_differ(
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

/// Name one target's build.
///
/// The analysis name already covers the target definition, the code that
/// analyses it, the configuration, and the dependencies' provider records.
/// What it does not cover is which build those providers came from, so the
/// dependencies' action digests go in too: a dependency that rebuilt gives
/// everything above it a different name even when its provider is unchanged.
pub(super) fn key(analysis_key: &str, dependency_action_digests: &[(String, Digest)]) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(SCHEMA.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(analysis_key.as_bytes());
    let mut ordered = dependency_action_digests.to_vec();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    for (label, digest) in ordered {
        bytes.push(0);
        bytes.extend_from_slice(label.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(digest.to_hex().as_bytes());
    }
    Digest::of_bytes(&bytes).to_hex()
}

/// Turn off reusing recorded outcomes, to confirm a build visited its targets
/// rather than reading what they produced last time.
fn enabled() -> bool {
    !matches!(
        std::env::var("ONCE_TARGET_OUTCOMES").ok().as_deref(),
        Some("0" | "false")
    )
}

#[cfg(test)]
mod tests;
