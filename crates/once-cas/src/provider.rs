use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt};

use crate::tuist::{TuistCache, TuistCacheConfig};
use crate::{ActionResult, Cas, Digest, GcReport, Result, Stats};

const TUIST_STREAM_REMOTE_UPLOAD_LIMIT: u64 = 8 * 1024 * 1024;

/// The cache a command talks to: a local store, optionally fronting a
/// remote tier.
///
/// Every method dispatches to the selected tier, so callers do not branch
/// on which provider is configured.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum CacheProvider {
    /// Filesystem-backed store with no remote tier.
    Local(Cas),
    /// Local store fronting a Tuist remote tier, which is consulted on a
    /// local miss and mirrored to on write.
    Tuist(TuistCache),
}

impl CacheProvider {
    /// Open a purely local store rooted at `root`.
    pub fn open_local(root: impl Into<PathBuf>) -> Self {
        Self::Local(Cas::open(root))
    }

    /// Open a local store backed by the Tuist remote tier.
    ///
    /// `auth_root` holds the credentials the remote tier authenticates
    /// with; it is kept separate from the cache root so clearing the
    /// cache never signs the user out.
    pub fn tuist(
        local_root: impl Into<PathBuf>,
        auth_root: impl AsRef<Path>,
        config: TuistCacheConfig,
    ) -> Result<Self> {
        Ok(Self::Tuist(TuistCache::new(
            Cas::open(local_root),
            auth_root,
            config,
        )?))
    }

    /// Filesystem root of the local tier, remote tier or not.
    pub fn root(&self) -> &Path {
        match self {
            Self::Local(cas) => cas.root(),
            Self::Tuist(cache) => cache.local().root(),
        }
    }

    /// Store a blob already held in memory and return its digest.
    ///
    /// Prefer [`put_stream`](Self::put_stream) for anything file-sized.
    pub async fn put_blob(&self, bytes: &[u8]) -> Result<Digest> {
        match self {
            Self::Local(cas) => cas.put_blob(bytes).await,
            Self::Tuist(cache) => cache.put_blob(bytes).await,
        }
    }

    /// Store a blob by streaming it, without holding it all in memory.
    ///
    /// Only blobs below an internal size limit are mirrored to a remote
    /// tier, so a huge stream stays local rather than stalling the write
    /// on a slow upload.
    pub async fn put_stream<R: AsyncRead + Unpin>(&self, reader: R) -> Result<Digest> {
        match self {
            Self::Local(cas) => cas.put_stream(reader).await,
            Self::Tuist(cache) => {
                let digest = cache.local().put_stream(reader).await?;
                if cache.local().blob_size(&digest).await? <= TUIST_STREAM_REMOTE_UPLOAD_LIMIT {
                    let bytes = cache.local().get_blob(&digest).await?;
                    let _ = cache.put_blob(&bytes).await?;
                }
                Ok(digest)
            }
        }
    }

    /// Read a whole blob into memory, consulting the remote tier on a
    /// local miss.
    ///
    /// Use [`copy_blob_to_file`](Self::copy_blob_to_file) or
    /// [`read_blob_limited`](Self::read_blob_limited) when the blob may
    /// be large.
    pub async fn get_blob(&self, digest: &Digest) -> Result<Vec<u8>> {
        match self {
            Self::Local(cas) => cas.get_blob(digest).await,
            Self::Tuist(cache) => cache.get_blob(digest).await,
        }
    }

    /// Materialize a blob at `destination`, streaming it rather than
    /// buffering it in memory.
    pub async fn copy_blob_to_file(&self, digest: &Digest, destination: &Path) -> Result<()> {
        match self {
            Self::Local(cas) => cas.copy_blob_to_file(digest, destination).await,
            Self::Tuist(cache) => {
                cache.ensure_blob_local(digest).await?;
                cache.local().copy_blob_to_file(digest, destination).await
            }
        }
    }

    /// Read at most `limit` bytes of a blob, from the start or, when
    /// `from_end` is set, from the tail.
    ///
    /// Returns the bytes and whether the blob was longer than `limit`.
    /// Reading the tail is what a caller wants when surfacing the end of
    /// a long log without paying for the whole thing.
    pub async fn read_blob_limited(
        &self,
        digest: &Digest,
        limit: u64,
        from_end: bool,
    ) -> Result<(Vec<u8>, bool)> {
        let directory = tempfile::tempdir().map_err(|source| crate::Error::Io {
            path: std::env::temp_dir(),
            source,
        })?;
        let path = directory.path().join("blob");
        self.copy_blob_to_file(digest, &path).await?;
        let mut file = tokio::fs::File::open(&path)
            .await
            .map_err(|source| crate::Error::Io {
                path: path.clone(),
                source,
            })?;
        let len = file
            .metadata()
            .await
            .map_err(|source| crate::Error::Io {
                path: path.clone(),
                source,
            })?
            .len();
        let truncated = len > limit;
        if from_end && truncated {
            file.seek(SeekFrom::Start(len - limit))
                .await
                .map_err(|source| crate::Error::Io {
                    path: path.clone(),
                    source,
                })?;
        }
        let capacity = usize::try_from(len.min(limit)).map_err(|_| crate::Error::Io {
            path: path.clone(),
            source: std::io::Error::other("blob limit exceeds addressable memory"),
        })?;
        let mut bytes = Vec::with_capacity(capacity);
        file.take(limit)
            .read_to_end(&mut bytes)
            .await
            .map_err(|source| crate::Error::Io { path, source })?;
        Ok((bytes, truncated))
    }

