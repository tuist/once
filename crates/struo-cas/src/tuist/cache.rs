use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bazel_remote_apis::{
    build::bazel::remote::execution::v2::{
        self as reapi, action_cache_client::ActionCacheClient,
        capabilities_client::CapabilitiesClient,
        content_addressable_storage_client::ContentAddressableStorageClient,
    },
    google::bytestream::{self, byte_stream_client::ByteStreamClient},
};
use futures::{future::try_join_all, stream};
use reqwest::{Method, Url};
use sha2::{Digest as _, Sha256};
use tokio::sync::{Mutex, Semaphore};
use tokio::time::Instant;
use tonic::metadata::MetadataValue;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
#[cfg(test)]
use tonic::Status;
use tonic::{Code, Request};
use uuid::Uuid;

use super::recovery::Recovering;
use super::status;
use super::{
    env_token, file_body, flights::Flights, join_url, redact, retry, stall, TuistAuth,
    TuistCacheConfig, ENDPOINTS_PATH, PROVIDER_NAME,
};
use crate::transfer::{self, TransferDirection};
use crate::{ActionCacheHit, ActionResult, Cas, Digest, Error, ErrorKind, Result, TransferBudget};

use endpoints::grpc_endpoint;
pub(super) use endpoints::grpc_endpoint_url;

mod batch_download;
mod batch_rpc;
mod download;
mod endpoints;
mod execution;
mod stream_limits;

#[path = "cache_download.rs"]
mod stream_download;

/// Ceiling on blobs transferred to or from the remote cache at once. The
/// number of outputs comes from a remote `ActionResult` (untrusted), and
/// each transfer buffers a whole blob, so an unbounded fan-out would let
/// a single response spike memory and open a connection per blob. This
/// caps the in-flight transfers regardless of how many the response
/// declares.
const MAX_CONCURRENT_BLOB_TRANSFERS: usize = 16;

const ENDPOINT_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// Cache endpoint that replaces server-side endpoint discovery, matching the
/// Tuist CLI and Gradle plugin. Tuist runners set it to the in-cluster Kura
/// service, which their network policy allows, while the public Kura ingress
/// that discovery returns can be unreachable from inside the runner network.
const TUIST_CACHE_ENDPOINT_ENV: &str = "TUIST_CACHE_ENDPOINT";
const GRPC_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a `ByteStream` transfer may go without moving. `GRPC_REQUEST_TIMEOUT`
/// only lasts until response headers arrive. A write's headers come after its
/// last byte, so a fixed deadline caps the blob size a link can upload; a
/// read's come before its first, so a body that stops is never bounded.
const STREAM_STALL_TIMEOUT: Duration = Duration::from_secs(30);
const BATCH_BLOB_LIMIT: usize = 2 * 1024 * 1024;
const BATCH_BLOB_LIMIT_I64: i64 = 2 * 1024 * 1024;
const BYTE_STREAM_CHUNK_SIZE: usize = 2 * 1024 * 1024;
#[allow(clippy::cast_possible_truncation)]
const BYTE_STREAM_MESSAGE_LIMIT: usize = crate::budget::MAX_STREAM_MESSAGE_BYTES as usize;
const MAX_PARALLEL_TRANSFERS: usize = 8;
/// Largest response a metadata RPC (action cache, existence checks) may
/// decode. Each such call reserves this much memory from the transfer budget,
/// which makes the reservation a bound rather than an estimate.
const METADATA_RESPONSE_LIMIT: usize = 4 * 1024 * 1024;
/// Memory a streamed upload from a file holds: the chunk being sent, the one
/// queued behind it, and the one being read or compressed. The compressor's
/// own working memory is outside the budget.
fn file_upload_memory(size_bytes: i64) -> u64 {
    let chunk = u64::try_from(size_bytes)
        .unwrap_or_default()
        .min(BYTE_STREAM_CHUNK_SIZE as u64);
    chunk.saturating_mul(3)
}
const KURA_FEATURE_FLAGS_HEADER: &str = "x-tuist-feature-flags";
const KURA_FEATURE_FLAG: &str = "kura";
const BLOB_MAPPING_OUTPUT_PATH: &str = "blob";
const BLOB_MAPPING_PREFIX: &str = "struo.blob.v1";
const ACTION_MAPPING_PREFIX: &str = "struo.action.v1";
const ACTION_RESULT_METADATA_TYPE_URL: &str = "type.googleapis.com/struo.cache.v1.ActionResult";
const REAPI_STATUS_OK: i32 = 0;
const REAPI_STATUS_NOT_FOUND: i32 = 5;
const SHA256_DIGEST_FUNCTION: i32 = reapi::digest_function::Value::Sha256 as i32;

#[derive(Debug, Clone)]
pub struct TuistCache {
    local: Cas,
    client: Arc<Recovering<reqwest::Client>>,
    config: TuistCacheConfig,
    grpc_channel_cache: Arc<Recovering<GrpcChannels>>,
    stream_stall_timeout: Duration,
    capabilities_cache: Arc<Recovering<RemoteCapabilities>>,
    known_remote_blobs: Arc<Mutex<HashMap<Digest, reapi::Digest>>>,
    publication_flights: Arc<Flights>,
    download_flights: Arc<Flights>,
    batch_downloads: Arc<batch_download::BatchDownloads>,
    transfer_limit: Arc<Semaphore>,
    budget: TransferBudget,
    auth: TuistAuth,
    auth_token_cache: Arc<Recovering<String>>,
    endpoint_override: Option<String>,
    execution_cache: Arc<Recovering<execution::TuistExecution>>,
    /// A credential resolved again after the service rejected the cached one.
    /// Shared by every clone, so one refresh serves the whole process.
    refreshed_token: Arc<std::sync::Mutex<Option<String>>>,
}

pub use execution::{Executed, TuistExecution, DEFAULT_EXECUTION_TIMEOUT};

/// Both channels reach the same endpoint. `ByteStream` writes get their own
/// because tonic's request timeout lasts until response headers arrive, which
/// for a write is after its last byte.
#[derive(Debug, Clone)]
struct GrpcChannels {
    requests: Channel,
    uploads: Channel,
}

#[derive(Debug, Clone, Copy, Default)]
struct RemoteCapabilities {
    zstd_streams: bool,
    zstd_batches: bool,
    batch_read_limit: Option<u64>,
}

