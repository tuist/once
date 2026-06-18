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
    pub exit_code: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<Digest>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, Digest>,
}

#[derive(Debug, Clone, Copy)]
pub struct Stats {
    pub blob_count: u64,
    pub blob_bytes: u64,
    pub action_count: u64,
    pub action_bytes: u64,
}
