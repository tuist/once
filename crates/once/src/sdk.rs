use std::path::{Path, PathBuf};

use once_cas::{ActionResult, CacheProvider, Digest, Stats};
use once_core::Xdg;

/// Result type used by the high-level Once SDK.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors returned by the high-level Once SDK.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid digest: {0}")]
    InvalidDigest(String),
    #[error(transparent)]
    Cache(#[from] once_cas::Error),
}

/// Embeddable Once cache client.
///
/// `Cache` is cheap to clone and can be reused across many cache
/// operations. The default constructor opens the local filesystem cache
/// at `$XDG_CACHE_HOME/once/cas`, or `$HOME/.cache/once/cas` when
/// `XDG_CACHE_HOME` is not set.
#[derive(Clone)]
pub struct Cache {
    cache: CacheProvider,
}

impl Cache {
    /// Create a client backed by Once's default XDG local cache.
    pub fn new() -> Self {
        Self::local(Xdg::from_env().once_cas())
    }

    /// Create a client backed by a local filesystem cache.
    pub fn local(local_cache_root: impl Into<PathBuf>) -> Self {
        Self::with_provider(CacheProvider::open_local(local_cache_root))
    }

    /// Create a client with an explicit cache provider.
    pub fn with_provider(cache: CacheProvider) -> Self {
        Self { cache }
    }

    /// Cache provider used by this client.
    pub fn provider(&self) -> &CacheProvider {
        &self.cache
    }

    /// Root directory used by the underlying provider.
    pub fn root(&self) -> &Path {
        self.cache.root()
    }

    /// Store a blob and return its content digest.
    pub async fn put_blob(&self, bytes: &[u8]) -> Result<Digest> {
        Ok(self.cache.put_blob(bytes).await?)
    }

    /// Read a blob by digest.
    pub async fn get_blob(&self, digest: Digest) -> Result<Vec<u8>> {
        Ok(self.cache.get_blob(&digest).await?)
    }

    /// Return whether a blob exists.
    pub async fn has_blob(&self, digest: Digest) -> Result<bool> {
        Ok(self.cache.has_blob(&digest).await?)
    }

    /// Store the cached result for an action digest.
    pub async fn put_action_result(&self, action: Digest, result: &ActionResult) -> Result<()> {
        Ok(self.cache.put_action_result(&action, result).await?)
    }

    /// Read the cached result for an action digest.
    pub async fn get_action_result(&self, action: Digest) -> Result<Option<ActionResult>> {
        Ok(self.cache.get_action_result(&action).await?)
    }

    /// Remove one cached action result, leaving referenced blobs intact.
    pub async fn forget_action(&self, action: Digest) -> Result<bool> {
        Ok(self.cache.forget_action(&action).await?)
    }

    /// Return local cache statistics.
    pub async fn stats(&self) -> Result<Stats> {
        Ok(self.cache.stats().await?)
    }
}

impl Default for Cache {
    fn default() -> Self {
        Self::new()
    }
}

/// Backward-compatible name for the cache client.
pub type OnceCache = Cache;

/// Parse a lowercase BLAKE3 hex digest.
pub fn digest_from_hex(hex: &str) -> Result<Digest> {
    Digest::from_hex(hex).ok_or_else(|| Error::InvalidDigest(hex.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn default_cache_uses_xdg_cas_root() {
        let cache = Cache::new();

        assert!(cache.root().ends_with("once/cas"));
    }

    #[tokio::test]
    async fn stores_and_reads_blobs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cache = Cache::local(tmp.path());

        let digest = cache.put_blob(b"hello").await.unwrap();

        assert!(cache.has_blob(digest).await.unwrap());
        assert_eq!(cache.get_blob(digest).await.unwrap(), b"hello");
    }

    #[tokio::test]
    async fn stores_and_reads_action_results() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cache = Cache::local(tmp.path());
        let stdout = cache.put_blob(b"out").await.unwrap();
        let action = Digest::of_bytes(b"action");
        let result = ActionResult {
            exit_code: 0,
            stdout: Some(stdout),
            stderr: None,
            outputs: BTreeMap::new(),
        };

        cache.put_action_result(action, &result).await.unwrap();

        assert_eq!(cache.get_action_result(action).await.unwrap(), Some(result));
        assert!(cache.forget_action(action).await.unwrap());
        assert_eq!(cache.get_action_result(action).await.unwrap(), None);
    }
}