impl TuistCache {
    pub fn new(local: Cas, auth_root: impl AsRef<Path>, config: TuistCacheConfig) -> Result<Self> {
        let Some(account) = config.account.as_deref() else {
            return Err(Error::InvalidConfig {
                provider: PROVIDER_NAME,
                message: "cache provider `tuist` requires `account`".to_string(),
            });
        };
        if account.is_empty() {
            return Err(Error::InvalidConfig {
                provider: PROVIDER_NAME,
                message: "cache provider `tuist` requires non-empty `account`".to_string(),
            });
        }
        if matches!(config.project.as_deref(), Some("")) {
            return Err(Error::InvalidConfig {
                provider: PROVIDER_NAME,
                message: "cache provider `tuist` requires non-empty `project` when set".to_string(),
            });
        }
        let auth = TuistAuth::new(auth_root, &config);
        Ok(Self {
            local,
            client: Arc::default(),
            config,
            grpc_channel_cache: Arc::default(),
            stream_stall_timeout: STREAM_STALL_TIMEOUT,
            capabilities_cache: Arc::default(),
            known_remote_blobs: Arc::new(Mutex::new(HashMap::new())),
            publication_flights: Arc::new(Flights::default()),
            download_flights: Arc::new(Flights::default()),
            batch_downloads: Arc::default(),
            transfer_limit: Arc::new(Semaphore::new(MAX_PARALLEL_TRANSFERS)),
            budget: TransferBudget::default(),
            auth,
            auth_token_cache: Arc::default(),
            endpoint_override: env_token(TUIST_CACHE_ENDPOINT_ENV),
            execution_cache: Arc::default(),
            refreshed_token: Arc::default(),
        })
    }

    pub fn local(&self) -> &Cas {
        &self.local
    }

    /// Draw remote transfers from `budget` instead of this cache's own.
    pub fn set_transfer_budget(&mut self, budget: TransferBudget) {
        self.budget = budget;
        self.batch_downloads = Arc::default();
    }

    pub fn transfer_budget(&self) -> &TransferBudget {
        &self.budget
    }

    /// Memory for one metadata RPC attempt.
    async fn reserve_metadata(&self) -> Result<crate::budget::Reservation> {
        self.budget
            .reserve_memory(METADATA_RESPONSE_LIMIT as u64)
            .await
    }

    pub fn config(&self) -> &TuistCacheConfig {
        &self.config
    }

    pub async fn get_blob(&self, digest: &Digest) -> Result<Vec<u8>> {
        match self.local.get_blob(digest).await {
            Ok(bytes) => return Ok(bytes),
            Err(Error::BlobNotFound(_)) => {}
            // The store removed the bad copy; the remote one is authoritative.
            Err(error) if error.is_corrupt() => {
                tracing::warn!(blob_digest = %digest, error = %error, "refetching a corrupt local blob");
            }
            Err(error) => return Err(error),
        }

        if is_empty_blob(digest) {
            self.local.mirror_blob(digest, &[]).await?;
            return Ok(Vec::new());
        }

        let remote_digest = self.remote_blob_digest(digest).await?;
        if remote_digest.size_bytes > download::IN_MEMORY_BLOB_LIMIT {
            self.ensure_blob_local(digest).await?;
            return self.local.get_blob(digest).await;
        }
        let _flight = self.download_flights.acquire(*digest).await;
        match self.local.get_blob(digest).await {
            Ok(bytes) => return Ok(bytes),
            Err(Error::BlobNotFound(_)) => {}
            Err(error) if error.is_corrupt() => {}
            Err(error) => return Err(error),
        }
        let started = Instant::now();
        let bytes = self.get_blob_remote(digest, &remote_digest).await?;
        let mirrored = self.local.mirror_blob(digest, &bytes).await?;
        if mirrored != *digest {
            return Err(status::remote(
                "get blob",
                ErrorKind::Corrupt,
                format!("remote blob {digest} did not match requested digest"),
            ));
        }
        transfer::record(
            TransferDirection::Download,
            *digest,
            bytes.len() as u64,
            started.elapsed(),
        );
        Ok(bytes)
    }

    /// Materialize a blob, fetching it first when the local tier lacks it.
    /// A local copy that fails verification is removed by the store and
    /// fetched again once from the remote tier.
    pub async fn copy_blob_to_file(&self, digest: &Digest, destination: &Path) -> Result<()> {
        self.ensure_blob_local(digest).await?;
        match self.local.copy_blob_to_file(digest, destination).await {
            Err(error) if error.is_corrupt() => {
                tracing::warn!(blob_digest = %digest, error = %error, "refetching a corrupt local blob");
                self.ensure_blob_local(digest).await?;
                self.local.copy_blob_to_file(digest, destination).await
            }
            result => result,
        }
    }

    /// Mirror a blob the local tier already holds, streaming it. Best effort,
    /// like every remote write.
    pub(crate) async fn publish_local_blob(&self, digest: &Digest) {
        if let Err(error) = self.upload_local_blob(Some(digest), "put blob").await {
            tracing::warn!(
                blob_digest = %digest,
                error = %error,
                "failed to upload blob to remote cache"
            );
        }
    }

    pub async fn ensure_blob_local(&self, digest: &Digest) -> Result<()> {
        self.ensure_blob_local_guarded(digest, ()).await
    }

