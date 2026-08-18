//! Remember what analysing a target produced, so an unchanged target is not
//! analysed again.
//!
//! A target kind's implementation is Starlark, and running it is the largest
//! single cost of an invocation with nothing to build: a graph derived from a
//! package manifest can hold thousands of targets, each one assembling command
//! lines, walking its dependencies' providers, and declaring its outputs, all
//! to arrive at exactly what it arrived at last time.
//!
//! What makes skipping it sound is that the implementation is a function. Its
//! arguments are the target definition and its dependencies' providers; its
//! other inputs are the answers it got from the host, and those are recorded as
//! it runs. A record is reused only when the arguments hash to the same name
//! and every recorded answer is still the answer the host gives. This is the
//! verifying-trace rebuilder from Mokhov, Mitchell and Peyton Jones, "Build
//! Systems à la Carte", applied one level up from actions: the trace is over
//! analysis rather than over execution.
//!
//! Records are one file per name so a build reads only the targets it needs,
//! rather than parsing every target's record to use one.

use std::path::{Path, PathBuf};

use once_frontend::analysis::{AnalysisObservations, AnalysisResult, DeclaredAction};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

const SCHEMA: &str = "once.analysis-memo.v1";

/// Analysis of one target, held against the name of the call that produced it.
#[derive(Deserialize, Serialize)]
struct Record {
    schema: String,
    actions: Vec<DeclaredAction>,
    provider: JsonValue,
    declared_outputs: Vec<String>,
    observations: AnalysisObservations,
}

#[derive(Clone)]
pub struct AnalysisMemo {
    root: PathBuf,
    /// Identity of the running executable, part of every name because the
    /// globals an implementation calls are compiled into it.
    executable_identity: String,
    enabled: bool,
}

impl AnalysisMemo {
    pub fn open(workspace: &Path) -> Self {
        let executable_identity = executable_identity();
        Self {
            root: workspace.join(".once").join("analysis"),
            // With no identity for the executable there is no way to notice
            // that the code behind the globals changed, so remember nothing.
            enabled: enabled() && executable_identity.is_some(),
            executable_identity: executable_identity.unwrap_or_default(),
        }
    }

    #[must_use]
    pub fn executable_identity(&self) -> &str {
        &self.executable_identity
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// The stored analysis under `key`, if there is one and it parses.
    pub fn read(&self, key: &str) -> Option<(AnalysisResult, AnalysisObservations)> {
        if !self.enabled {
            return None;
        }
        let raw = std::fs::read(self.path(key)).ok()?;
        let record = serde_json::from_slice::<Record>(&raw).ok()?;
        if record.schema != SCHEMA {
            return None;
        }
        Some((
            AnalysisResult {
                actions: record.actions,
                provider: record.provider,
                declared_outputs: record.declared_outputs,
                // A replayed analysis observed nothing: its answers were
                // checked before it was handed back, and re-recording them
                // would claim this run asked the questions.
                observations: AnalysisObservations::default(),
            },
            record.observations,
        ))
    }

    /// Remember `analysis` under `key`.
    ///
    /// An analysis that read something the ledger cannot describe is not
    /// stored: there would be no way to find out later whether it still holds,
    /// and a record that cannot be checked is worse than no record.
    pub fn write(&self, key: &str, analysis: &AnalysisResult) {
        if !self.enabled || !analysis.observations.is_complete() {
            return;
        }
        let record = Record {
            schema: SCHEMA.to_string(),
            actions: analysis.actions.clone(),
            provider: analysis.provider.clone(),
            declared_outputs: analysis.declared_outputs.clone(),
            observations: analysis.observations.clone(),
        };
        let Ok(raw) = serde_json::to_vec(&record) else {
            return;
        };
        let path = self.path(key);
        let Some(parent) = path.parent() else {
            return;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        // Named by process so two builds writing the same record concurrently
        // cannot read each other's half-written file. Either rename wins and
        // both hold the same bytes.
        let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
        if std::fs::write(&temporary, raw).is_ok() && std::fs::rename(&temporary, &path).is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
    }

    fn path(&self, key: &str) -> PathBuf {
        // One directory per leading byte of the name: a workspace with
        // thousands of targets would otherwise put every record in one
        // directory, which some filesystems handle poorly.
        let (shard, rest) = key.split_at(2.min(key.len()));
        self.root.join(shard).join(format!("{rest}.json"))
    }
}

/// Turn off remembering analyses. Useful to confirm that a result came from
/// running the implementation rather than from a record of it.
fn enabled() -> bool {
    !matches!(
        std::env::var("ONCE_ANALYSIS_MEMO").ok().as_deref(),
        Some("0" | "false")
    )
}

/// Identity of the running binary.
///
/// Content would be the honest answer, but hashing a large executable on every
/// invocation costs more than the records save. Size with the inode change time
/// distinguishes any rebuild or reinstall: both are writes, and the kernel
/// stamps ctime on writes whatever a build system claims about mtime.
fn executable_identity() -> Option<String> {
    let executable = std::env::current_exe().ok()?;
    let metadata = std::fs::metadata(&executable).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        Some(format!(
            "{}:{}:{}.{}:{}",
            executable.display(),
            metadata.len(),
            metadata.ctime(),
            metadata.ctime_nsec(),
            metadata.ino()
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
            "{}:{}:{}.{}",
            executable.display(),
            metadata.len(),
            modified.as_secs(),
            modified.subsec_nanos()
        ))
    }
}

#[cfg(test)]
mod tests;
