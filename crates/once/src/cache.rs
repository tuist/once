use std::path::{Path, PathBuf};

use once_cas::{ActionResult, CacheProvider, Digest, Stats, TuistCacheConfig};

pub type Result<T> = std::result::Result<T, Error>;

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
    /// Create a client backed by Once's default base-directory local cache.
    pub fn new() -> Self {
        Self::with_provider(CacheProvider::open_local(default_cache_root()))
    }

    /// Create a client backed by a caller-owned local cache root.
    pub fn local(root: impl Into<PathBuf>) -> Self {
        Self::with_provider(CacheProvider::open_local(root))
    }

    /// Create a client using the effective provider for a workspace.
    ///
    /// Provider selection matches the command line: the process override,
    /// workspace infrastructure, legacy Tuist project configuration, and the
    /// user default are considered in that order.
    pub fn from_workspace(workspace: impl AsRef<Path>) -> Result<Self> {
        let xdg = once_core::Xdg::from_env();
        let config = once_frontend::resolve_cache_provider(
            workspace.as_ref(),
            &xdg.config_home.join("once").join("config.toml"),
            std::env::var("ONCE_CACHE_PROVIDER").ok(),
        )
        .map_err(|error| once_cas::Error::InvalidConfig {
            provider: "workspace",
            message: error.to_string(),
        })?;
        let provider = match config {
            once_frontend::ResolvedCacheProviderConfig::Local => {
                CacheProvider::open_local(xdg.once_cas())
            }
            once_frontend::ResolvedCacheProviderConfig::Tuist(config) => {
                let oauth_client_id =
                    non_empty_env(once_cas::TUIST_OAUTH_CLIENT_ID_ENV).or(config.oauth_client_id);
                CacheProvider::tuist(
                    xdg.once_cas(),
                    xdg.config_home.join("once").join("credentials"),
                    TuistCacheConfig {
                        url: config.url,
                        account: config.account,
                        project: config.project,
                        oauth_client_id,
                        provider_name: config.provider_name,
                    },
                )?
            }
        };
        Ok(Self::with_provider(provider))
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

    /// Store bytes from an asynchronous reader with bounded memory use.
    pub async fn put_stream<R>(&self, reader: R) -> Result<Digest>
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        Ok(self.cache.put_stream(reader).await?)
    }

    /// Store a file without loading the complete file into memory.
    pub async fn put_file(&self, path: impl AsRef<Path>) -> Result<Digest> {
        let path = path.as_ref();
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|source| once_cas::Error::Io {
                path: path.to_path_buf(),
                source,
            })?;
        self.put_stream(file).await
    }

    /// Read a blob by digest.
    pub async fn get_blob(&self, digest: Digest) -> Result<Vec<u8>> {
        Ok(self.cache.get_blob(&digest).await?)
    }

    /// Materialize a blob at a file path and return the number of bytes written.
    pub async fn get_blob_to_file(&self, digest: Digest, path: impl AsRef<Path>) -> Result<u64> {
        let path = path.as_ref();
        let bytes = self.get_blob(digest).await?;
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| once_cas::Error::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
        }
        tokio::fs::write(path, &bytes)
            .await
            .map_err(|source| once_cas::Error::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(bytes.len() as u64)
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

pub fn digest_from_hex(hex: &str) -> Result<Digest> {
    Digest::from_hex(hex).ok_or_else(|| Error::InvalidDigest(hex.to_string()))
}

fn default_cache_root() -> PathBuf {
    once_core::Xdg::from_env().once_cas()
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

#[cfg(test)]
mod tests;