    pub(crate) async fn ensure_blob_local_guarded<G: Clone + Send + 'static>(
        &self,
        digest: &Digest,
        guard: G,
    ) -> Result<()> {
        if self.local.has_blob(digest).await? {
            return Ok(());
        }
        if is_empty_blob(digest) {
            self.local.mirror_blob(digest, &[]).await?;
            return Ok(());
        }
        let flight = self.download_flights.acquire(*digest).await;
        if self.local.has_blob(digest).await? {
            return Ok(());
        }
        let remote_digest = self.remote_blob_digest(digest).await?;
        if let Some(limit) = self.batch_download_limit().await {
            if u64::try_from(remote_digest.size_bytes).is_ok_and(|size| size <= limit) {
                return self
                    .enqueue_download(*digest, remote_digest, flight, guard, limit)
                    .await;
            }
        }
        let _flight = flight;
        let started = Instant::now();
        retry::grpc("get blob", || {
            self.stream_remote_blob(digest, &remote_digest, guard.clone())
        })
        .await?;
        transfer::record(
            TransferDirection::Download,
            *digest,
            u64::try_from(remote_digest.size_bytes).unwrap_or_default(),
            started.elapsed(),
        );
        Ok(())
    }

    pub async fn put_blob(&self, bytes: &[u8]) -> Result<Digest> {
        let digest = self.local.put_blob(bytes).await?;
        if let Err(error) = self.put_blob_remote(&digest, bytes).await {
            tracing::warn!(
                blob_digest = %digest,
                bytes = bytes.len(),
                error = %error,
                "failed to upload blob to remote cache"
            );
        }
        Ok(digest)
    }

    /// True if the blob is present locally or remotely. `false` means the
    /// remote tier confirmed its absence; a remote that cannot answer is an
    /// error, never "not present".
    pub async fn has_blob(&self, digest: &Digest) -> Result<bool> {
        if self.has_local_blob(digest).await? {
            return Ok(true);
        }
        if is_empty_blob(digest) {
            return Ok(true);
        }
        self.head_blob_remote(digest).await
    }

    /// Whether the local tier already holds the blob, without asking the
    /// remote one. For callers that only want to know whether something is
    /// available at no cost.
    pub async fn has_local_blob(&self, digest: &Digest) -> Result<bool> {
        self.local.has_blob(digest).await
    }

    /// Look up an action result. `None` means the remote tier confirmed there
    /// is no usable entry: none is recorded, or one is recorded but a blob it
    /// references is confirmed gone. Outages, refusals, and malformed entries
    /// are errors.
    pub async fn get_action_result(&self, action: &Digest) -> Result<Option<ActionResult>> {
        Ok(self
            .lookup_action_result(action)
            .await?
            .map(|hit| hit.result))
    }

    pub(crate) async fn lookup_action_result(
        &self,
        action: &Digest,
    ) -> Result<Option<ActionCacheHit>> {
        if let Some(result) = self.local.get_action_result(action).await? {
            tracing::debug!(action_digest = %action, tier = "local", "action cache hit");
            return Ok(Some(ActionCacheHit {
                result,
                blobs_available: false,
            }));
        }

        let Some(result) = self.get_action_result_remote(action).await? else {
            return Ok(None);
        };
        tracing::debug!(action_digest = %action, tier = "remote", "action cache hit");
        if let Err(error) = self.local.mirror_action_result(action, &result).await {
            tracing::warn!(
                action_digest = %action,
                error = %error,
                "failed to persist remote action result locally"
            );
        }
        Ok(Some(ActionCacheHit {
            result,
            blobs_available: true,
        }))
    }

    pub async fn put_action_result(&self, action: &Digest, result: &ActionResult) -> Result<()> {
        self.local.put_action_result(action, result).await?;
        if let Err(error) = self.put_action_result_remote(action, result).await {
            tracing::warn!(
                action_digest = %action,
                error = %error,
                "failed to upload action result to remote cache"
            );
        }
        Ok(())
    }

    pub async fn forget_action(&self, action: &Digest) -> Result<bool> {
        self.local.forget_action(action).await
    }

    async fn head_blob_remote(&self, digest: &Digest) -> Result<bool> {
        let sha256 = match self.remote_blob_digest(digest).await {
            Ok(digest) => digest,
            Err(Error::BlobNotFound(_)) => return Ok(false),
            Err(error) => return Err(error),
        };
        self.reapi_blob_exists(&sha256).await
    }

    async fn get_blob_remote(&self, digest: &Digest, sha256: &reapi::Digest) -> Result<Vec<u8>> {
        let Some(bytes) = self.read_reapi_blob(sha256, "get blob").await? else {
            tracing::debug!(blob_digest = %digest, "remote blob mapping points at a missing blob");
            return Err(Error::BlobNotFound(*digest));
        };
        tracing::debug!(blob_digest = %digest, bytes = bytes.len(), "fetched blob from remote cache");
        Ok(bytes)
    }

    async fn put_blob_remote(&self, digest: &Digest, bytes: &[u8]) -> Result<reapi::Digest> {
        if Digest::of_bytes(bytes) != *digest {
            return Err(status::remote(
                "put blob",
                ErrorKind::Corrupt,
                format!("blob body did not match digest {digest}"),
            ));
        }
        let _flight = self.publication_flights.acquire(*digest).await;
        if let Some(remote_digest) = self.known_remote_blobs.lock().await.get(digest).cloned() {
            return Ok(remote_digest);
        }
        let sha256 = sha256_digest(bytes)?;
        if self.reapi_blob_exists(&sha256).await? {
            tracing::debug!(blob_digest = %digest, bytes = bytes.len(), "remote blob already present");
        } else {
            let started = Instant::now();
            self.upload_reapi_blob(&sha256, bytes, "put blob").await?;
            transfer::record(
                TransferDirection::Upload,
                *digest,
                bytes.len() as u64,
                started.elapsed(),
            );
            tracing::debug!(blob_digest = %digest, bytes = bytes.len(), "uploaded blob to remote cache");
        }
        self.put_blob_mapping(digest, &sha256).await?;
        self.known_remote_blobs
            .lock()
            .await
            .insert(*digest, sha256.clone());
        Ok(sha256)
    }

    async fn get_action_result_remote(&self, action: &Digest) -> Result<Option<ActionResult>> {
        let mapping_digest = action_mapping_digest(action)?;
        let Some(result) = self
            .get_reapi_action_result(&mapping_digest, "get action result")
            .await?
        else {
            tracing::debug!(action_digest = %action, "action cache miss");
            return Ok(None);
        };
        match self.reapi_to_struo_action_result(result).await {
            Ok(result) => Ok(Some(result)),
            Err(error) if error.is_not_found() => {
                tracing::debug!(
                    action_digest = %action,
                    error = %error,
                    "remote action result references a missing blob; treating it as a miss"
                );
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    async fn put_action_result_remote(&self, action: &Digest, result: &ActionResult) -> Result<()> {
        let stdout_upload = self.upload_local_blob(result.stdout.as_ref(), "put action result");
        let stderr_upload = self.upload_local_blob(result.stderr.as_ref(), "put action result");
        let upload_permits = Arc::new(Semaphore::new(MAX_CONCURRENT_BLOB_TRANSFERS));
        let output_uploads = try_join_all(result.outputs.iter().map(|(path, digest)| {
            let upload_permits = Arc::clone(&upload_permits);
            async move {
                let _permit = upload_permits
                    .acquire()
                    .await
                    .expect("blob transfer semaphore is never closed");
                let reapi_digest = self
                    .upload_local_blob(Some(digest), "put action result")
                    .await?;
                Ok::<_, Error>(reapi::OutputFile {
                    path: path.clone(),
                    digest: reapi_digest,
                    ..Default::default()
                })
            }
        }));
        let (stdout_digest, stderr_digest, output_files) =
            tokio::try_join!(stdout_upload, stderr_upload, output_uploads)?;

        let reapi_result = reapi::ActionResult {
            output_files,
            exit_code: result.exit_code,
            stdout_digest,
            stderr_digest,
            execution_metadata: Some(reapi::ExecutedActionMetadata {
                auxiliary_metadata: vec![encode_action_result_metadata(result)?],
                ..Default::default()
            }),
            ..Default::default()
        };
        let mapping_digest = action_mapping_digest(action)?;
        tracing::debug!(
            action_digest = %action,
            output_count = result.outputs.len(),
            "uploading action result to remote cache"
        );
        self.update_reapi_action_result(&mapping_digest, reapi_result, "put action result")
            .await
    }

    async fn upload_local_blob(
        &self,
        digest: Option<&Digest>,
        operation: &'static str,
    ) -> Result<Option<reapi::Digest>> {
        let Some(digest) = digest else {
            return Ok(None);
        };
        let _flight = self.publication_flights.acquire(*digest).await;
        if let Some(remote_digest) = self.known_remote_blobs.lock().await.get(digest).cloned() {
            return Ok(Some(remote_digest));
        }

        // The upload reads from a verified copy in temporary space, which
        // counts against the staging budget like a download does.
        let _staging = self
            .budget
            .reserve_staging(self.local.blob_size(digest).await?)
            .await?;
        let directory = tempfile::tempdir().map_err(|source| Error::Io {
            path: std::env::temp_dir(),
            source,
        })?;
        let path = directory.path().join("blob");
        let (struo_digest, remote_digest) = self
            .local
            .copy_blob_to_file_with(digest, &path, copy_blob_digests)
            .await?;
        if struo_digest != *digest {
            return Err(status::remote(
                operation,
                ErrorKind::Corrupt,
                format!("local blob body did not match digest {digest}"),
            ));
        }

        if !self.reapi_blob_exists(&remote_digest).await? {
            // Budget before the transfer slot, the order every transfer uses.
            let _memory = self
                .budget
                .reserve_memory(file_upload_memory(remote_digest.size_bytes))
                .await?;
            let _permit = self
                .transfer_limit
                .acquire()
                .await
                .expect("transfer semaphore remains open");
            let started = Instant::now();
            self.write_reapi_blob_file_stream(&remote_digest, &path, operation)
                .await?;
            transfer::record(
                TransferDirection::Upload,
                *digest,
                u64::try_from(remote_digest.size_bytes).unwrap_or_default(),
                started.elapsed(),
            );
        }
        self.put_blob_mapping(digest, &remote_digest).await?;
        self.known_remote_blobs
            .lock()
            .await
            .insert(*digest, remote_digest.clone());
        Ok(Some(remote_digest))
    }

    async fn put_blob_mapping(&self, digest: &Digest, sha256: &reapi::Digest) -> Result<()> {
        let action_digest = blob_mapping_digest(digest)?;
        let action_result = reapi::ActionResult {
            output_files: vec![reapi::OutputFile {
                path: BLOB_MAPPING_OUTPUT_PATH.to_string(),
                digest: Some(sha256.clone()),
                ..Default::default()
            }],
            ..Default::default()
        };
        self.update_reapi_action_result(&action_digest, action_result, "put blob mapping")
            .await
    }

    async fn get_blob_mapping(&self, digest: &Digest) -> Result<reapi::Digest> {
        let action_digest = blob_mapping_digest(digest)?;
        let Some(action_result) = self
            .get_reapi_action_result(&action_digest, "get blob mapping")
            .await?
        else {
            return Err(Error::BlobNotFound(*digest));
        };
        action_result
            .output_files
            .into_iter()
            .find(|file| file.path == BLOB_MAPPING_OUTPUT_PATH)
            .and_then(|file| file.digest)
            .ok_or_else(|| {
                status::remote(
                    "get blob mapping",
                    ErrorKind::Corrupt,
                    format!("remote blob mapping for {digest} is missing its REAPI digest"),
                )
            })
    }

    async fn remote_blob_digest(&self, digest: &Digest) -> Result<reapi::Digest> {
        if let Some(remote_digest) = self.known_remote_blobs.lock().await.get(digest).cloned() {
            return Ok(remote_digest);
        }
        let remote_digest = self.get_blob_mapping(digest).await?;
        self.known_remote_blobs
            .lock()
            .await
            .insert(*digest, remote_digest.clone());
        Ok(remote_digest)
    }

    async fn reapi_to_struo_action_result(
        &self,
        result: reapi::ActionResult,
    ) -> Result<ActionResult> {
        if let Some(native) = decode_action_result_metadata(&result)? {
            self.remember_remote_action_result_blobs(&result, &native)
                .await?;
            return Ok(native);
        }

        let stdout_mirror = self.mirror_reapi_digest_or_raw(
            result.stdout_digest.as_ref(),
            &result.stdout_raw,
            "get action result",
        );
        let stderr_mirror = self.mirror_reapi_digest_or_raw(
            result.stderr_digest.as_ref(),
            &result.stderr_raw,
            "get action result",
        );
        let mirror_permits = Arc::new(Semaphore::new(MAX_CONCURRENT_BLOB_TRANSFERS));
        let output_mirrors = try_join_all(result.output_files.iter().map(|output_file| {
            let mirror_permits = Arc::clone(&mirror_permits);
            async move {
                let _permit = mirror_permits
                    .acquire()
                    .await
                    .expect("blob transfer semaphore is never closed");
                let digest = match output_file.digest.as_ref() {
                    Some(reapi_digest) => Some(
                        self.mirror_reapi_blob(reapi_digest, "get action result")
                            .await?,
                    ),
                    None if !output_file.contents.is_empty() => {
                        Some(self.local.put_blob(&output_file.contents).await?)
                    }
                    None => None,
                };
                Ok::<_, Error>(digest.map(|digest| (output_file.path.clone(), digest)))
            }
        }));
        let (stdout, stderr, output_pairs) =
            tokio::try_join!(stdout_mirror, stderr_mirror, output_mirrors)?;
        let outputs: BTreeMap<String, Digest> = output_pairs.into_iter().flatten().collect();

        Ok(ActionResult {
            exit_code: result.exit_code,
            stdout,
            stderr,
            outputs,
        })
    }

    async fn remember_remote_action_result_blobs(
        &self,
        remote: &reapi::ActionResult,
        native: &ActionResult,
    ) -> Result<()> {
        if remote.exit_code != native.exit_code {
            return Err(remote_action_metadata_error(
                "native and remote exit codes do not match",
            ));
        }

        let remote_outputs = remote
            .output_files
            .iter()
            .filter_map(|output| {
                output
                    .digest
                    .as_ref()
                    .map(|digest| (output.path.as_str(), digest))
            })
            .collect::<BTreeMap<_, _>>();
        if remote_outputs.len() != native.outputs.len() {
            return Err(remote_action_metadata_error(
                "native and remote output sets do not match",
            ));
        }

        let mut mappings = HashMap::new();
        let mut remote_digests = Vec::with_capacity(native.outputs.len() + 2);
        for (path, native_digest) in &native.outputs {
            let remote_digest = remote_outputs.get(path.as_str()).ok_or_else(|| {
                remote_action_metadata_error(&format!(
                    "remote action result is missing native output `{path}`"
                ))
            })?;
            remote_digests.push((*remote_digest).clone());
            mappings.insert(*native_digest, (*remote_digest).clone());
        }
        remember_optional_remote_blob(
            &mut mappings,
            native.stdout.as_ref(),
            remote.stdout_digest.as_ref(),
            "stdout",
        )?;
        remember_optional_remote_blob(
            &mut mappings,
            native.stderr.as_ref(),
            remote.stderr_digest.as_ref(),
            "stderr",
        )?;
        remote_digests.extend(remote.stdout_digest.iter().cloned());
        remote_digests.extend(remote.stderr_digest.iter().cloned());
        if let Some(missing) = self.find_missing_reapi_blobs(remote_digests).await? {
            let native = mappings
                .iter()
                .find_map(|(native, remote)| (*remote == missing).then_some(*native));
            return Err(match native {
                Some(native) => Error::BlobNotFound(native),
                None => status::remote(
                    "get action result",
                    ErrorKind::NotFound,
                    format!(
                        "remote action result points at missing blob {}",
                        digest_key(&missing)
                    ),
                ),
            });
        }
        self.known_remote_blobs.lock().await.extend(mappings);
        Ok(())
    }

    async fn mirror_reapi_digest_or_raw(
        &self,
        digest: Option<&reapi::Digest>,
        raw: &[u8],
        operation: &'static str,
    ) -> Result<Option<Digest>> {
        if let Some(digest) = digest {
            return self.mirror_reapi_blob(digest, operation).await.map(Some);
        }
        if raw.is_empty() {
            return Ok(None);
        }
        self.local.put_blob(raw).await.map(Some)
    }

    async fn mirror_reapi_blob(
        &self,
        digest: &reapi::Digest,
        operation: &'static str,
    ) -> Result<Digest> {
        if digest.size_bytes == 0 {
            validate_sha256_digest(digest, &[], operation)?;
            let local_digest = Digest::of_bytes(&[]);
            self.local.mirror_blob(&local_digest, &[]).await?;
            self.known_remote_blobs
                .lock()
                .await
                .insert(local_digest, digest.clone());
            return Ok(local_digest);
        }
        let started = Instant::now();
        if digest.size_bytes > download::IN_MEMORY_BLOB_LIMIT {
            let staged =
                retry::grpc(operation, || self.stage_remote_blob(digest, operation)).await?;
            let size = staged.len();
            let local_digest = self.local.commit_staged(staged).await?;
            transfer::record(
                TransferDirection::Download,
                local_digest,
                size,
                started.elapsed(),
            );
            self.known_remote_blobs
                .lock()
                .await
                .insert(local_digest, digest.clone());
            return Ok(local_digest);
        }
        let Some(bytes) = self.read_reapi_blob(digest, operation).await? else {
            return Err(status::remote(
                operation,
                ErrorKind::NotFound,
                format!(
                    "remote action result points at missing blob {}",
                    digest_key(digest)
                ),
            ));
        };
        let local_digest = Digest::of_bytes(&bytes);
        self.local.mirror_blob(&local_digest, &bytes).await?;
        transfer::record(
            TransferDirection::Download,
            local_digest,
            bytes.len() as u64,
            started.elapsed(),
        );
        self.known_remote_blobs
            .lock()
            .await
            .insert(local_digest, digest.clone());
        Ok(local_digest)
    }

    async fn upload_reapi_blob(
        &self,
        digest: &reapi::Digest,
        bytes: &[u8],
        operation: &'static str,
    ) -> Result<()> {
        // The encoded body, the copy each attempt sends, and one chunk.
        let len = bytes.len() as u64;
        let _memory = self
            .budget
            .reserve_memory(
                len.saturating_mul(2)
                    .saturating_add(len.min(BYTE_STREAM_CHUNK_SIZE as u64)),
            )
            .await?;
        let _permit = self
            .transfer_limit
            .acquire()
            .await
            .expect("transfer semaphore remains open");
        if bytes.len() > BATCH_BLOB_LIMIT {
            return self.write_reapi_blob_stream(digest, bytes, operation).await;
        }
        let capabilities = self.remote_capabilities().await;
        let (data, compressor) = encode_reapi_blob(bytes, capabilities.zstd_batches, operation)?;
        let channel = self.grpc_channel().await?;
        let client = ContentAddressableStorageClient::new(channel);
        let request = reapi::BatchUpdateBlobsRequest {
            instance_name: self.instance_name(),
            requests: vec![reapi::batch_update_blobs_request::Request {
                digest: Some(digest.clone()),
                data,
                compressor,
            }],
            digest_function: SHA256_DIGEST_FUNCTION,
        };
        retry::grpc(operation, || {
            let mut client = client.clone();
            let request = request.clone();
            async move {
                let response = client
                    .batch_update_blobs(self.authorized_grpc_request(request, operation).await?)
                    .await?
                    .into_inner();
                batch_upload_outcome(&response, operation)
            }
        })
        .await
    }

    async fn write_reapi_blob_stream(
        &self,
        digest: &reapi::Digest,
        bytes: &[u8],
        operation: &'static str,
    ) -> Result<()> {
        retry::grpc(operation, || {
            self.write_reapi_blob_stream_attempt(digest, bytes, operation)
        })
        .await
    }

    async fn write_reapi_blob_stream_attempt(
        &self,
        digest: &reapi::Digest,
        bytes: &[u8],
        operation: &'static str,
    ) -> std::result::Result<(), retry::Failure> {
        let capabilities = self.remote_capabilities().await;
        let (body, compressor) = encode_reapi_blob(bytes, capabilities.zstd_streams, operation)?;
        let channel = self.grpc_upload_channel().await?;
        let mut client = ByteStreamClient::new(channel);
        let resource_name =
            byte_stream_write_resource(&self.instance_name(), digest, Uuid::now_v7(), compressor);
        let body = Arc::new(body);
        let total = body.len();
        let requests = stream::unfold(
            (body, resource_name, 0usize),
            |(body, resource_name, offset)| async move {
                if offset >= body.len() {
                    return None;
                }
                let end = (offset + BYTE_STREAM_CHUNK_SIZE).min(body.len());
                let request = bytestream::WriteRequest {
                    resource_name: if offset == 0 {
                        resource_name.clone()
                    } else {
                        String::new()
                    },
                    write_offset: i64::try_from(offset)
                        .expect("blob offset fits the remote digest size"),
                    finish_write: end == body.len(),
                    data: body[offset..end].to_vec(),
                };
                Some((request, (body, resource_name, end)))
            },
        );
        let progress = stall::Progress::new();
        let request = self
            .authorized_grpc_request(stall::tracked(requests, Arc::clone(&progress)), operation)
            .await?;
        let response =
            stall::unless_stalled(&progress, self.stream_stall_timeout, client.write(request))
                .await?
                .into_inner();
        if ![
            -1,
            digest.size_bytes,
            i64::try_from(total).unwrap_or(i64::MAX),
        ]
        .contains(&response.committed_size)
        {
            return Err(status::remote(
                operation,
                ErrorKind::Protocol,
                format!(
                    "byte stream committed {} of {total} bytes",
                    response.committed_size
                ),
            )
            .into());
        }
        Ok(())
    }

    async fn write_reapi_blob_file_stream(
        &self,
        digest: &reapi::Digest,
        path: &Path,
        operation: &'static str,
    ) -> Result<()> {
        retry::grpc(operation, || {
            self.write_reapi_blob_file_stream_attempt(digest, path, operation)
        })
        .await
    }

    async fn write_reapi_blob_file_stream_attempt(
        &self,
        digest: &reapi::Digest,
        path: &Path,
        operation: &'static str,
    ) -> std::result::Result<(), retry::Failure> {
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|source| Error::Io {
                path: path.to_path_buf(),
                source,
            })?
            .into_std()
            .await;
        let compressor = if self.remote_capabilities().await.zstd_streams {
            reapi::compressor::Value::Zstd
        } else {
            reapi::compressor::Value::Identity
        };
        let channel = self.grpc_upload_channel().await?;
        let mut client = ByteStreamClient::new(channel);
        let resource_name = byte_stream_write_resource(
            &self.instance_name(),
            digest,
            Uuid::now_v7(),
            compressor as i32,
        );
        let total = digest.size_bytes;
        let body = file_body::chunks(
            file,
            u64::try_from(total).unwrap_or_default(),
            compressor == reapi::compressor::Value::Zstd,
            BYTE_STREAM_CHUNK_SIZE,
        );
        let read_error = Arc::new(Mutex::new(None::<String>));
        let sent = Arc::new(AtomicI64::new(0));
        let requests = stream::unfold(
            (
                body,
                resource_name,
                0_i64,
                Arc::clone(&read_error),
                Arc::clone(&sent),
            ),
            |(mut body, resource_name, offset, read_error, sent)| async move {
                let chunk = match body.recv().await? {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        *read_error.lock().await = Some(error.to_string());
                        return None;
                    }
                };
                let end =
                    offset.saturating_add(i64::try_from(chunk.data.len()).unwrap_or(i64::MAX));
                sent.store(end, Ordering::Relaxed);
                let request = bytestream::WriteRequest {
                    resource_name: if offset == 0 {
                        resource_name.clone()
                    } else {
                        String::new()
                    },
                    write_offset: offset,
                    finish_write: chunk.last,
                    data: chunk.data,
                };
                Some((request, (body, resource_name, end, read_error, sent)))
            },
        );
        let progress = stall::Progress::new();
        let request = self
            .authorized_grpc_request(stall::tracked(requests, Arc::clone(&progress)), operation)
            .await?;
        let response =
            stall::unless_stalled(&progress, self.stream_stall_timeout, client.write(request))
                .await;
        if let Some(message) = read_error.lock().await.take() {
            return Err(Error::Io {
                path: path.to_path_buf(),
                source: std::io::Error::other(message),
            }
            .into());
        }
        let response = response?.into_inner();
        if ![-1, total, sent.load(Ordering::Relaxed)].contains(&response.committed_size) {
            return Err(status::remote(
                operation,
                ErrorKind::Protocol,
                format!(
                    "byte stream committed {} of {total} bytes",
                    response.committed_size
                ),
            )
            .into());
        }
        Ok(())
    }

    /// Compression support, discovered once. Without it transfers still work
    /// uncompressed, so a failure only costs compression: a server that
    /// answered but not usefully is remembered as supporting none, and an
    /// outage is asked about again after the usual cooldown.
    async fn remote_capabilities(&self) -> RemoteCapabilities {
        let this = self.clone();
        let discovered = self
            .capabilities_cache
            .get_or_try_init(move || async move {
                match this.fetch_remote_capabilities().await {
                    Err(error) if error.kind() == ErrorKind::Protocol => {
                        tracing::warn!(error = %error, "remote cache capabilities are unusable");
                        Ok(RemoteCapabilities::default())
                    }
                    discovered => discovered,
                }
            })
            .await;
        match discovered {
            Ok(capabilities) => *capabilities,
            Err(error) => {
                tracing::warn!(error = %error, "failed to discover remote cache capabilities");
                RemoteCapabilities::default()
            }
        }
    }

    async fn fetch_remote_capabilities(&self) -> Result<RemoteCapabilities> {
        let channel = self.grpc_channel().await?;
        let client = CapabilitiesClient::new(channel);
        let request = reapi::GetCapabilitiesRequest {
            instance_name: self.instance_name(),
        };
        let response = retry::grpc("discover capabilities", || {
            let mut client = client.clone();
            let request = request.clone();
            async move {
                Ok(client
                    .get_capabilities(
                        self.authorized_grpc_request(request, "discover capabilities")
                            .await?,
                    )
                    .await?)
            }
        })
        .await?
        .into_inner();
        let Some(capabilities) = response.cache_capabilities else {
            return Ok(RemoteCapabilities::default());
        };
        let zstd = reapi::compressor::Value::Zstd as i32;
        Ok(RemoteCapabilities {
            zstd_streams: capabilities.supported_compressors.contains(&zstd),
            zstd_batches: capabilities
                .supported_batch_update_compressors
                .contains(&zstd),
            batch_read_limit: u64::try_from(capabilities.max_batch_total_size_bytes).ok(),
        })
    }

    async fn reapi_blob_exists(&self, digest: &reapi::Digest) -> Result<bool> {
        let channel = self.grpc_channel().await?;
        let client = ContentAddressableStorageClient::new(channel)
            .max_decoding_message_size(METADATA_RESPONSE_LIMIT);
        let request = reapi::FindMissingBlobsRequest {
            instance_name: self.instance_name(),
            blob_digests: vec![digest.clone()],
            digest_function: SHA256_DIGEST_FUNCTION,
        };
        let response = retry::grpc("head blob", || {
            let mut client = client.clone();
            let request = request.clone();
            async move {
                let _memory = self.reserve_metadata().await?;
                Ok(client
                    .find_missing_blobs(self.authorized_grpc_request(request, "head blob").await?)
                    .await?)
            }
        })
        .await?
        .into_inner();
        Ok(response.missing_blob_digests.is_empty())
    }

    /// The first of `digests` the remote tier confirms it does not hold.
    async fn find_missing_reapi_blobs(
        &self,
        digests: Vec<reapi::Digest>,
    ) -> Result<Option<reapi::Digest>> {
        if digests.is_empty() {
            return Ok(None);
        }
        let channel = self.grpc_channel().await?;
        let client = ContentAddressableStorageClient::new(channel)
            .max_decoding_message_size(METADATA_RESPONSE_LIMIT);
        let request = reapi::FindMissingBlobsRequest {
            instance_name: self.instance_name(),
            blob_digests: digests,
            digest_function: SHA256_DIGEST_FUNCTION,
        };
        let response = retry::grpc("get action result", || {
            let mut client = client.clone();
            let request = request.clone();
            async move {
                let _memory = self.reserve_metadata().await?;
                Ok(client
                    .find_missing_blobs(
                        self.authorized_grpc_request(request, "get action result")
                            .await?,
                    )
                    .await?)
            }
        })
        .await?
        .into_inner();
        Ok(response.missing_blob_digests.into_iter().next())
    }

    /// `None` only when the action cache answers `NOT_FOUND`.
    async fn get_reapi_action_result(
        &self,
        action_digest: &reapi::Digest,
        operation: &'static str,
    ) -> Result<Option<reapi::ActionResult>> {
        let channel = self.grpc_channel().await?;
        let client =
            ActionCacheClient::new(channel).max_decoding_message_size(METADATA_RESPONSE_LIMIT);
        let request = reapi::GetActionResultRequest {
            instance_name: self.instance_name(),
            action_digest: Some(action_digest.clone()),
            inline_stdout: false,
            inline_stderr: false,
            inline_output_files: Vec::new(),
            digest_function: SHA256_DIGEST_FUNCTION,
        };
        retry::grpc(operation, || {
            let mut client = client.clone();
            let request = request.clone();
            async move {
                let _memory = self.reserve_metadata().await?;
                match client
                    .get_action_result(self.authorized_grpc_request(request, operation).await?)
                    .await
                {
                    Ok(response) => Ok(Some(response.into_inner())),
                    Err(status) if status.code() == Code::NotFound => Ok(None),
                    Err(status) => Err(status.into()),
                }
            }
        })
        .await
    }

    async fn update_reapi_action_result(
        &self,
        action_digest: &reapi::Digest,
        action_result: reapi::ActionResult,
        operation: &'static str,
    ) -> Result<()> {
        let channel = self.grpc_channel().await?;
        let client =
            ActionCacheClient::new(channel).max_decoding_message_size(METADATA_RESPONSE_LIMIT);
        let request = reapi::UpdateActionResultRequest {
            instance_name: self.instance_name(),
            action_digest: Some(action_digest.clone()),
            action_result: Some(action_result),
            results_cache_policy: None,
            digest_function: SHA256_DIGEST_FUNCTION,
        };
        retry::grpc(operation, || {
            let mut client = client.clone();
            let request = request.clone();
            async move {
                let _memory = self.reserve_metadata().await?;
                Ok(client
                    .update_action_result(self.authorized_grpc_request(request, operation).await?)
                    .await?)
            }
        })
        .await?;
        Ok(())
    }
}

