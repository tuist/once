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

mod batch_download;
mod batch_rpc;
mod download;
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
const BLOB_MAPPING_PREFIX: &str = "once.blob.v1";
const ACTION_MAPPING_PREFIX: &str = "once.action.v1";
const ACTION_RESULT_METADATA_TYPE_URL: &str = "type.googleapis.com/once.cache.v1.ActionResult";
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
}

pub use execution::{TuistExecution, DEFAULT_EXECUTION_TIMEOUT};

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
        match self.reapi_to_once_action_result(result).await {
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
        let (once_digest, remote_digest) = self
            .local
            .copy_blob_to_file_with(digest, &path, copy_blob_digests)
            .await?;
        if once_digest != *digest {
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

    async fn reapi_to_once_action_result(
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

    async fn data_plane_endpoint(&self) -> Result<String> {
        if let Some(endpoint) = &self.endpoint_override {
            tracing::debug!(
                endpoint = %redact::endpoint(endpoint),
                "using Tuist cache endpoint override"
            );
            return Ok(endpoint.clone());
        }
        let endpoints = self.fetch_endpoints().await?;
        match endpoints.as_slice() {
            [] => Err(status::remote(
                "discover endpoints",
                ErrorKind::Protocol,
                "Tuist returned no Kura cache endpoints",
            )),
            [endpoint] => Ok(endpoint.clone()),
            _ => self.pick_fastest_endpoint(&endpoints).await,
        }
    }

    /// Resolves the account's cache endpoints the way the Tuist CLI and Gradle
    /// plugin do: the answer is persisted next to the local cache and reused
    /// across Once processes until the `max-age` the server sends expires, so
    /// most commands never ask. A request that cannot reach Tuist falls back to
    /// the last answer, even an expired one, and otherwise reports the
    /// remote tier as unavailable.
    async fn fetch_endpoints(&self) -> Result<Vec<String>> {
        let persisted = self.persisted_endpoints().await;
        if let Some(persisted) = &persisted {
            if persisted.is_fresh() {
                return Ok(persisted.endpoints.clone());
            }
        }
        match self.request_endpoints().await {
            Ok((endpoints, max_age)) => {
                self.persist_endpoints(&endpoints, max_age).await;
                Ok(endpoints)
            }
            Err(error) => match persisted {
                Some(stale) if error.is_unavailable() => {
                    tracing::warn!(
                        error = %error,
                        "using the last known Tuist cache endpoints because discovery is unreachable"
                    );
                    Ok(stale.endpoints)
                }
                _ => Err(error),
            },
        }
    }

    async fn request_endpoints(&self) -> Result<(Vec<String>, Option<Duration>)> {
        let url = self.endpoints_url()?;
        let token = self.auth_token().await?;
        let response = self
            .authorized_request(Method::GET, url, token)
            .await?
            .header(KURA_FEATURE_FLAGS_HEADER, KURA_FEATURE_FLAG)
            .send()
            .await
            .map_err(|source| {
                status::remote(
                    "reach endpoint discovery",
                    ErrorKind::Unavailable,
                    redact::transport(&source),
                )
            })?;
        let status = response.status();
        if status.is_success() {
            let max_age = cache_control_max_age(response.headers());
            let body = response.bytes().await.map_err(|source| {
                status::remote(
                    "reach endpoint discovery",
                    ErrorKind::Unavailable,
                    redact::transport(&source),
                )
            })?;
            // The decoding error can quote the body, so only its position is
            // reported.
            let endpoints: EndpointResponse = serde_json::from_slice(&body).map_err(|source| {
                status::remote(
                    "decode endpoints",
                    ErrorKind::Protocol,
                    format!(
                        "malformed endpoint response at line {} column {}",
                        source.line(),
                        source.column()
                    ),
                )
            })?;
            return Ok((endpoints.endpoints, max_age));
        }
        // A server that cannot answer right now is unreachable; any other
        // refusal, such as an unknown account or a rejected token, has to
        // surface as what it is.
        let kind = status::http_kind(status);
        let operation = if kind == ErrorKind::Unavailable {
            "reach endpoint discovery"
        } else {
            "discover endpoints"
        };
        Err(status::remote(operation, kind, redact::http_status(status)))
    }

    fn persisted_endpoints_path(&self) -> std::path::PathBuf {
        let key = Sha256::digest(format!("{}\n{}", self.config.url, self.account()).as_bytes());
        let key = key.iter().fold(String::new(), |mut hex, byte| {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
            hex
        });
        self.local
            .root()
            .join("remote-endpoints")
            .join(format!("{key}.json"))
    }

    async fn persisted_endpoints(&self) -> Option<PersistedEndpoints> {
        let bytes = tokio::fs::read(self.persisted_endpoints_path())
            .await
            .ok()?;
        serde_json::from_slice::<PersistedEndpoints>(&bytes)
            .ok()
            .filter(|persisted| !persisted.endpoints.is_empty())
    }

    /// Best effort: failing to persist only means the next command asks again.
    async fn persist_endpoints(&self, endpoints: &[String], max_age: Option<Duration>) {
        if endpoints.is_empty() {
            return;
        }
        let path = self.persisted_endpoints_path();
        let persisted = PersistedEndpoints {
            endpoints: endpoints.to_vec(),
            expires_at_unix_ms: unix_ms_now()
                .saturating_add(duration_ms(max_age.unwrap_or(DEFAULT_ENDPOINTS_MAX_AGE))),
        };
        let write = async {
            let bytes = serde_json::to_vec(&persisted).map_err(std::io::Error::other)?;
            crate::filesystem::write_atomically(&path, &bytes)
                .await
                .map_err(std::io::Error::other)
        };
        if let Err(error) = write.await {
            tracing::debug!(error = %error, "could not persist Tuist cache endpoints");
        }
    }

    async fn pick_fastest_endpoint(&self, endpoints: &[String]) -> Result<String> {
        let mut set = tokio::task::JoinSet::new();
        for endpoint in endpoints {
            let endpoint = endpoint.clone();
            set.spawn(async move {
                let start = Instant::now();
                let outcome =
                    tokio::time::timeout(ENDPOINT_PROBE_TIMEOUT, connect_grpc_endpoint(&endpoint))
                        .await
                        .map_err(|_| "endpoint probe timed out".to_string())
                        .and_then(|result| result.map(|_| ()));
                match outcome {
                    Ok(()) => Ok((start.elapsed(), endpoint)),
                    Err(error) => Err(format!("{}: {error}", redact::endpoint(&endpoint))),
                }
            });
        }

        let mut best: Option<(Duration, String)> = None;
        let mut failures = Vec::new();
        while let Some(joined) = set.join_next().await {
            let Ok(result) = joined else {
                failures.push("probe task failed".to_string());
                continue;
            };
            let (elapsed, endpoint) = match result {
                Ok(candidate) => candidate,
                Err(error) => {
                    failures.push(error);
                    continue;
                }
            };
            match &best {
                Some((best_elapsed, _)) if *best_elapsed <= elapsed => {}
                _ => best = Some((elapsed, endpoint)),
            }
        }

        if !failures.is_empty() {
            tracing::warn!(
                unreachable = failures.len(),
                candidates = endpoints.len(),
                "some Tuist cache endpoints were unreachable"
            );
        }
        best.map(|(elapsed, endpoint)| {
            tracing::debug!(
                endpoint = %redact::endpoint(&endpoint),
                probe_ms = elapsed.as_millis(),
                candidates = endpoints.len(),
                "selected Tuist cache endpoint"
            );
            endpoint
        })
        .ok_or_else(|| {
            let message = if failures.is_empty() {
                "no reachable Tuist Kura cache endpoints".to_string()
            } else {
                format!(
                    "no reachable Tuist Kura cache endpoints: {}",
                    failures.join("; ")
                )
            };
            status::remote("pick fastest endpoint", ErrorKind::Unavailable, message)
        })
    }

    async fn grpc_channel(&self) -> Result<Channel> {
        Ok(self.grpc_channels().await?.requests)
    }

    async fn grpc_upload_channel(&self) -> Result<Channel> {
        Ok(self.grpc_channels().await?.uploads)
    }

    /// Connect once, then reuse the channels. A failed discovery or connection
    /// is retried after a cooldown instead of disabling the remote tier for
    /// the rest of the process.
    async fn grpc_channels(&self) -> Result<GrpcChannels> {
        let this = self.clone();
        self.grpc_channel_cache
            .get_or_try_init(move || async move {
                let url = this.data_plane_endpoint().await?;
                let endpoint = grpc_endpoint(&url).map_err(|message| {
                    status::remote("connect endpoint", ErrorKind::Misconfigured, message)
                })?;
                let requests = endpoint
                    .clone()
                    .timeout(GRPC_REQUEST_TIMEOUT)
                    .connect()
                    .await
                    .map_err(|_| {
                        status::remote(
                            "connect endpoint",
                            ErrorKind::Unavailable,
                            format!("could not connect to {}", redact::endpoint(&url)),
                        )
                    })?;
                Ok(GrpcChannels {
                    requests,
                    uploads: endpoint.connect_lazy(),
                })
            })
            .await
            .cloned()
    }

    /// Resolve authentication once per command. Tokens are proactively
    /// refreshed during this first lookup. A single command that outlives
    /// the resulting token's lifetime must be retried to resolve a new token.
    /// A missing or rejected credential is final; a failure to reach the
    /// token service is retried after a cooldown. Token requests are never
    /// retried immediately because a refresh can rotate the stored token.
    async fn auth_token(&self) -> Result<&str> {
        let auth = self.auth.clone();
        self.auth_token_cache
            .get_or_try_init(move || async move {
                match tokio::task::spawn_blocking(move || auth.token()).await {
                    Ok(result) => result,
                    Err(source) => Err(status::remote(
                        "load auth token",
                        ErrorKind::Other,
                        source.to_string(),
                    )),
                }
            })
            .await
            .map(String::as_str)
    }

    async fn authorized_request(
        &self,
        method: Method,
        url: Url,
        token: &str,
    ) -> Result<reqwest::RequestBuilder> {
        let client = self
            .client
            .get_or_try_init(|| async {
                reqwest::Client::builder()
                    .timeout(GRPC_REQUEST_TIMEOUT)
                    .build()
                    .map_err(|source| {
                        status::remote("build client", ErrorKind::Misconfigured, source.to_string())
                    })
            })
            .await?;
        Ok(client.request(method, url).bearer_auth(token))
    }

    async fn authorized_grpc_request<T>(
        &self,
        message: T,
        operation: &'static str,
    ) -> Result<Request<T>> {
        let token = self.auth_token().await?;
        authorized_grpc_request_with_token(message, token, Some(self.account()))
            .map_err(|message| status::remote(operation, ErrorKind::Misconfigured, message))
    }

    fn account(&self) -> &str {
        self.config
            .account
            .as_deref()
            .expect("validated at construction")
    }

    fn instance_name(&self) -> String {
        let Some(project) = self.config.project.as_deref() else {
            return "default".to_string();
        };
        project.to_string()
    }

    fn endpoints_url(&self) -> Result<Url> {
        let mut url = self.server_url(ENDPOINTS_PATH)?;
        url.query_pairs_mut()
            .append_pair("account_handle", self.account());
        Ok(url)
    }

    fn server_url(&self, path: &str) -> Result<Url> {
        join_url(&self.config.url, path)
    }
}

/// How long discovered endpoints are reused when the server sends no
/// `Cache-Control: max-age`, matching the Tuist CLI and Gradle plugin.
const DEFAULT_ENDPOINTS_MAX_AGE: Duration = Duration::from_hours(1);

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedEndpoints {
    endpoints: Vec<String>,
    expires_at_unix_ms: u64,
}

impl PersistedEndpoints {
    fn is_fresh(&self) -> bool {
        unix_ms_now() < self.expires_at_unix_ms
    }
}

fn unix_ms_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, duration_ms)
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Reads `max-age` from `Cache-Control`, ignoring the other directives.
fn cache_control_max_age(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::CACHE_CONTROL)?
        .to_str()
        .ok()?
        .split(',')
        .find_map(|directive| {
            let (name, value) = directive.trim().split_once('=')?;
            name.trim()
                .eq_ignore_ascii_case("max-age")
                .then(|| value.trim().parse::<u64>().ok())
                .flatten()
        })
        .map(Duration::from_secs)
}

