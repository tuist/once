use std::path::{Path, PathBuf};

use tokio::io::AsyncRead;

use crate::tuist::{TuistCache, TuistCacheConfig};
use crate::{ActionResult, Cas, Digest, Result, Stats};

#[derive(Debug, Clone)]
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
                let bytes = cache.local().get_blob(&digest).await?;
                let _ = cache.put_blob(&bytes).await?;
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