fn local_mirror_error(source: &std::io::Error) -> retry::Failure {
    status::remote("get blob", ErrorKind::Io, source.to_string()).into()
}

/// Per-blob statuses use the gRPC code space, so a transient one is retried
/// like a failed RPC. Permanent ones keep the REAPI status wording.
fn batch_upload_outcome(
    response: &reapi::BatchUpdateBlobsResponse,
    operation: &'static str,
) -> std::result::Result<(), retry::Failure> {
    let Some(status) = response
        .responses
        .first()
        .and_then(|response| response.status.as_ref())
    else {
        return Err(status::remote(
            operation,
            ErrorKind::Protocol,
            "Kura returned no status for blob upload",
        )
        .into());
    };
    if status.code == REAPI_STATUS_OK {
        return Ok(());
    }
    let rpc = super::retry_hint::from_rpc(status);
    if retry::retryable(&rpc) {
        Err(rpc.into())
    } else {
        Err(status::rpc_status_error(operation, status).into())
    }
}

fn encode_action_result_metadata(
    result: &ActionResult,
) -> Result<bazel_remote_apis::google::protobuf::Any> {
    let value = serde_json::to_vec(result).map_err(|source| {
        status::remote(
            "put action result",
            ErrorKind::Other,
            format!("serializing native action result metadata: {source}"),
        )
    })?;
    Ok(bazel_remote_apis::google::protobuf::Any {
        type_url: ACTION_RESULT_METADATA_TYPE_URL.to_string(),
        value,
    })
}