#[derive(Debug, serde::Deserialize)]
struct EndpointResponse {
    endpoints: Vec<String>,
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

/// Failures are described without the transport's error text, which can
/// include the full endpoint URI.
async fn connect_grpc_endpoint(endpoint: &str) -> std::result::Result<Channel, String> {
    grpc_endpoint(endpoint)?
        .connect()
        .await
        .map_err(|_| "the connection failed".to_string())
}

fn grpc_endpoint(endpoint: &str) -> std::result::Result<Endpoint, String> {
    let endpoint_url = grpc_endpoint_url(endpoint);
    let invalid = || format!("invalid cache endpoint {}", redact::endpoint(endpoint));
    let mut endpoint = Endpoint::from_shared(endpoint_url.clone())
        .map_err(|_| invalid())?
        .connect_timeout(ENDPOINT_PROBE_TIMEOUT)
        .http2_adaptive_window(true);
    if endpoint_url.starts_with("https://") {
        endpoint = endpoint
            .tls_config(ClientTlsConfig::new().with_enabled_roots())
            .map_err(|_| invalid())?;
    }
    Ok(endpoint)
}

fn authorized_grpc_request_with_token<T>(
    message: T,
    token: &str,
    account: Option<&str>,
) -> std::result::Result<Request<T>, String> {
    let header = format!("Bearer {token}");
    let metadata = MetadataValue::try_from(header.as_str()).map_err(|source| source.to_string())?;
    let mut request = Request::new(message);
    request.metadata_mut().insert("authorization", metadata);
    if let Some(account) = account.and_then(non_empty_str) {
        let metadata = MetadataValue::try_from(account).map_err(|source| source.to_string())?;
        request
            .metadata_mut()
            .insert("x-tuist-account-handle", metadata);
    }
    Ok(request)
}

fn copy_blob_digests(
    reader: &mut dyn std::io::Read,
    writer: &mut dyn std::io::Write,
) -> std::io::Result<(Digest, reapi::Digest)> {
    let mut once = blake3::Hasher::new();
    let mut sha256 = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        once.update(&buffer[..read]);
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
        Digest::from_bytes(*once.finalize().as_bytes()),
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

pub(super) fn grpc_endpoint_url(endpoint: &str) -> String {
    if let Some(rest) = endpoint.strip_prefix("grpc://") {
        format!("http://{rest}")
    } else if let Some(rest) = endpoint.strip_prefix("grpcs://") {
        format!("https://{rest}")
    } else {
        endpoint.to_string()
    }
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
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn tuist_cache(temp: &TempDir, project: Option<&str>) -> TuistCache {
        TuistCache::new(
            Cas::open(temp.path().join("cas")),
            temp.path().join("auth"),
            TuistCacheConfig {
                url: "https://tuist.dev".to_string(),
                account: Some("tuist".to_string()),
                project: project.map(str::to_string),
                oauth_client_id: None,
                provider_name: "tuist".to_string(),
            },
        )
        .unwrap()
    }

    /// Answers each connection with the next response, counting requests.
    async fn serve_discovery(
        responses: Vec<String>,
    ) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = requests.clone();
        tokio::spawn(async move {
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut request = [0_u8; 4096];
                let _ = stream.read(&mut request).await.unwrap();
                stream.write_all(response.as_bytes()).await.unwrap();
                let _ = stream.shutdown().await;
            }
        });
        (format!("http://{address}"), requests)
    }

