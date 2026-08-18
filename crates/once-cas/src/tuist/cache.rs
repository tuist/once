use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bazel_remote_apis::{
    build::bazel::remote::execution::v2::{
        self as reapi, action_cache_client::ActionCacheClient,
        capabilities_client::CapabilitiesClient,
        content_addressable_storage_client::ContentAddressableStorageClient,
    },
    google::bytestream::{self, byte_stream_client::ByteStreamClient},
    google::rpc::Status as RpcStatus,
};
use futures::{future::try_join_all, stream};
use reqwest::{Method, Url};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, OnceCell, Semaphore};
use tokio::time::Instant;
use tonic::metadata::MetadataValue;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tonic::{Code, Request, Status};
use uuid::Uuid;

use super::{
    join_url, remote_status_message, TuistAuth, TuistCacheConfig, ENDPOINTS_PATH, PROVIDER_NAME,
};
use crate::{ActionResult, Cas, Digest, Error, Result};

/// Ceiling on blobs transferred to or from the remote cache at once. The
/// number of outputs comes from a remote `ActionResult` (untrusted), and
/// each transfer buffers a whole blob, so an unbounded fan-out would let
/// a single response spike memory and open a connection per blob. This
/// caps the in-flight transfers regardless of how many the response
/// declares.
const MAX_CONCURRENT_BLOB_TRANSFERS: usize = 16;

const ENDPOINT_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const GRPC_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const BATCH_BLOB_LIMIT: usize = 2 * 1024 * 1024;
const BATCH_BLOB_LIMIT_I64: i64 = 2 * 1024 * 1024;
const BYTE_STREAM_CHUNK_SIZE: usize = 2 * 1024 * 1024;
const BYTE_STREAM_MESSAGE_LIMIT: usize = 32 * 1024 * 1024;
const MAX_PARALLEL_TRANSFERS: usize = 8;
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
    client: Arc<OnceCell<std::result::Result<reqwest::Client, CachedRemoteError>>>,
    config: TuistCacheConfig,
    grpc_channel_cache: Arc<OnceCell<std::result::Result<Channel, CachedRemoteError>>>,
    capabilities_cache: Arc<Mutex<Option<RemoteCapabilities>>>,
    known_remote_blobs: Arc<Mutex<HashMap<Digest, reapi::Digest>>>,
    transfer_limit: Arc<Semaphore>,
    auth: TuistAuth,
    auth_token_cache: Arc<OnceCell<std::result::Result<String, CachedRemoteError>>>,
}

#[derive(Debug, Clone, Copy, Default)]
struct RemoteCapabilities {
    zstd_streams: bool,
    zstd_batches: bool,
}

#[derive(Debug, Clone)]
struct CachedRemoteError {
    operation: &'static str,
    message: String,
}

impl CachedRemoteError {
    fn from_error(error: Error, operation: &'static str) -> Self {
        match error {
            Error::Remote {
                operation, message, ..
            } => Self { operation, message },
            error => Self {
                operation,
                message: error.to_string(),
            },
        }
    }