fn decode_action_result_metadata(result: &reapi::ActionResult) -> Result<Option<ActionResult>> {
    let Some(metadata) = result.execution_metadata.as_ref().and_then(|metadata| {
        metadata
            .auxiliary_metadata
            .iter()
            .find(|value| value.type_url == ACTION_RESULT_METADATA_TYPE_URL)
    }) else {
        return Ok(None);
    };
    serde_json::from_slice(&metadata.value)
        .map(Some)
        .map_err(|source| {
            remote_action_metadata_error(&format!(
                "decoding native action result metadata: {source}"
            ))
        })
}

fn remember_optional_remote_blob(
    mappings: &mut HashMap<Digest, reapi::Digest>,
    native: Option<&Digest>,
    remote: Option<&reapi::Digest>,
    stream: &str,
) -> Result<()> {
    match (native, remote) {
        (Some(native), Some(remote)) => {
            mappings.insert(*native, remote.clone());
            Ok(())
        }
        (None, None) => Ok(()),
        _ => Err(remote_action_metadata_error(&format!(
            "native and remote {stream} digests do not match"
        ))),
    }
}

/// A remote entry whose metadata is inconsistent is reported as corrupt, not
/// trusted and not mistaken for a miss.
fn remote_action_metadata_error(message: &str) -> Error {
    status::remote("get action result", ErrorKind::Corrupt, message)
}

