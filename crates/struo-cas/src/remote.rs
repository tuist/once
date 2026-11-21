//! A local store fronting any keyed remote blob store.
//!
//! [`RemoteStore`] is the transport contract: get, head, and put opaque
//! objects by key. [`RemoteCache`] owns everything else, so every backend
//! gets the same local-first reads, integrity verification, transfer
//! budgets, and best-effort publication.

use std::fmt;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::Semaphore;

use crate::transfer::{self, TransferDirection};
use crate::tuist::Flights;
use crate::{ActionCacheHit, ActionResult, Cas, Digest, Error, ErrorKind, Result, TransferBudget};

mod aws_credentials;
mod http;
mod object_storage;

pub use self::http::{HttpCacheAuth, HttpCacheRemote};
pub use self::object_storage::ObjectStoreRemote;

/// Largest action record a remote may return. Records only list digests, so
/// anything larger is not a record this client wrote.
const MAX_ACTION_RECORD_BYTES: u64 = 16 * 1024 * 1024;
/// Memory a streamed download holds: transport chunks, the ingest chunk, and
/// the pipe to the encoder.
const DOWNLOAD_MEMORY_BYTES: u64 = crate::MIN_TRANSFER_MEMORY_BYTES;
const DEFAULT_MAX_CONCURRENT_TRANSFERS: usize = 8;

/// A streamed object body and the size the store declared for it.
pub struct RemoteBody {
    /// Bytes the store says the body holds. Ingest rejects a body of any
    /// other length.
    pub size: u64,
    /// The body. Dropping it cancels the transfer.
    pub reader: Pin<Box<dyn AsyncRead + Send>>,
}

impl fmt::Debug for RemoteBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteBody")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// A remote store that holds opaque objects by key.
///
/// Keys are `cas/<blake3-hex>` for content and `ac/<action-hex>` for JSON
/// action records; a store maps them into its own versioned namespace.
///
/// Only `Ok(None)` from [`head`](Self::head) or [`get`](Self::get) means
/// the object is absent. Report everything else as an [`Error`] classified
/// with [`ErrorKind`]: rejected credentials or denied access as
/// [`ErrorKind::Refused`], outages and throttling as
/// [`ErrorKind::Unavailable`], and unusable configuration as
/// [`ErrorKind::Misconfigured`].
///
/// A put must never make a partial object readable. A put to a key that
/// already exists, or that another writer is publishing, may keep the other
/// value and still return `Ok(())`: content keys are immutable, and a reader
/// treats an object that is not readable yet as a miss.
// async_trait marks its boxed futures `#[must_use]` again.
#[allow(clippy::double_must_use)]
#[async_trait::async_trait]
pub trait RemoteStore: Send + Sync + fmt::Debug + 'static {
    /// Short label used in errors and logs, such as `s3`.
    fn provider(&self) -> &'static str;

    /// Size of the object at `key`, or `None` when it is absent.
    async fn head(&self, key: &str) -> Result<Option<u64>>;

    /// Sizes of several objects, in request order. Stores with a batched
    /// lookup override this; the default asks concurrently.
    async fn head_many(&self, keys: &[String]) -> Result<Vec<Option<u64>>> {
        let lookups: Vec<_> = keys.iter().map(|key| self.head(key)).collect();
        futures::stream::iter(lookups)
            .buffered(self.max_concurrent_transfers().max(1))
            .try_collect()
            .await
    }

    /// Stream the object at `key`, or `None` when it is absent.
    async fn get(&self, key: &str) -> Result<Option<RemoteBody>>;

    /// Store `bytes` at `key`.
    async fn put_bytes(&self, key: &str, bytes: Bytes) -> Result<()>;

    /// Store the `size`-byte file at `path` under `key`, streaming it.
    async fn put_file(&self, key: &str, path: &Path, size: u64) -> Result<()>;

    /// Most memory [`put_file`](Self::put_file) buffers for a `size`-byte
    /// file. The cache reserves it from the transfer budget first.
    fn upload_memory(&self, size: u64) -> u64;

    /// Transfers this store runs at once.
    fn max_concurrent_transfers(&self) -> usize {
        DEFAULT_MAX_CONCURRENT_TRANSFERS
    }
}

/// Environment variable that makes the generic remote tier read only when
/// set to `1` or `true`. This does not control the Tuist tier.
pub const STRUO_CACHE_READ_ONLY_ENV: &str = "STRUO_CACHE_READ_ONLY";

