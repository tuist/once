use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Digest;

/// Cached result of a single action execution.
///
/// `outputs` records each declared output file the action produced
/// (workspace-relative path -> blob digest). On a cache hit, the runner
/// restores these blobs from the CAS to their declared paths so a
/// dependent action sees the file it expected, even if the producing
/// action did not actually run on this machine.
///
/// `stdout` and `stderr` are optional: a caller that did not capture
/// output (or had nothing worth recording) simply leaves them unset
/// rather than materialising an empty blob.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionResult {
    /// Process exit code, or `-1` when the process was signalled.
    pub exit_code: i32,
    /// Blob holding captured stdout, absent when it was not captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<Digest>,
    /// Blob holding captured stderr, absent when it was not captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<Digest>,
    /// Declared outputs the action produced, keyed by workspace-relative
    /// path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, Digest>,
}

/// Size of a local store, as counted by a full walk.
#[derive(Debug, Clone, Copy)]
pub struct Stats {
    /// Number of content blobs held.
    pub blob_count: u64,
    /// Bytes those blobs occupy on disk, after compression.
    pub blob_bytes: u64,
    /// Number of cached action results held.
    pub action_count: u64,
    /// Bytes those action records occupy on disk.
    pub action_bytes: u64,
}