fn copy_blob_digests(
    reader: &mut dyn std::io::Read,
    writer: &mut dyn std::io::Write,
) -> std::io::Result<(Digest, reapi::Digest)> {
    let mut struo = blake3::Hasher::new();
    let mut sha256 = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        struo.update(&buffer[..read]);
        sha256.update(&buffer[..read]);
        size = size.saturating_add(read as u64);
    }
    let size_bytes = i64::try_from(size).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "blob exceeds the remote size range",
        )
    })?;
    Ok((
        Digest::from_bytes(*struo.finalize().as_bytes()),
        reapi::Digest {
            hash: hex::encode(sha256.finalize()),
            size_bytes,
        },
    ))
}

fn sha256_digest(bytes: &[u8]) -> Result<reapi::Digest> {
    Ok(reapi::Digest {
        hash: hex::encode(Sha256::digest(bytes)),
        size_bytes: size_bytes_i64(bytes.len())?,
    })
}

fn validate_sha256_digest(
    digest: &reapi::Digest,
    bytes: &[u8],
    operation: &'static str,
) -> Result<()> {
    let actual = sha256_digest(bytes)?;
    if actual == *digest {
        Ok(())
    } else {
        Err(status::remote(
            operation,
            ErrorKind::Corrupt,
            format!(
                "remote blob {} did not match expected digest {}",
                digest_key(&actual),
                digest_key(digest)
            ),
        ))
    }
}

