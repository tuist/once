use std::path::{Path, PathBuf};

use tokio::io::AsyncRead;

use crate::tuist::{TuistCache, TuistCacheConfig};
use crate::{ActionResult, Cas, Digest, Result, Stats};

const TUIST_STREAM_REMOTE_UPLOAD_LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum CacheProvider {
    Local(Cas),
    Tuist(TuistCache),
}

impl CacheProvider {
    pub fn open_local(root: impl Into<PathBuf>) -> Self {
        Self::Local(Cas::open(root))
    }

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

    pub fn root(&self) -> &Path {
        match self {
            Self::Local(cas) => cas.root(),
            Self::Tuist(cache) => cache.local().root(),
        }
    }

    pub async fn put_blob(&self, bytes: &[u8]) -> Result<Digest> {
        match self {
            Self::Local(cas) => cas.put_blob(bytes).await,
            Self::Tuist(cache) => cache.put_blob(bytes).await,
        }
    }

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

    pub async fn get_blob(&self, digest: &Digest) -> Result<Vec<u8>> {
        match self {
            Self::Local(cas) => cas.get_blob(digest).await,
            Self::Tuist(cache) => cache.get_blob(digest).await,
        }
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

    pub async fn put_action_result(&self, action: &Digest, result: &ActionResult) -> Result<()> {
        match self {
            Self::Local(cas) => cas.put_action_result(action, result).await,
            Self::Tuist(cache) => cache.put_action_result(action, result).await,
        }
    }

    pub async fn get_action_result(&self, action: &Digest) -> Result<Option<ActionResult>> {
        match self {
            Self::Local(cas) => cas.get_action_result(action).await,
            Self::Tuist(cache) => cache.get_action_result(action).await,
        }
    }

    pub async fn forget_action(&self, action: &Digest) -> Result<bool> {
        match self {
            Self::Local(cas) => cas.forget_action(action).await,
            Self::Tuist(cache) => cache.forget_action(action).await,
        }
    }

    pub async fn stats(&self) -> Result<Stats> {
        match self {
            Self::Local(cas) => cas.stats().await,
            Self::Tuist(cache) => cache.local().stats().await,
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