/// Whether a remote cache may publish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RemoteAccess {
    /// Read from and publish to the remote store.
    #[default]
    ReadWrite,
    /// Read only. Local writes still succeed and are never published, so an
    /// untrusted job cannot place records other jobs reuse.
    ReadOnly,
}

impl RemoteAccess {
    /// Access for a provider configured with `read_only`, made read only by
    /// [`STRUO_CACHE_READ_ONLY_ENV`] as well. The environment can only narrow
    /// access.
    pub fn from_config(read_only: bool) -> Result<Self> {
        Self::resolve(
            read_only,
            std::env::var(STRUO_CACHE_READ_ONLY_ENV).ok().as_deref(),
        )
    }

    fn resolve(read_only: bool, env: Option<&str>) -> Result<Self> {
        let env_read_only = match env.map(str::trim) {
            None | Some("" | "0" | "false") => false,
            Some("1" | "true") => true,
            Some(other) => {
                return Err(Error::InvalidConfig {
                    provider: "remote",
                    message: format!(
                        "{STRUO_CACHE_READ_ONLY_ENV} must be `1`, `true`, `0`, or `false`, not `{other}`"
                    ),
                })
            }
        };
        Ok(if read_only || env_read_only {
            Self::ReadOnly
        } else {
            Self::ReadWrite
        })
    }
}

/// A local store fronting a [`RemoteStore`].
///
/// Reads consult the local tier first and verify remote content against its
/// digest and declared size before it enters the local tier. Writes land
/// locally first; publishing them is best effort.
#[derive(Debug, Clone)]
pub struct RemoteCache {
    local: Cas,
    store: Arc<dyn RemoteStore>,
    access: RemoteAccess,
    budget: TransferBudget,
    transfers: Arc<Semaphore>,
    downloads: Arc<Flights>,
}

impl RemoteCache {
    /// Front `store` with `local`.
    pub fn new(local: Cas, store: Arc<dyn RemoteStore>, access: RemoteAccess) -> Self {
        let transfers = Arc::new(Semaphore::new(store.max_concurrent_transfers().max(1)));
        Self {
            local,
            store,
            access,
            budget: TransferBudget::default(),
            transfers,
            downloads: Arc::default(),
        }
    }

    /// The local tier.
    pub fn local(&self) -> &Cas {
        &self.local
    }

    /// The remote store.
    pub fn store(&self) -> &Arc<dyn RemoteStore> {
        &self.store
    }

    /// Whether this cache publishes.
    pub fn access(&self) -> RemoteAccess {
        self.access
    }

    pub(crate) fn set_transfer_budget(&mut self, budget: TransferBudget) {
        self.budget = budget;
    }

    pub(crate) fn transfer_budget(&self) -> &TransferBudget {
        &self.budget
    }

    pub(crate) async fn put_blob(&self, bytes: &[u8]) -> Result<Digest> {
        let digest = self.local.put_blob(bytes).await?;
        if self.access == RemoteAccess::ReadWrite {
            if let Err(error) = self.upload_bytes(&digest, bytes).await {
                tracing::warn!(
                    provider = self.store.provider(),
                    blob_digest = %digest,
                    error = %error,
                    "failed to upload blob to remote cache"
                );
            }
        }
        Ok(digest)
    }

    /// Publish a blob the local tier holds. Best effort.
    pub(crate) async fn publish_local_blob(&self, digest: &Digest) {
        if self.access == RemoteAccess::ReadOnly {
            return;
        }
        if let Err(error) = self.upload_local_blob(digest).await {
            tracing::warn!(
                provider = self.store.provider(),
                blob_digest = %digest,
                error = %error,
                "failed to upload blob to remote cache"
            );
        }
    }

    pub(crate) async fn get_blob(&self, digest: &Digest) -> Result<Vec<u8>> {
        match self.local.get_blob(digest).await {
            Ok(bytes) => return Ok(bytes),
            Err(Error::BlobNotFound(_)) => {}
            Err(error) if error.is_corrupt() => {
                tracing::warn!(blob_digest = %digest, error = %error, "refetching a corrupt local blob");
            }
            Err(error) => return Err(error),
        }
        self.ensure_blob_local_guarded(digest, ()).await?;
        self.local.get_blob(digest).await
    }