fn encode_reapi_blob(
    bytes: &[u8],
    supports_zstd: bool,
    operation: &'static str,
) -> Result<(Vec<u8>, i32)> {
    if supports_zstd {
        let compressed = zstd::stream::encode_all(bytes, 0).map_err(|source| {
            status::remote(
                operation,
                ErrorKind::Other,
                format!("compressing blob: {source}"),
            )
        })?;
        if compressed.len() < bytes.len() {
            return Ok((compressed, reapi::compressor::Value::Zstd as i32));
        }
    }
    Ok((bytes.to_vec(), reapi::compressor::Value::Identity as i32))
}

fn decode_reapi_blob(
    data: Vec<u8>,
    compressor: i32,
    digest: &reapi::Digest,
    operation: &'static str,
) -> Result<Vec<u8>> {
    let bytes = match reapi::compressor::Value::try_from(compressor) {
        Ok(reapi::compressor::Value::Identity) => data,
        Ok(reapi::compressor::Value::Zstd) => {
            let expected = usize::try_from(digest.size_bytes).map_err(|_| {
                status::remote(
                    operation,
                    ErrorKind::Protocol,
                    format!(
                        "remote blob has invalid size in digest {}",
                        digest_key(digest)
                    ),
                )
            })?;
            let limit = u64::try_from(expected)
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            let mut decompressor =
                zstd::stream::read::Decoder::new(data.as_slice()).map_err(|source| {
                    status::remote(
                        operation,
                        ErrorKind::Corrupt,
                        format!("opening compressed blob: {source}"),
                    )
                })?;
            decompressor.window_log_max(23).map_err(|source| {
                status::remote(
                    operation,
                    ErrorKind::Corrupt,
                    format!("limiting compressed blob window: {source}"),
                )
            })?;
            let mut uncompressed = Vec::with_capacity(expected.min(BYTE_STREAM_MESSAGE_LIMIT));
            decompressor
                .take(limit)
                .read_to_end(&mut uncompressed)
                .map_err(|source| {
                    status::remote(
                        operation,
                        ErrorKind::Corrupt,
                        format!("decompressing blob: {source}"),
                    )
                })?;
            uncompressed
        }
        _ => {
            return Err(status::remote(
                operation,
                ErrorKind::Protocol,
                format!("unsupported remote blob compressor {compressor}"),
            ));
        }
    };
    validate_sha256_digest(digest, &bytes, operation)?;
    Ok(bytes)
}