    fn endpoints_response(endpoint: &str, cache_control: &str) -> String {
        let body = format!(r#"{{"endpoints":["{endpoint}"]}}"#);
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nCache-Control: {cache_control}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn status_response(status: &str) -> String {
        format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    }

    /// A cache as a fresh Once process would open it on `temp`.
    fn discovery_cache(temp: &TempDir, url: &str) -> TuistCache {
        let mut cache = tuist_cache(temp, Some("once"));
        cache.config.url = url.to_string();
        cache.auth_token_cache.set("test-token".into());
        cache
    }

    #[tokio::test]
    async fn discovered_endpoints_are_reused_by_later_processes() {
        let temp = TempDir::new().unwrap();
        let (url, requests) = serve_discovery(vec![endpoints_response(
            "https://cache.example.test",
            "max-age=300",
        )])
        .await;

        let first = discovery_cache(&temp, &url)
            .fetch_endpoints()
            .await
            .unwrap();
        let second = discovery_cache(&temp, &url)
            .fetch_endpoints()
            .await
            .unwrap();

        assert_eq!(first, ["https://cache.example.test"]);
        assert_eq!(second, first);
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn expired_endpoints_are_discovered_again() {
        let temp = TempDir::new().unwrap();
        let (url, requests) = serve_discovery(vec![
            endpoints_response("https://old.example.test", "max-age=0"),
            endpoints_response("https://new.example.test", "max-age=300"),
        ])
        .await;

        discovery_cache(&temp, &url)
            .fetch_endpoints()
            .await
            .unwrap();
        let refreshed = discovery_cache(&temp, &url)
            .fetch_endpoints()
            .await
            .unwrap();

        assert_eq!(refreshed, ["https://new.example.test"]);
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn unreachable_discovery_falls_back_to_the_last_known_endpoints() {
        let temp = TempDir::new().unwrap();
        let (url, _) = serve_discovery(vec![
            endpoints_response("https://cache.example.test", "max-age=0"),
            status_response("503 Service Unavailable"),
        ])
        .await;

        discovery_cache(&temp, &url)
            .fetch_endpoints()
            .await
            .unwrap();
        let endpoints = discovery_cache(&temp, &url)
            .fetch_endpoints()
            .await
            .unwrap();

        assert_eq!(endpoints, ["https://cache.example.test"]);
    }

    #[tokio::test]
    async fn unreachable_discovery_without_a_known_endpoint_is_a_read_miss() {
        let temp = TempDir::new().unwrap();
        let (url, _) = serve_discovery(vec![status_response("503 Service Unavailable")]).await;

        let error = discovery_cache(&temp, &url)
            .fetch_endpoints()
            .await
            .unwrap_err();

        assert!(error.is_unavailable(), "{error}");
    }

    #[tokio::test]
    async fn interrupted_discovery_body_without_known_endpoints_is_a_read_miss() {
        let temp = TempDir::new().unwrap();
        let (url, _) = serve_discovery(vec![
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n".to_string(),
        ])
        .await;

        let error = discovery_cache(&temp, &url)
            .fetch_endpoints()
            .await
            .unwrap_err();

        assert!(error.is_unavailable(), "{error}");
    }

    #[tokio::test]
    async fn interrupted_discovery_body_reuses_stale_endpoints() {
        let temp = TempDir::new().unwrap();
        let (url, _) = serve_discovery(vec![
            endpoints_response("https://cache.example.test", "max-age=0"),
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n".to_string(),
        ])
        .await;

        discovery_cache(&temp, &url)
            .fetch_endpoints()
            .await
            .unwrap();
        let endpoints = discovery_cache(&temp, &url)
            .fetch_endpoints()
            .await
            .unwrap();

        assert_eq!(endpoints, ["https://cache.example.test"]);
    }

    #[tokio::test]
    async fn malformed_discovery_body_stays_fatal() {
        let temp = TempDir::new().unwrap();
        let (url, _) = serve_discovery(vec![
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]".to_string(),
        ])
        .await;

        let error = discovery_cache(&temp, &url)
            .fetch_endpoints()
            .await
            .unwrap_err();

        assert!(!error.is_unavailable(), "{error}");
    }

    #[tokio::test]
    async fn refused_discovery_stays_fatal() {
        let temp = TempDir::new().unwrap();
        let (url, _) = serve_discovery(vec![status_response("403 Forbidden")]).await;

        let error = discovery_cache(&temp, &url)
            .fetch_endpoints()
            .await
            .unwrap_err();

        assert!(!error.is_unavailable(), "{error}");
    }

    #[test]
    fn cache_control_max_age_ignores_other_directives() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CACHE_CONTROL,
            "private, MAX-AGE=42, must-revalidate".parse().unwrap(),
        );
        assert_eq!(
            cache_control_max_age(&headers),
            Some(Duration::from_secs(42))
        );
        assert_eq!(
            cache_control_max_age(&reqwest::header::HeaderMap::new()),
            None
        );
    }

    #[tokio::test]
    async fn data_plane_endpoint_prefers_the_override_over_discovery() {
        let temp = TempDir::new().unwrap();
        let mut cache = tuist_cache(&temp, Some("once"));
        cache.config.url = "http://127.0.0.1:9".to_string();
        cache.endpoint_override = Some("http://kura.kura.svc.cluster.local:4000".to_string());

        let endpoint = cache.data_plane_endpoint().await.unwrap();

        assert_eq!(endpoint, "http://kura.kura.svc.cluster.local:4000");
    }

    #[test]
    fn grpc_endpoint_url_normalizes_grpc_schemes() {
        assert_eq!(
            grpc_endpoint_url("grpc://localhost:5101"),
            "http://localhost:5101"
        );
        assert_eq!(
            grpc_endpoint_url("grpcs://cache.example.com"),
            "https://cache.example.com"
        );
        assert_eq!(
            grpc_endpoint_url("https://cache.example.com"),
            "https://cache.example.com"
        );
    }

    #[test]
    fn instance_name_uses_project_namespace_without_account_prefix() {
        let temp = TempDir::new().unwrap();
        let cache = tuist_cache(&temp, Some("gradle-plugin"));

        assert_eq!(cache.instance_name(), "gradle-plugin");
    }

    #[test]
    fn instance_name_defaults_to_default_namespace() {
        let temp = TempDir::new().unwrap();
        let cache = tuist_cache(&temp, None);

        assert_eq!(cache.instance_name(), "default");
    }

    #[tokio::test]
    async fn empty_blob_is_available_without_remote_storage() {
        let temp = TempDir::new().unwrap();
        let cache = tuist_cache(&temp, None);
        let digest = Digest::of_bytes(&[]);

        assert!(!cache.local().has_blob(&digest).await.unwrap());
        assert!(cache.has_blob(&digest).await.unwrap());
        assert!(cache.get_blob(&digest).await.unwrap().is_empty());
        assert!(cache.local().has_blob(&digest).await.unwrap());
    }

    #[tokio::test]
    async fn empty_blob_materializes_without_remote_storage() {
        let temp = TempDir::new().unwrap();
        let cache = tuist_cache(&temp, None);
        let digest = Digest::of_bytes(&[]);

        cache.ensure_blob_local(&digest).await.unwrap();

        assert!(cache.local().get_blob(&digest).await.unwrap().is_empty());
    }

    #[test]
    fn authorized_grpc_request_includes_tuist_account_header() {
        let request =
            authorized_grpc_request_with_token((), "token", Some("acme")).expect("valid metadata");

        assert_eq!(
            request
                .metadata()
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer token")
        );
        assert_eq!(
            request
                .metadata()
                .get("x-tuist-account-handle")
                .and_then(|value| value.to_str().ok()),
            Some("acme")
        );
    }

    #[test]
    fn sha256_digest_matches_stable_vectors() {
        let cases = [
            (
                b"".as_slice(),
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                0,
            ),
            (
                b"hello".as_slice(),
                "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
                5,
            ),
        ];

        for (bytes, hash, size_bytes) in cases {
            let digest = sha256_digest(bytes).unwrap();
            assert_eq!(digest.hash, hash);
            assert_eq!(digest.size_bytes, size_bytes);
        }
    }

    #[test]
    fn file_digests_match_in_memory_digests() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("blob");
        let bytes = b"streamed remote cache body";
        std::fs::write(&path, bytes).unwrap();

        let mut file = std::fs::File::open(&path).unwrap();
        let mut copied = Vec::new();
        let (once, remote) = copy_blob_digests(&mut file, &mut copied).unwrap();
        assert_eq!(copied, bytes);

        assert_eq!(once, Digest::of_bytes(bytes));
        assert_eq!(remote, sha256_digest(bytes).unwrap());
    }