    fn to_error(&self) -> Error {
        Error::Remote {
            provider: PROVIDER_NAME,
            operation: self.operation,
            message: self.message.clone(),
        }
    }
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
            client: Arc::new(OnceCell::new()),
            config,
            grpc_channel_cache: Arc::new(OnceCell::new()),
            capabilities_cache: Arc::new(Mutex::new(None)),
            known_remote_blobs: Arc::new(Mutex::new(HashMap::new())),
            transfer_limit: Arc::new(Semaphore::new(MAX_PARALLEL_TRANSFERS)),
            auth,
            auth_token_cache: Arc::new(OnceCell::new()),
        })
    }

    pub fn local(&self) -> &Cas {
        &self.local
    }

    pub fn config(&self) -> &TuistCacheConfig {
        &self.config
    }

    pub async fn get_blob(&self, digest: &Digest) -> Result<Vec<u8>> {
        match self.local.get_blob(digest).await {
            Ok(bytes) => return Ok(bytes),
            Err(Error::BlobNotFound(_)) => {}
            Err(error) => return Err(error),
        }

        let bytes = self.get_blob_remote(digest).await?;
        let mirrored = self.local.mirror_blob(digest, &bytes).await?;
        if mirrored != *digest {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "get blob",
                message: format!("remote blob {digest} did not match requested digest"),
            });
        }
        Ok(bytes)
    }

    pub async fn ensure_blob_local(&self, digest: &Digest) -> Result<()> {
        if self.local.has_blob(digest).await? {
            return Ok(());
        }
        let remote_digest = self.remote_blob_digest(digest).await?;
        let _permit = self
            .transfer_limit
            .acquire()
            .await
            .expect("transfer semaphore remains open");
        let channel = self.grpc_channel().await?;
        let mut client =
            ByteStreamClient::new(channel).max_decoding_message_size(BYTE_STREAM_MESSAGE_LIMIT);
        let request = bytestream::ReadRequest {
            resource_name: byte_stream_read_resource(
                &self.instance_name(),
                &remote_digest,
                reapi::compressor::Value::Identity as i32,
            ),
            read_offset: 0,
            read_limit: 0,
        };
        let response = match client
            .read(self.authorized_grpc_request(request, "get blob").await?)
            .await
        {
            Ok(response) => response,
            Err(status) if status.code() == Code::NotFound => {
                return Err(Error::BlobNotFound(*digest));
            }
            Err(status) => return Err(grpc_error("get blob", &status)),
        };
        let mut stream = response.into_inner();
        let (mut writer, reader) = tokio::io::duplex(64 * 1024);
        let download = async {
            while let Some(response) = stream
                .message()
                .await
                .map_err(|source| grpc_error("get blob", &source))?
            {
                writer
                    .write_all(&response.data)
                    .await
                    .map_err(|source| Error::Remote {
                        provider: PROVIDER_NAME,
                        operation: "get blob",
                        message: source.to_string(),
                    })?;
            }
            Ok::<_, Error>(())
        };
        let (mirrored, ()) = tokio::try_join!(self.local.put_stream(reader), download)?;
        if mirrored != *digest {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "get blob",
                message: format!("remote blob {digest} did not match requested digest"),
            });
        }
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

    /// True if the blob is present locally or remotely. A remote
    /// failure is treated as "not present" so a transient outage degrades
    /// to a cache miss instead of a hard error.
    pub async fn has_blob(&self, digest: &Digest) -> Result<bool> {
        if self.has_local_blob(digest).await? {
            return Ok(true);
        }
        Ok(self.head_blob_remote(digest).await.unwrap_or(false))
    }

    /// Whether the local tier already holds the blob, without asking the
    /// remote one. For callers that only want to know whether something is
    /// available at no cost.
    pub async fn has_local_blob(&self, digest: &Digest) -> Result<bool> {
        self.local.has_blob(digest).await
    }

    pub async fn get_action_result(&self, action: &Digest) -> Result<Option<ActionResult>> {
        if let Some(result) = self.local.get_action_result(action).await? {
            tracing::debug!(action_digest = %action, tier = "local", "action cache hit");
            return Ok(Some(result));
        }

        match self.get_action_result_remote(action).await {
            Ok(Some(result)) => {
                tracing::debug!(action_digest = %action, tier = "remote", "action cache hit");
                if let Err(error) = self.local.mirror_action_result(action, &result).await {
                    tracing::warn!(
                        action_digest = %action,
                        error = %error,
                        "failed to persist remote action result locally"
                    );
                }
                Ok(Some(result))
            }
            Ok(None) => {
                tracing::debug!(action_digest = %action, "action cache miss");
                Ok(None)
            }
            Err(error) if error.is_read_miss() => {
                tracing::debug!(
                    action_digest = %action,
                    error = ?error,
                    "action cache miss"
                );
                Ok(None)
            }
            Err(error) => Err(error.into_public_error("get action result")),
        }
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

    async fn get_blob_remote(&self, digest: &Digest) -> Result<Vec<u8>> {
        let sha256 = self.remote_blob_digest(digest).await?;
        let Some(bytes) = self.read_reapi_blob(&sha256, "get blob").await? else {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "get blob",
                message: format!("remote blob mapping for {digest} points at a missing blob"),
            });
        };
        tracing::debug!(blob_digest = %digest, bytes = bytes.len(), "fetched blob from remote cache");
        Ok(bytes)
    }

    async fn put_blob_remote(&self, digest: &Digest, bytes: &[u8]) -> Result<reapi::Digest> {
        if Digest::of_bytes(bytes) != *digest {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "put blob",
                message: format!("blob body did not match digest {digest}"),
            });
        }
        if let Some(remote_digest) = self.known_remote_blobs.lock().await.get(digest).cloned() {
            return Ok(remote_digest);
        }
        let sha256 = sha256_digest(bytes)?;
        if self.reapi_blob_exists(&sha256).await? {
            tracing::debug!(blob_digest = %digest, bytes = bytes.len(), "remote blob already present");
        } else {
            self.upload_reapi_blob(&sha256, bytes, "put blob").await?;
            tracing::debug!(blob_digest = %digest, bytes = bytes.len(), "uploaded blob to remote cache");
        }
        self.put_blob_mapping(digest, &sha256).await?;
        self.known_remote_blobs
            .lock()
            .await
            .insert(*digest, sha256.clone());
        Ok(sha256)
    }

    async fn get_action_result_remote(
        &self,
        action: &Digest,
    ) -> std::result::Result<Option<ActionResult>, RemoteReadError> {
        let mapping_digest = action_mapping_digest(action).map_err(RemoteReadError::fatal)?;
        let Some(result) = self
            .get_reapi_action_result(&mapping_digest, "get action result")
            .await
            .map_err(remote_action_read_error)?
        else {
            return Ok(None);
        };

        self.reapi_to_once_action_result(result)
            .await
            .map(Some)
            .map_err(remote_action_read_error)
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
        if let Some(remote_digest) = self.known_remote_blobs.lock().await.get(digest).cloned() {
            return Ok(Some(remote_digest));
        }

        let directory = tempfile::tempdir().map_err(|source| Error::Io {
            path: std::env::temp_dir(),
            source,
        })?;
        let path = directory.path().join("blob");
        self.local.copy_blob_to_file(digest, &path).await?;
        let digest_to_verify = *digest;
        let hash_path = path.clone();
        let (once_digest, remote_digest) =
            tokio::task::spawn_blocking(move || file_digests(&hash_path))
                .await
                .map_err(|source| Error::Io {
                    path: path.clone(),
                    source: std::io::Error::other(source.to_string()),
                })??;
        if once_digest != digest_to_verify {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation,
                message: format!("local blob body did not match digest {digest}"),
            });
        }

        if !self.reapi_blob_exists(&remote_digest).await? {
            let _permit = self
                .transfer_limit
                .acquire()
                .await
                .expect("transfer semaphore remains open");
            self.write_reapi_blob_file_stream(&remote_digest, &path, operation)
                .await?;
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
            .ok_or_else(|| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "get blob mapping",
                message: format!("remote blob mapping for {digest} is missing its REAPI digest"),
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
        self.ensure_reapi_blobs_present(remote_digests).await?;
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
        let Some(bytes) = self.read_reapi_blob(digest, operation).await? else {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation,
                message: format!(
                    "remote action result points at missing blob {}",
                    digest_key(digest)
                ),
            });
        };
        let local_digest = Digest::of_bytes(&bytes);
        self.local.mirror_blob(&local_digest, &bytes).await?;
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
        let mut client = ContentAddressableStorageClient::new(channel);
        let request = reapi::BatchUpdateBlobsRequest {
            instance_name: self.instance_name(),
            requests: vec![reapi::batch_update_blobs_request::Request {
                digest: Some(digest.clone()),
                data,
                compressor,
            }],
            digest_function: SHA256_DIGEST_FUNCTION,
        };
        let response = client
            .batch_update_blobs(self.authorized_grpc_request(request, operation).await?)
            .await
            .map_err(|source| grpc_error(operation, &source))?
            .into_inner();
        let Some(status) = response
            .responses
            .first()
            .and_then(|response| response.status.as_ref())
        else {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation,
                message: "Kura returned no status for blob upload".to_string(),
            });
        };
        if status.code == REAPI_STATUS_OK {
            Ok(())
        } else {
            Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation,
                message: rpc_status_message(status),
            })
        }
    }

    async fn read_reapi_blob(
        &self,
        digest: &reapi::Digest,
        operation: &'static str,
    ) -> Result<Option<Vec<u8>>> {
        let _permit = self
            .transfer_limit
            .acquire()
            .await
            .expect("transfer semaphore remains open");
        if digest.size_bytes > BATCH_BLOB_LIMIT_I64 {
            return self.read_reapi_blob_stream(digest, operation).await;
        }
        let capabilities = self.remote_capabilities().await;
        let channel = self.grpc_channel().await?;
        let mut client = ContentAddressableStorageClient::new(channel);
        let mut acceptable_compressors = vec![reapi::compressor::Value::Identity as i32];
        if capabilities.zstd_batches {
            acceptable_compressors.push(reapi::compressor::Value::Zstd as i32);
        }
        let request = reapi::BatchReadBlobsRequest {
            instance_name: self.instance_name(),
            digests: vec![digest.clone()],
            acceptable_compressors,
            digest_function: SHA256_DIGEST_FUNCTION,
        };
        let response = client
            .batch_read_blobs(self.authorized_grpc_request(request, operation).await?)
            .await
            .map_err(|source| grpc_error(operation, &source))?
            .into_inner();
        let Some(blob) = response.responses.into_iter().next() else {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation,
                message: "Kura returned no blob response".to_string(),
            });
        };
        match blob
            .status
            .as_ref()
            .map_or(REAPI_STATUS_OK, |status| status.code)
        {
            REAPI_STATUS_OK => {
                let bytes = decode_reapi_blob(blob.data, blob.compressor, digest, operation)?;
                Ok(Some(bytes))
            }
            REAPI_STATUS_NOT_FOUND => Ok(None),
            _ => Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation,
                message: rpc_status_message(
                    blob.status
                        .as_ref()
                        .expect("non-successful response has a status"),
                ),
            }),
        }
    }

    async fn write_reapi_blob_stream(
        &self,
        digest: &reapi::Digest,
        bytes: &[u8],
        operation: &'static str,
    ) -> Result<()> {
        let capabilities = self.remote_capabilities().await;
        let (body, compressor) = encode_reapi_blob(bytes, capabilities.zstd_streams, operation)?;
        let channel = self.grpc_channel().await?;
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
        let response = client
            .write(self.authorized_grpc_request(requests, operation).await?)
            .await
            .map_err(|source| grpc_error(operation, &source))?
            .into_inner();
        if ![
            -1,
            digest.size_bytes,
            i64::try_from(total).unwrap_or(i64::MAX),
        ]
        .contains(&response.committed_size)
        {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation,
                message: format!(
                    "byte stream committed {} of {total} bytes",
                    response.committed_size
                ),
            });
        }
        Ok(())
    }

    async fn write_reapi_blob_file_stream(
        &self,
        digest: &reapi::Digest,
        path: &Path,
        operation: &'static str,
    ) -> Result<()> {
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|source| Error::Io {
                path: path.to_path_buf(),
                source,
            })?;
        let channel = self.grpc_channel().await?;
        let mut client = ByteStreamClient::new(channel);
        let resource_name = byte_stream_write_resource(
            &self.instance_name(),
            digest,
            Uuid::now_v7(),
            reapi::compressor::Value::Identity as i32,
        );
        let total = digest.size_bytes;
        let read_error = Arc::new(Mutex::new(None::<String>));
        let request_error = Arc::clone(&read_error);
        let requests = stream::unfold(
            (file, resource_name, 0_i64, false, request_error),
            move |(mut file, resource_name, offset, finished, read_error)| async move {
                if finished {
                    return None;
                }
                let mut data = vec![0_u8; BYTE_STREAM_CHUNK_SIZE];
                let read = match file.read(&mut data).await {
                    Ok(read) => read,
                    Err(error) => {
                        *read_error.lock().await = Some(error.to_string());
                        return None;
                    }
                };
                if read == 0 && offset < total {
                    *read_error.lock().await =
                        Some("staged blob ended before its declared size".to_string());
                    return None;
                }
                data.truncate(read);
                let end = offset.saturating_add(i64::try_from(read).unwrap_or(i64::MAX));
                let finish_write = end == total;
                let request = bytestream::WriteRequest {
                    resource_name: if offset == 0 {
                        resource_name.clone()
                    } else {
                        String::new()
                    },
                    write_offset: offset,
                    finish_write,
                    data,
                };
                Some((
                    request,
                    (
                        file,
                        resource_name,
                        end,
                        finish_write,
                        Arc::clone(&read_error),
                    ),
                ))
            },
        );
        let response = client
            .write(self.authorized_grpc_request(requests, operation).await?)
            .await;
        if let Some(message) = read_error.lock().await.take() {
            return Err(Error::Io {
                path: path.to_path_buf(),
                source: std::io::Error::other(message),
            });
        }
        let response = response
            .map_err(|source| grpc_error(operation, &source))?
            .into_inner();
        if ![-1, total].contains(&response.committed_size) {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation,
                message: format!(
                    "byte stream committed {} of {total} bytes",
                    response.committed_size
                ),
            });
        }
        Ok(())
    }

    async fn read_reapi_blob_stream(
        &self,
        digest: &reapi::Digest,
        operation: &'static str,
    ) -> Result<Option<Vec<u8>>> {
        let capabilities = self.remote_capabilities().await;
        let compressor = if capabilities.zstd_streams {
            reapi::compressor::Value::Zstd as i32
        } else {
            reapi::compressor::Value::Identity as i32
        };
        let channel = self.grpc_channel().await?;
        let mut client =
            ByteStreamClient::new(channel).max_decoding_message_size(BYTE_STREAM_MESSAGE_LIMIT);
        let request = bytestream::ReadRequest {
            resource_name: byte_stream_read_resource(&self.instance_name(), digest, compressor),
            read_offset: 0,
            read_limit: 0,
        };
        let response = match client
            .read(self.authorized_grpc_request(request, operation).await?)
            .await
        {
            Ok(response) => response,
            Err(status) if status.code() == Code::NotFound => return Ok(None),
            Err(status) => return Err(grpc_error(operation, &status)),
        };
        let expected = usize::try_from(digest.size_bytes).unwrap_or(0);
        let initial_capacity = if compressor == reapi::compressor::Value::Identity as i32 {
            expected.min(BYTE_STREAM_MESSAGE_LIMIT)
        } else {
            expected.min(BYTE_STREAM_CHUNK_SIZE)
        };
        let mut transferred = Vec::with_capacity(initial_capacity);
        let mut stream = response.into_inner();
        while let Some(response) = stream
            .message()
            .await
            .map_err(|source| grpc_error(operation, &source))?
        {
            transferred.extend_from_slice(&response.data);
        }
        let bytes = decode_reapi_blob(transferred, compressor, digest, operation)?;
        Ok(Some(bytes))
    }

    async fn remote_capabilities(&self) -> RemoteCapabilities {
        let mut cached = self.capabilities_cache.lock().await;
        if let Some(capabilities) = *cached {
            return capabilities;
        }
        let capabilities = match self.fetch_remote_capabilities().await {
            Ok(capabilities) => capabilities,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "failed to discover remote cache capabilities"
                );
                RemoteCapabilities::default()
            }
        };
        *cached = Some(capabilities);
        capabilities
    }

    async fn fetch_remote_capabilities(&self) -> Result<RemoteCapabilities> {
        let channel = self.grpc_channel().await?;
        let mut client = CapabilitiesClient::new(channel);
        let request = reapi::GetCapabilitiesRequest {
            instance_name: self.instance_name(),
        };
        let response = client
            .get_capabilities(
                self.authorized_grpc_request(request, "discover capabilities")
                    .await?,
            )
            .await
            .map_err(|source| grpc_error("discover capabilities", &source))?
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
        })
    }

    async fn reapi_blob_exists(&self, digest: &reapi::Digest) -> Result<bool> {
        let channel = self.grpc_channel().await?;
        let mut client = ContentAddressableStorageClient::new(channel);
        let request = reapi::FindMissingBlobsRequest {
            instance_name: self.instance_name(),
            blob_digests: vec![digest.clone()],
            digest_function: SHA256_DIGEST_FUNCTION,
        };
        let response = client
            .find_missing_blobs(self.authorized_grpc_request(request, "head blob").await?)
            .await
            .map_err(|source| grpc_error("head blob", &source))?
            .into_inner();
        Ok(response.missing_blob_digests.is_empty())
    }

    async fn ensure_reapi_blobs_present(&self, digests: Vec<reapi::Digest>) -> Result<()> {
        if digests.is_empty() {
            return Ok(());
        }
        let channel = self.grpc_channel().await?;
        let mut client = ContentAddressableStorageClient::new(channel);
        let request = reapi::FindMissingBlobsRequest {
            instance_name: self.instance_name(),
            blob_digests: digests,
            digest_function: SHA256_DIGEST_FUNCTION,
        };
        let response = client
            .find_missing_blobs(
                self.authorized_grpc_request(request, "get action result")
                    .await?,
            )
            .await
            .map_err(|source| grpc_error("get action result", &source))?
            .into_inner();
        if let Some(missing) = response.missing_blob_digests.first() {
            return Err(remote_action_metadata_error(&format!(
                "remote action result points at {} missing blob(s), including {}",
                response.missing_blob_digests.len(),
                digest_key(missing)
            )));
        }
        Ok(())
    }

    async fn get_reapi_action_result(
        &self,
        action_digest: &reapi::Digest,
        operation: &'static str,
    ) -> Result<Option<reapi::ActionResult>> {
        let channel = self.grpc_channel().await?;
        let mut client = ActionCacheClient::new(channel);
        let request = reapi::GetActionResultRequest {
            instance_name: self.instance_name(),
            action_digest: Some(action_digest.clone()),
            inline_stdout: false,
            inline_stderr: false,
            inline_output_files: Vec::new(),
            digest_function: SHA256_DIGEST_FUNCTION,
        };
        match client
            .get_action_result(self.authorized_grpc_request(request, operation).await?)
            .await
        {
            Ok(response) => Ok(Some(response.into_inner())),
            Err(status) if status.code() == Code::NotFound => Ok(None),
            Err(status) => Err(grpc_error(operation, &status)),
        }
    }

    async fn update_reapi_action_result(
        &self,
        action_digest: &reapi::Digest,
        action_result: reapi::ActionResult,
        operation: &'static str,
    ) -> Result<()> {
        let channel = self.grpc_channel().await?;
        let mut client = ActionCacheClient::new(channel);
        let request = reapi::UpdateActionResultRequest {
            instance_name: self.instance_name(),
            action_digest: Some(action_digest.clone()),
            action_result: Some(action_result),
            results_cache_policy: None,
            digest_function: SHA256_DIGEST_FUNCTION,
        };
        client
            .update_action_result(self.authorized_grpc_request(request, operation).await?)
            .await
            .map_err(|source| grpc_error(operation, &source))?;
        Ok(())
    }

    async fn data_plane_endpoint(&self) -> Result<String> {
        let endpoints = self.fetch_endpoints().await?;
        match endpoints.as_slice() {
            [] => Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "discover endpoints",
                message: "Tuist returned no Kura cache endpoints".to_string(),
            }),
            [endpoint] => Ok(endpoint.clone()),
            _ => self.pick_fastest_endpoint(&endpoints).await,
        }
    }

    async fn fetch_endpoints(&self) -> Result<Vec<String>> {
        let url = self.endpoints_url()?;
        let token = self.auth_token().await?;
        let response = self
            .authorized_request(Method::GET, url, token)
            .await?
            .header(KURA_FEATURE_FLAGS_HEADER, KURA_FEATURE_FLAG)
            .send()
            .await
            .map_err(|source| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "discover endpoints",
                message: source.to_string(),
            })?;
        match response.status() {
            status if status.is_success() => {
                let endpoints: EndpointResponse =
                    response.json().await.map_err(|source| Error::Remote {
                        provider: PROVIDER_NAME,
                        operation: "decode endpoints",
                        message: source.to_string(),
                    })?;
                Ok(endpoints.endpoints)
            }
            _ => Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "discover endpoints",
                message: remote_status_message(response).await,
            }),
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
                    Err(error) => Err(format!("{endpoint}: {error}")),
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
                endpoint = %endpoint,
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
            Error::Remote {
                provider: PROVIDER_NAME,
                operation: "pick fastest endpoint",
                message,
            }
        })
    }

    async fn grpc_channel(&self) -> Result<Channel> {
        let result =
            self.grpc_channel_cache
                .get_or_init(|| async {
                    let endpoint = self.data_plane_endpoint().await.map_err(|error| {
                        CachedRemoteError::from_error(error, "discover endpoints")
                    })?;
                    connect_grpc_endpoint(&endpoint)
                        .await
                        .map_err(|message| CachedRemoteError {
                            operation: "connect endpoint",
                            message,
                        })
                })
                .await;
        match result {
            Ok(channel) => Ok(channel.clone()),
            Err(error) => Err(error.to_error()),
        }
    }

    /// Resolve authentication once per command. Tokens are proactively
    /// refreshed during this first lookup. A single command that outlives
    /// the resulting token's lifetime must be retried to resolve a new token.
    async fn auth_token(&self) -> Result<&str> {
        let result = self
            .auth_token_cache
            .get_or_init(|| async {
                let auth = self.auth.clone();
                match tokio::task::spawn_blocking(move || auth.token()).await {
                    Ok(result) => result
                        .map_err(|error| CachedRemoteError::from_error(error, "load auth token")),
                    Err(source) => Err(CachedRemoteError {
                        operation: "load auth token",
                        message: source.to_string(),
                    }),
                }
            })
            .await;
        match result {
            Ok(token) => Ok(token),
            Err(error) => Err(error.to_error()),
        }
    }

    async fn authorized_request(
        &self,
        method: Method,
        url: Url,
        token: &str,
    ) -> Result<reqwest::RequestBuilder> {
        let client = self
            .client
            .get_or_init(|| async {
                reqwest::Client::builder()
                    .timeout(GRPC_REQUEST_TIMEOUT)
                    .build()
                    .map_err(|source| CachedRemoteError {
                        operation: "build client",
                        message: source.to_string(),
                    })
            })
            .await;
        match client {
            Ok(client) => Ok(client.request(method, url).bearer_auth(token)),
            Err(error) => Err(error.to_error()),
        }
    }

    async fn authorized_grpc_request<T>(
        &self,
        message: T,
        operation: &'static str,
    ) -> Result<Request<T>> {
        let token = self.auth_token().await?;
        authorized_grpc_request_with_token(message, token, Some(self.account())).map_err(
            |message| Error::Remote {
                provider: PROVIDER_NAME,
                operation,
                message,
            },
        )
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

#[derive(Debug, serde::Deserialize)]
struct EndpointResponse {
    endpoints: Vec<String>,
}

#[derive(Debug)]
enum RemoteReadError {
    Miss(String),
    Fatal(Error),
}

impl RemoteReadError {
    fn miss(message: String) -> Self {
        Self::Miss(message)
    }

    fn fatal(error: Error) -> Self {
        Self::Fatal(error)
    }

    fn is_read_miss(&self) -> bool {
        matches!(self, Self::Miss(_))
    }

    fn into_public_error(self, operation: &'static str) -> Error {
        match self {
            Self::Miss(message) => Error::Remote {
                provider: PROVIDER_NAME,
                operation,
                message,
            },
            Self::Fatal(error) => error,
        }
    }
}

fn remote_action_read_error(error: Error) -> RemoteReadError {
    match error {
        Error::Remote {
            message,
            operation,
            provider,
        } if provider == PROVIDER_NAME
            && matches!(
                operation,
                "connect endpoint" | "get action result" | "pick fastest endpoint"
            ) =>
        {
            RemoteReadError::miss(message)
        }
        other => RemoteReadError::fatal(other),
    }
}

fn encode_action_result_metadata(
    result: &ActionResult,
) -> Result<bazel_remote_apis::google::protobuf::Any> {
    let value = serde_json::to_vec(result).map_err(|source| Error::Remote {
        provider: PROVIDER_NAME,
        operation: "put action result",
        message: format!("serializing native action result metadata: {source}"),
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

fn remote_action_metadata_error(message: &str) -> Error {
    Error::Remote {
        provider: PROVIDER_NAME,
        operation: "get action result",
        message: message.to_string(),
    }
}

async fn connect_grpc_endpoint(endpoint: &str) -> std::result::Result<Channel, String> {
    let endpoint_url = grpc_endpoint_url(endpoint);
    let mut endpoint = Endpoint::from_shared(endpoint_url.clone())
        .map_err(|source| source.to_string())?
        .connect_timeout(ENDPOINT_PROBE_TIMEOUT)
        .timeout(GRPC_REQUEST_TIMEOUT);
    if endpoint_url.starts_with("https://") {
        endpoint = endpoint
            .tls_config(ClientTlsConfig::new().with_enabled_roots())
            .map_err(|source| format!("{source:?}"))?;
    }
    endpoint
        .connect()
        .await
        .map_err(|source| format!("{source:?}"))
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

fn grpc_error(operation: &'static str, status: &Status) -> Error {
    Error::Remote {
        provider: PROVIDER_NAME,
        operation,
        message: grpc_status_message(status),
    }
}

fn grpc_status_message(status: &Status) -> String {
    if status.message().is_empty() {
        format!("gRPC {}", status.code())
    } else {
        format!("gRPC {}: {}", status.code(), status.message())
    }
}

fn rpc_status_message(status: &RpcStatus) -> String {
    if status.message.is_empty() {
        format!("REAPI status {}", status.code)
    } else {
        format!("REAPI status {}: {}", status.code, status.message)
    }
}

fn file_digests(path: &Path) -> Result<(Digest, reapi::Digest)> {
    let mut file = std::fs::File::open(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut once = blake3::Hasher::new();
    let mut sha256 = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if read == 0 {
            break;
        }
        once.update(&buffer[..read]);
        sha256.update(&buffer[..read]);
        size = size.saturating_add(read as u64);
    }
    let size_bytes = i64::try_from(size).map_err(|_| Error::Remote {
        provider: PROVIDER_NAME,
        operation: "put blob",
        message: "blob exceeds the remote size range".to_string(),
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
        Err(Error::Remote {
            provider: PROVIDER_NAME,
            operation,
            message: format!(
                "remote blob {} did not match expected digest {}",
                digest_key(&actual),
                digest_key(digest)
            ),
        })
    }
}

fn encode_reapi_blob(
    bytes: &[u8],
    supports_zstd: bool,
    operation: &'static str,
) -> Result<(Vec<u8>, i32)> {
    if supports_zstd {
        let compressed = zstd::stream::encode_all(bytes, 0).map_err(|source| Error::Remote {
            provider: PROVIDER_NAME,
            operation,
            message: format!("compressing blob: {source}"),
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
            let expected = usize::try_from(digest.size_bytes).map_err(|_| Error::Remote {
                provider: PROVIDER_NAME,
                operation,
                message: format!(
                    "remote blob has invalid size in digest {}",
                    digest_key(digest)
                ),
            })?;
            let limit = u64::try_from(expected)
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            let decompressor =
                zstd::stream::read::Decoder::new(data.as_slice()).map_err(|source| {
                    Error::Remote {
                        provider: PROVIDER_NAME,
                        operation,
                        message: format!("opening compressed blob: {source}"),
                    }
                })?;
            let mut uncompressed = Vec::with_capacity(expected);
            decompressor
                .take(limit)
                .read_to_end(&mut uncompressed)
                .map_err(|source| Error::Remote {
                    provider: PROVIDER_NAME,
                    operation,
                    message: format!("decompressing blob: {source}"),
                })?;
            uncompressed
        }
        _ => {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation,
                message: format!("unsupported remote blob compressor {compressor}"),
            });
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
        return Err(Error::Remote {
            provider: PROVIDER_NAME,
            operation: "compute digest",
            message: format!("blob size exceeds REAPI size limit: {size} bytes"),
        });
    };
    Ok(size)
}

fn grpc_endpoint_url(endpoint: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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

        let (once, remote) = file_digests(&path).unwrap();

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
    fn grpc_error_includes_code_and_message() {
        let error = grpc_error("get blob", &Status::unavailable("cache unavailable"));
        let message = error.to_string();
        assert!(message.to_lowercase().contains("unavailable"));
        assert!(message.contains("cache unavailable"));
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

    #[test]
    fn remote_action_materialization_errors_are_read_misses() {
        let error = Error::Remote {
            provider: PROVIDER_NAME,
            operation: "get action result",
            message: "remote action result points at missing blob".to_string(),
        };

        assert!(remote_action_read_error(error).is_read_miss());
    }

    #[test]
    fn remote_endpoint_connectivity_errors_are_read_misses() {
        for operation in ["connect endpoint", "pick fastest endpoint"] {
            let error = Error::Remote {
                provider: PROVIDER_NAME,
                operation,
                message: "cache endpoint unavailable".to_string(),
            };

            assert!(remote_action_read_error(error).is_read_miss());
        }
    }

    #[test]
    fn unrelated_remote_action_errors_stay_fatal() {
        for operation in ["discover endpoints", "put blob"] {
            let error = Error::Remote {
                provider: PROVIDER_NAME,
                operation,
                message: "remote operation failed".to_string(),
            };

            assert!(!remote_action_read_error(error).is_read_miss());
        }
    }
}