    /// True if a content-addressed blob exists. For Tuist this consults
    /// the remote tier on local miss so it mirrors `get_blob`'s reach;
    /// scripts can probe `exists` then `get` without surprises.
    pub async fn has_blob(&self, digest: &Digest) -> Result<bool> {
        match self {
            Self::Local(cas) => cas.has_blob(digest).await,
            Self::Tuist(cache) => cache.has_blob(digest).await,
        }
    }

    /// Record the result of an action under its action digest.
    pub async fn put_action_result(&self, action: &Digest, result: &ActionResult) -> Result<()> {
        match self {
            Self::Local(cas) => cas.put_action_result(action, result).await,
            Self::Tuist(cache) => cache.put_action_result(action, result).await,
        }
    }

    /// Look up a cached action result. `None` is a miss; a stored record
    /// that fails to decode is also treated as a miss.
    pub async fn get_action_result(&self, action: &Digest) -> Result<Option<ActionResult>> {
        match self {
            Self::Local(cas) => cas.get_action_result(action).await,
            Self::Tuist(cache) => cache.get_action_result(action).await,
        }
    }

    /// Drop a cached action result, returning whether one was present.
    pub async fn forget_action(&self, action: &Digest) -> Result<bool> {
        match self {
            Self::Local(cas) => cas.forget_action(action).await,
            Self::Tuist(cache) => cache.forget_action(action).await,
        }
    }

    /// Count what the local tier currently holds.
    pub async fn stats(&self) -> Result<Stats> {
        match self {
            Self::Local(cas) => cas.stats().await,
            Self::Tuist(cache) => cache.local().stats().await,
        }
    }

    /// Reclaim local disk space until it fits within `max_bytes`.
    ///
    /// Only the local tier is collected: the Tuist remote tier is shared
    /// and managed server-side, so `gc` never deletes from it.
    pub async fn gc(&self, max_bytes: u64, dry_run: bool) -> Result<GcReport> {
        match self {
            Self::Local(cas) => cas.gc(max_bytes, dry_run).await,
            Self::Tuist(cache) => cache.local().gc(max_bytes, dry_run).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::TempDir;

    use super::CacheProvider;
    use crate::{ActionResult, Digest};

    #[tokio::test]
    async fn open_local_roots_at_the_given_directory() {
        let tmp = TempDir::new().unwrap();
        let provider = CacheProvider::open_local(tmp.path());
        assert_eq!(provider.root(), tmp.path());
    }

    #[tokio::test]
    async fn local_blob_roundtrips_through_the_provider() {
        let tmp = TempDir::new().unwrap();
        let provider = CacheProvider::open_local(tmp.path());
        let digest = provider.put_blob(b"payload").await.unwrap();
        assert!(provider.has_blob(&digest).await.unwrap());
        assert_eq!(provider.get_blob(&digest).await.unwrap(), b"payload");
    }

    #[tokio::test]
    async fn local_has_blob_is_false_for_unknown_digest() {
        let tmp = TempDir::new().unwrap();
        let provider = CacheProvider::open_local(tmp.path());
        assert!(!provider
            .has_blob(&Digest::of_bytes(b"absent"))
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn local_put_stream_stores_and_addresses_by_content() {
        let tmp = TempDir::new().unwrap();
        let provider = CacheProvider::open_local(tmp.path());
        let digest = provider.put_stream(&b"streamed"[..]).await.unwrap();
        assert_eq!(digest, Digest::of_bytes(b"streamed"));
        assert_eq!(provider.get_blob(&digest).await.unwrap(), b"streamed");
    }

    #[tokio::test]
    async fn limited_blob_reads_bound_prefixes_and_suffixes() {
        let tmp = TempDir::new().unwrap();
        let provider = CacheProvider::open_local(tmp.path());
        let digest = provider.put_blob(b"0123456789").await.unwrap();

        assert_eq!(
            provider.read_blob_limited(&digest, 4, false).await.unwrap(),
            (b"0123".to_vec(), true)
        );
        assert_eq!(
            provider.read_blob_limited(&digest, 4, true).await.unwrap(),
            (b"6789".to_vec(), true)
        );
    }

    #[tokio::test]
    async fn local_action_result_roundtrips_and_forgets() {
        let tmp = TempDir::new().unwrap();
        let provider = CacheProvider::open_local(tmp.path());
        let stdout = provider.put_blob(b"out").await.unwrap();
        let action = Digest::of_bytes(b"action");
        let result = ActionResult {
            exit_code: 0,
            stdout: Some(stdout),
            stderr: None,
            outputs: BTreeMap::new(),
        };
        provider.put_action_result(&action, &result).await.unwrap();
        assert_eq!(
            provider.get_action_result(&action).await.unwrap(),
            Some(result)
        );
        assert!(provider.forget_action(&action).await.unwrap());
        assert_eq!(provider.get_action_result(&action).await.unwrap(), None);
    }

    #[tokio::test]
    async fn local_stats_count_stored_blobs() {
        let tmp = TempDir::new().unwrap();
        let provider = CacheProvider::open_local(tmp.path());
        provider.put_blob(b"a").await.unwrap();
        provider.put_blob(b"bb").await.unwrap();
        let stats = provider.stats().await.unwrap();
        assert_eq!(stats.blob_count, 2);
    }
}