    #[test]
    fn size_bytes_rejects_values_that_do_not_fit_reapi_digest() {
        if i64::try_from(usize::MAX).is_ok() {
            return;
        }

        let error = size_bytes_i64(usize::MAX).unwrap_err();
        assert!(error.to_string().contains("REAPI size limit"));
    }

    #[test]
    fn blob_and_action_mapping_digests_are_distinct() {
        let digest = Digest::of_bytes(b"payload");
        assert_ne!(
            blob_mapping_digest(&digest).unwrap(),
            action_mapping_digest(&digest).unwrap()
        );
    }

    #[test]
    fn byte_stream_resources_include_instance_digest_and_upload_id() {
        let digest = sha256_digest(b"payload").unwrap();
        let upload_id = Uuid::parse_str("018f22d2-6e6e-7e63-a6cc-06c3d9ca8f22").unwrap();

        assert_eq!(
            byte_stream_read_resource(
                "project",
                &digest,
                reapi::compressor::Value::Identity as i32
            ),
            format!("project/blobs/{}", digest_key(&digest))
        );
        assert_eq!(
            byte_stream_write_resource(
                "project",
                &digest,
                upload_id,
                reapi::compressor::Value::Identity as i32
            ),
            format!("project/uploads/{upload_id}/blobs/{}", digest_key(&digest))
        );
        assert_eq!(
            byte_stream_read_resource("", &digest, reapi::compressor::Value::Zstd as i32),
            format!("compressed-blobs/zstd/{}", digest_key(&digest))
        );
    }