fn blob_mapping_digest(digest: &Digest) -> Result<reapi::Digest> {
    mapping_digest(BLOB_MAPPING_PREFIX, digest)
}

fn action_mapping_digest(digest: &Digest) -> Result<reapi::Digest> {
    mapping_digest(ACTION_MAPPING_PREFIX, digest)
}

fn mapping_digest(prefix: &str, digest: &Digest) -> Result<reapi::Digest> {
    let preimage = format!("{prefix}\0{}", digest.to_hex());
    sha256_digest(preimage.as_bytes())
}

fn digest_key(digest: &reapi::Digest) -> String {
    format!("{}/{}", digest.hash, digest.size_bytes)
}

fn byte_stream_read_resource(
    instance_name: &str,
    digest: &reapi::Digest,
    compressor: i32,
) -> String {
    let suffix = if compressor == reapi::compressor::Value::Zstd as i32 {
        format!("compressed-blobs/zstd/{}", digest_key(digest))
    } else {
        format!("blobs/{}", digest_key(digest))
    };
    resource_name(instance_name, &suffix)
}

fn byte_stream_write_resource(
    instance_name: &str,
    digest: &reapi::Digest,
    upload_id: Uuid,
    compressor: i32,
) -> String {
    let suffix = if compressor == reapi::compressor::Value::Zstd as i32 {
        format!(
            "uploads/{upload_id}/compressed-blobs/zstd/{}",
            digest_key(digest)
        )
    } else {
        format!("uploads/{upload_id}/blobs/{}", digest_key(digest))
    };
    resource_name(instance_name, &suffix)
}

fn resource_name(instance_name: &str, suffix: &str) -> String {
    if instance_name.is_empty() {
        suffix.to_string()
    } else {
        format!("{instance_name}/{suffix}")
    }
}

fn size_bytes_i64(size: usize) -> Result<i64> {
    let Ok(size) = i64::try_from(size) else {
        return Err(status::remote(
            "compute digest",
            ErrorKind::Protocol,
            format!("blob size exceeds REAPI size limit: {size} bytes"),
        ));
    };
    Ok(size)
}

fn non_empty_str(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn is_empty_blob(digest: &Digest) -> bool {
    *digest == Digest::of_bytes(&[])
}

#[cfg(test)]
#[path = "cache_stream_tests.rs"]
mod stream_tests;

#[cfg(test)]
#[path = "cache_upload_tests.rs"]
mod upload_tests;

#[cfg(test)]
mod fault_server;

#[cfg(test)]
mod fault_tests;

#[cfg(test)]
mod budget_tests;
#[cfg(test)]
#[path = "cache/staged_integrity_tests.rs"]
mod staged_integrity_tests;

#[cfg(test)]
mod redaction_tests;

#[cfg(test)]
mod tests;
