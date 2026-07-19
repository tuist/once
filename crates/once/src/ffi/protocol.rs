use std::path::PathBuf;

use once_cas::ActionResult;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize)]
pub(super) struct CacheSelection {
    pub(super) local_cache_root: Option<PathBuf>,
    pub(super) workspace_root: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CacheRequest {
    #[serde(flatten)]
    pub(super) cache: CacheSelection,
}

#[derive(Debug, Deserialize)]
pub(super) struct BlobPutRequest {
    #[serde(flatten)]
    pub(super) cache: CacheSelection,
    pub(super) bytes: Vec<u8>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DigestRequest {
    #[serde(flatten)]
    pub(super) cache: CacheSelection,
    pub(super) digest: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct FilePutRequest {
    #[serde(flatten)]
    pub(super) cache: CacheSelection,
    pub(super) path: PathBuf,
}

#[derive(Debug, Deserialize)]
pub(super) struct BlobFileRequest {
    #[serde(flatten)]
    pub(super) cache: CacheSelection,
    pub(super) digest: String,
    pub(super) path: PathBuf,
}

#[derive(Debug, Deserialize)]
pub(super) struct ActionResultPutRequest {
    #[serde(flatten)]
    pub(super) cache: CacheSelection,
    pub(super) action_digest: String,
    pub(super) result: ActionResult,
}

#[derive(Debug, Deserialize)]
pub(super) struct ActionDigestRequest {
    #[serde(flatten)]
    pub(super) cache: CacheSelection,
    pub(super) action_digest: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct ActionKeyRequest {
    pub(super) namespace: String,
    pub(super) inputs: Vec<ActionKeyInputRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum ActionKeyInputRequest {
    Bytes { label: String, bytes: Vec<u8> },
    Digest { label: String, digest: String },
}

#[derive(Debug, Serialize)]
pub(super) struct BlobResponse {
    pub(super) bytes: Vec<u8>,
}

#[derive(Debug, Serialize)]
pub(super) struct StatsResponse {
    pub(super) blob_count: u64,
    pub(super) blob_bytes: u64,
    pub(super) action_count: u64,
    pub(super) action_bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(super) enum FfiResponse<T> {
    Ok { value: T },
    Error { message: String },
}