    #[test]
    fn zstd_blob_transfer_roundtrips_and_preserves_digest() {
        let bytes = vec![b'a'; BATCH_BLOB_LIMIT + 1];
        let digest = sha256_digest(&bytes).unwrap();
        let (encoded, compressor) = encode_reapi_blob(&bytes, true, "test").unwrap();

        assert_eq!(compressor, reapi::compressor::Value::Zstd as i32);
        assert!(encoded.len() < bytes.len());
        assert_eq!(
            decode_reapi_blob(encoded, compressor, &digest, "test").unwrap(),
            bytes
        );
    }

    #[test]
    fn native_action_result_metadata_roundtrips_without_output_bodies() {
        let output = Digest::of_bytes(b"output");
        let stdout = Digest::of_bytes(b"stdout");
        let native = ActionResult {
            exit_code: 0,
            stdout: Some(stdout),
            stderr: None,
            outputs: BTreeMap::from([("out/result".to_string(), output)]),
        };
        let remote = reapi::ActionResult {
            execution_metadata: Some(reapi::ExecutedActionMetadata {
                auxiliary_metadata: vec![encode_action_result_metadata(&native).unwrap()],
                ..Default::default()
            }),
            ..Default::default()
        };

        assert_eq!(
            decode_action_result_metadata(&remote).unwrap(),
            Some(native)
        );
    }
}