    pub(crate) async fn copy_blob_to_file(
        &self,
        digest: &Digest,
        destination: &Path,
    ) -> Result<()> {
        self.ensure_blob_local_guarded(digest, ()).await?;
        match self.local.copy_blob_to_file(digest, destination).await {
            Err(error) if error.is_corrupt() => {
                tracing::warn!(blob_digest = %digest, error = %error, "refetching a corrupt local blob");
                self.ensure_blob_local_guarded(digest, ()).await?;
                self.local.copy_blob_to_file(digest, destination).await
            }
            result => result,
        }
    }

    pub(crate) async fn prefetch_blobs_guarded<G: Clone + Send + Sync + 'static>(
        &self,
        digests: &[Digest],
        guard: G,
    ) -> Result<()> {
        let mut digests = digests.to_vec();
        digests.sort_unstable_by_key(|digest| *digest.as_bytes());
        digests.dedup();
        futures::stream::iter(digests)
            .map(|digest| {
                let guard = guard.clone();
                async move { self.ensure_blob_local_guarded(&digest, guard).await }
            })
            .buffer_unordered(self.store.max_concurrent_transfers().max(1))
            .try_collect::<()>()
            .await
    }

    /// Fetch a blob into the local tier unless it is already there.
    pub(crate) async fn ensure_blob_local_guarded<G: Send + 'static>(
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
        let _flight = self.downloads.acquire(*digest).await;
        if self.local.has_blob(digest).await? {
            return Ok(());
        }
        let key = blob_key(digest);
        let Some(size) = self.head_remote(&key).await? else {
            return Err(Error::BlobNotFound(*digest));
        };
        let staging = self.budget.reserve_staging(staging_bytes(size)).await?;
        let memory = self.budget.reserve_memory(DOWNLOAD_MEMORY_BYTES).await?;
        let permit = Arc::clone(&self.transfers)
            .acquire_owned()
            .await
            .expect("transfer semaphore remains open");
        let Some(body) = self.store.get(&key).await? else {
            return Err(Error::BlobNotFound(*digest));
        };
        if body.size != size {
            return Err(self.error(
                "get blob",
                ErrorKind::Protocol,
                "object size changed after HEAD".into(),
            ));
        }
        self.store_blob(digest, body, (guard, permit, memory, staging))
            .await
    }

    async fn store_blob<G: Send + 'static>(
        &self,
        digest: &Digest,
        body: RemoteBody,
        guard: G,
    ) -> Result<()> {
        let started = Instant::now();
        let stored = self
            .local
            .mirror_remote_verified(
                RemoteReader(body.reader),
                body.size,
                false,
                Some(*digest),
                None,
                guard,
            )
            .await
            .map_err(|error| self.ingest_error(digest, error))?;
        if stored != *digest {
            return Err(self.error(
                "get blob",
                ErrorKind::Corrupt,
                format!("remote blob {digest} did not match its digest"),
            ));
        }
        transfer::record(
            TransferDirection::Download,
            *digest,
            body.size,
            started.elapsed(),
        );
        Ok(())
    }

    pub(crate) async fn has_blob(&self, digest: &Digest) -> Result<bool> {
        if self.local.has_blob(digest).await? || is_empty_blob(digest) {
            return Ok(true);
        }
        Ok(self.head_remote(&blob_key(digest)).await?.is_some())
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
        if !self.referenced_blobs_available(&result).await? {
            tracing::debug!(
                provider = self.store.provider(),
                action_digest = %action,
                "remote action result references evicted content"
            );
            return Ok(None);
        }
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

    pub(crate) async fn put_action_result(
        &self,
        action: &Digest,
        result: &ActionResult,
    ) -> Result<()> {
        self.local.put_action_result(action, result).await?;
        if self.access == RemoteAccess::ReadOnly {
            return Ok(());
        }
        if let Err(error) = self.put_action_result_remote(action, result).await {
            tracing::warn!(
                provider = self.store.provider(),
                action_digest = %action,
                error = %error,
                "failed to upload action result to remote cache"
            );
        }
        Ok(())
    }

    async fn get_action_result_remote(&self, action: &Digest) -> Result<Option<ActionResult>> {
        let key = action_key(action);
        let Some(size) = self.head_remote(&key).await? else {
            return Ok(None);
        };
        if size > MAX_ACTION_RECORD_BYTES {
            return Err(self.error(
                "get action result",
                ErrorKind::Protocol,
                format!("action record {action} is {size} bytes"),
            ));
        }
        let _memory = self
            .budget
            .reserve_memory(size.saturating_add(1).saturating_mul(2))
            .await?;
        let _permit = self
            .transfers
            .acquire()
            .await
            .expect("transfer semaphore remains open");
        let Some(body) = self.store.get(&key).await? else {
            return Ok(None);
        };
        if body.size != size {
            return Err(self.error(
                "get action result",
                ErrorKind::Protocol,
                "object size changed after HEAD".into(),
            ));
        }
        let mut bytes = Vec::with_capacity(usize::try_from(size + 1).unwrap_or_default());
        body.reader
            .take(body.size.saturating_add(1))
            .read_to_end(&mut bytes)
            .await
            .map_err(|error| {
                self.error(
                    "get action result",
                    ErrorKind::Unavailable,
                    format!("reading action record {action}: {error}"),
                )
            })?;
        if bytes.len() as u64 != body.size {
            return Err(self.error(
                "get action result",
                ErrorKind::Protocol,
                format!(
                    "action record {action} declared {} bytes and returned {}",
                    body.size,
                    bytes.len()
                ),
            ));
        }
        serde_json::from_slice(&bytes).map(Some).map_err(|error| {
            self.error(
                "get action result",
                ErrorKind::Protocol,
                format!("action record {action} is malformed: {error}"),
            )
        })
    }

    async fn referenced_blobs_available(&self, result: &ActionResult) -> Result<bool> {
        let mut keys = Vec::new();
        for digest in referenced_blobs(result) {
            if !is_empty_blob(digest) && !self.local.has_blob(digest).await? {
                keys.push(blob_key(digest));
            }
        }
        if keys.is_empty() {
            return Ok(true);
        }
        for keys in keys.chunks(self.store.max_concurrent_transfers().max(1)) {
            let _permits = self
                .transfers
                .acquire_many(u32::try_from(keys.len()).expect("batch fits semaphore capacity"))
                .await
                .expect("transfer semaphore remains open");
            let sizes = self.store.head_many(keys).await?;
            if sizes.len() != keys.len() {
                return Err(self.error(
                    "head many",
                    ErrorKind::Protocol,
                    "availability response count does not match request".into(),
                ));
            }
            if sizes.iter().any(Option::is_none) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Upload every referenced blob, then the record, so a reader never sees
    /// a record before its content.
    async fn put_action_result_remote(&self, action: &Digest, result: &ActionResult) -> Result<()> {
        let mut digests: Vec<Digest> = referenced_blobs(result).copied().collect();
        digests.sort_unstable_by_key(|digest| *digest.as_bytes());
        digests.dedup();
        futures::stream::iter(digests)
            .map(|digest| async move { self.upload_local_blob(&digest).await })
            .buffer_unordered(self.store.max_concurrent_transfers().max(1))
            .try_collect::<()>()
            .await?;
        let mut size = RecordSize(0);
        serde_json::to_writer(&mut size, result).expect("ActionResult is serializable");
        if size.0 > MAX_ACTION_RECORD_BYTES {
            return Err(self.error(
                "put action result",
                ErrorKind::Exhausted,
                "action record exceeds the maximum supported size".to_string(),
            ));
        }
        let memory = self.budget.reserve_memory(size.0).await?;
        let mut record =
            Vec::with_capacity(usize::try_from(size.0).expect("record is at most 16 MiB"));
        serde_json::to_writer(&mut record, result).expect("ActionResult is serializable");
        let permit = Arc::clone(&self.transfers)
            .acquire_owned()
            .await
            .expect("transfer semaphore remains open");
        let store = Arc::clone(&self.store);
        let key = action_key(action);
        self.finish_upload(async move {
            let _owner = (memory, permit);
            store.put_bytes(&key, Bytes::from(record)).await
        })
        .await
    }

    async fn upload_bytes(&self, digest: &Digest, bytes: &[u8]) -> Result<()> {
        if is_empty_blob(digest) {
            return Ok(());
        }
        let key = blob_key(digest);
        let size = bytes.len() as u64;
        if self.head_remote(&key).await? == Some(size) {
            return Ok(());
        }
        let memory = self.budget.reserve_memory(size).await?;
        let permit = Arc::clone(&self.transfers)
            .acquire_owned()
            .await
            .expect("transfer semaphore remains open");
        let bytes = Bytes::copy_from_slice(bytes);
        let store = Arc::clone(&self.store);
        let digest = *digest;
        self.finish_upload(async move {
            let _owner = (memory, permit);
            let started = Instant::now();
            store.put_bytes(&key, bytes).await?;
            transfer::record(TransferDirection::Upload, digest, size, started.elapsed());
            Ok(())
        })
        .await
    }

    /// Publish a local blob from a verified copy in temporary space, which
    /// counts against the staging budget like a download does. A blob the
    /// store already holds at the same size is not sent again.
    pub(crate) async fn upload_local_blob(&self, digest: &Digest) -> Result<()> {
        if is_empty_blob(digest) {
            return Ok(());
        }
        let key = blob_key(digest);
        let size = self.local.blob_size(digest).await?;
        if self.head_remote(&key).await? == Some(size) {
            return Ok(());
        }
        let staging = self.budget.reserve_staging(size).await?;
        let memory = self
            .budget
            .reserve_memory(self.store.upload_memory(size).max(DOWNLOAD_MEMORY_BYTES))
            .await?;
        let permit = Arc::clone(&self.transfers)
            .acquire_owned()
            .await
            .expect("transfer semaphore remains open");
        let directory = tempfile::tempdir().map_err(|source| Error::Io {
            path: std::env::temp_dir(),
            source,
        })?;
        let path = directory.path().join("blob");
        let owner = Arc::new((directory, permit, memory, staging));
        self.local
            .copy_blob_to_file_guarded(digest, &path, Arc::clone(&owner), |reader, writer| {
                let mut reader = std::io::BufReader::with_capacity(512 * 1024, reader);
                std::io::copy(&mut reader, writer).map(|_| ())
            })
            .await?;
        let store = Arc::clone(&self.store);
        let digest = *digest;
        self.finish_upload(async move {
            let _owner = owner;
            let started = Instant::now();
            store.put_file(&key, &path, size).await?;
            transfer::record(TransferDirection::Upload, digest, size, started.elapsed());
            Ok(())
        })
        .await
    }

    async fn finish_upload(
        &self,
        upload: impl std::future::Future<Output = Result<()>> + Send + 'static,
    ) -> Result<()> {
        // Backends can detach blocking writes on cancellation. Keep their
        // future and admission alive until those writes actually finish.
        tokio::spawn(upload)
            .await
            .map_err(|_| self.error("put", ErrorKind::Unavailable, "upload task failed".into()))?
    }

    async fn head_remote(&self, key: &str) -> Result<Option<u64>> {
        let _permit = self
            .transfers
            .acquire()
            .await
            .expect("transfer semaphore remains open");
        self.store.head(key).await
    }

    fn ingest_error(&self, digest: &Digest, error: Error) -> Error {
        let Error::Io { source, .. } = &error else {
            return error;
        };
        if let Some(read) = source
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<RemoteReadFailed>())
        {
            return self.error(
                "get blob",
                ErrorKind::Unavailable,
                format!("reading remote blob {digest}: {}", read.0),
            );
        }
        if error.kind() == ErrorKind::Io
            && matches!(
                source.kind(),
                std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof
            )
        {
            return self.error(
                "get blob",
                ErrorKind::Corrupt,
                format!("remote blob {digest} failed verification: {source}"),
            );
        }
        error
    }

    fn error(&self, operation: &'static str, kind: ErrorKind, message: String) -> Error {
        Error::remote(self.store.provider(), operation, kind, message)
    }
}

/// Marks transport failures so ingest can tell them from verification
/// failures, which surface as I/O errors too.
struct RemoteReader(Pin<Box<dyn AsyncRead + Send>>);

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct RemoteReadFailed(std::io::Error);

impl AsyncRead for RemoteReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.0
            .as_mut()
            .poll_read(cx, buf)
            .map_err(|error| std::io::Error::other(RemoteReadFailed(error)))
    }
}

struct RecordSize(u64);

impl std::io::Write for RecordSize {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len() as u64);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn referenced_blobs(result: &ActionResult) -> impl Iterator<Item = &Digest> {
    result
        .stdout
        .iter()
        .chain(result.stderr.iter())
        .chain(result.outputs.values())
}

fn staging_bytes(raw: u64) -> u64 {
    // Ingest stages the raw body and may also stage its compressed form.
    raw.saturating_mul(2).saturating_add(64 * 1024)
}

/// Key a store holds content under.
pub fn blob_key(digest: &Digest) -> String {
    format!("cas/{}", digest.to_hex())
}

/// Key a store holds an action record under.
pub fn action_key(action: &Digest) -> String {
    format!("ac/{}", action.to_hex())
}

fn is_empty_blob(digest: &Digest) -> bool {
    *digest == Digest::of_bytes(&[])
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod admission_tests;

#[cfg(test)]
mod upload_tests;
