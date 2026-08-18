//! Remember the graph derived from a workspace's own package manifests.
//!
//! For a project that declares no Once targets at all, deriving the graph means
//! running the package manager's resolver as a subprocess, decoding its answer,
//! and turning it into one target per compilation unit. On a workspace with a
//! few hundred locked packages that is the largest single cost of an invocation
//! that has nothing to build, and it is paid again on every invocation that
//! touches a source file, because a changed source defeats the build receipt
//! and sends the whole pipeline through derivation again.
//!
//! Reuse is decided by asking the filesystem watcher, not by re-deriving.
//! Derivation reads two things: files in the workspace, and state on the host.
//! The record below names the files, and the watcher reports every workspace
//! path that changed since the record was written, so "no recorded input
//! changed, and no path that appeared matches a pattern that would have
//! selected one" settles the first. The observation ledger settles the second,
//! by asking each host question again and comparing.
//!
//! Without a watcher there is nothing to ask, so the graph is derived. That is
//! deliberate: the alternative would be looking at every manifest in the
//! workspace, which is most of what derivation costs anyway.

use std::path::{Path, PathBuf};

use once_cas::Digest;
use once_frontend::analysis::{AnalysisEngine, CommandPolicy};
use once_frontend::{GraphTarget, ResolutionRecord};
use serde::{Deserialize, Serialize};

use super::source_digest_cache::KnownChanges;
use crate::commands::change_tracker::ChangePosition;

const SCHEMA: &str = "once.resolution.v1";

#[derive(Deserialize, Serialize)]
struct Stored {
    schema: String,
    /// Names the derivation: same name, same question.
    key: String,
    /// Watcher position the recorded inputs describe.
    position: ChangePosition,
    graph: Vec<GraphTarget>,
    record: ResolutionRecord,
}

pub struct ResolutionCache {
    path: PathBuf,
    enabled: bool,
}

impl ResolutionCache {
    pub fn open(workspace: &Path) -> Self {
        Self {
            path: workspace.join(".once").join("resolution.json"),
            enabled: enabled(),
        }
    }

    /// The stored graph, when it was derived from the same question and nothing
    /// it read has moved since.
    pub fn reuse(
        &self,
        analyzer: &AnalysisEngine,
        workspace: &Path,
        key: &str,
        changes: &KnownChanges,
    ) -> Option<Vec<GraphTarget>> {
        if !self.enabled {
            return None;
        }
        let KnownChanges::Since { sources, .. } = changes else {
            // No watcher, or one that cannot account for the whole window.
            return None;
        };
        let stored = serde_json::from_slice::<Stored>(&std::fs::read(&self.path).ok()?).ok()?;
        if stored.schema != SCHEMA || stored.key != key {
            return None;
        }
        if stored.record.touched_by(sources) {
            tracing::debug!("workspace manifests moved; deriving the graph again");
            return None;
        }
        // Re-running the resolver's own commands is the entire cost this
        // avoids, so they are settled by the program rather than by the answer.
        if !analyzer.observations_hold(
            workspace,
            &stored.record.observations,
            CommandPolicy::TrustDeclaredInputs,
            &once_frontend::analysis::UnchangedWorkspace::Unknown,
        ) {
            tracing::debug!(
                stale = ?analyzer.first_stale_observation(
                    workspace,
                    &stored.record.observations,
                    CommandPolicy::TrustDeclaredInputs,
                    &once_frontend::analysis::UnchangedWorkspace::Unknown,
                ),
                "graph derivation no longer describes the host"
            );
            return None;
        }
        tracing::debug!(targets = stored.graph.len(), "reused the derived graph");
        Some(stored.graph)
    }

    /// Remember a derivation against the watcher position it describes.
    ///
    /// Without a position there is no window for a later invocation to ask
    /// about, so nothing is written: a record no one can check is dead weight.
    pub fn store(
        &self,
        key: &str,
        position: Option<&ChangePosition>,
        graph: &[GraphTarget],
        record: &ResolutionRecord,
    ) {
        let (true, Some(position)) = (self.enabled, position) else {
            return;
        };
        if !record.observations.is_complete() {
            return;
        }
        let stored = Stored {
            schema: SCHEMA.to_string(),
            key: key.to_string(),
            position: position.clone(),
            graph: graph.to_vec(),
            record: record.clone(),
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

/// Name one derivation.
///
/// The prelude decides what a manifest means, the configuration selects which
/// branches it takes, and the executable implements the globals the resolvers
/// call, so all three are part of the question. So is the set of targets the
/// workspace declared before resolution, which is what the resolvers expand.
pub fn key(
    module_source: &str,
    workspace: &Path,
    configuration: Digest,
    executable_identity: &str,
    seeds: &[once_frontend::Target],
) -> Option<String> {
    let workspace = std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let mut bytes = Vec::new();
    let mut part = |value: &[u8]| {
        bytes.extend_from_slice(&(value.len() as u64).to_le_bytes());
        bytes.extend_from_slice(value);
    };
    part(SCHEMA.as_bytes());
    part(executable_identity.as_bytes());
    part(module_source.as_bytes());
    part(workspace.to_string_lossy().as_bytes());
    part(configuration.to_hex().as_bytes());
    part(&serde_json::to_vec(seeds).ok()?);
    Some(Digest::of_bytes(&bytes).to_hex())
}

/// Turn off reusing derived graphs, to confirm a graph came from the workspace
/// rather than from a record of it.
fn enabled() -> bool {
    !matches!(
        std::env::var("ONCE_RESOLUTION_CACHE").ok().as_deref(),
        Some("0" | "false")
    )
}

/// Whether any changed path is one the derivation would have read.
///
/// Exposed for tests over [`ResolutionRecord`], which owns the matching rules.
#[cfg(test)]
pub(super) fn touched(
    record: &ResolutionRecord,
    changed: &std::collections::BTreeSet<String>,
) -> bool {
    record.touched_by(changed)
}

#[cfg(test)]
mod tests;
