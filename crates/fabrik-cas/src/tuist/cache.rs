use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use reqwest::{Method, StatusCode, Url};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::time::Instant;

use super::{
    join_url, remote_status_message, TuistAuth, TuistCacheConfig, CAS_PATH, ENDPOINTS_PATH,
    HEALTH_PATH, KEY_VALUE_PATH, PROVIDER_NAME,
};
use crate::{ActionResult, Cas, Digest, Error, Result};

#[derive(Debug, Clone)]
pub struct TuistCache {
    local: Cas,
    client: reqwest::Client,
    config: TuistCacheConfig,
    endpoint_cache: Arc<Mutex<Option<String>>>,
    auth: TuistAuth,
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
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|source| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "build client",
                message: source.to_string(),
            })?;
        let auth = TuistAuth::new(auth_root, &config);
        Ok(Self {
            local,
            client,
            config,
            endpoint_cache: Arc::new(Mutex::new(None)),
            auth,
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
        let mirrored = self.local.put_blob(&bytes).await?;
        if mirrored != *digest {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "get blob",
                message: format!("remote blob {digest} did not match requested digest"),
            });
        }
        Ok(bytes)
    }

    pub async fn put_blob(&self, bytes: &[u8]) -> Result<Digest> {
        let digest = self.local.put_blob(bytes).await?;
        let _ = self.put_blob_remote(&digest, bytes).await;
        Ok(digest)
    }

    pub async fn get_action_result(&self, action: &Digest) -> Result<Option<ActionResult>> {
        if let Some(result) = self.local.get_action_result(action).await? {
            return Ok(Some(result));
        }

        match self.get_action_result_remote(action).await {
            Ok(Some(result)) => {
                if self.prefetch_action_blobs(&result).await.is_ok() {
                    let _ = self.local.put_action_result(action, &result).await;
                }
                Ok(Some(result))
            }
            Ok(None) => Ok(None),
            Err(error) if error.is_read_miss() => Ok(None),
            Err(error) => Err(error.into_public_error("get action result")),
        }
    }

    pub async fn put_action_result(&self, action: &Digest, result: &ActionResult) -> Result<()> {
        self.local.put_action_result(action, result).await?;
        let _ = self.put_action_result_remote(action, result).await;
        Ok(())
    }

    pub async fn forget_action(&self, action: &Digest) -> Result<bool> {
        self.local.forget_action(action).await
    }

    async fn prefetch_action_blobs(&self, result: &ActionResult) -> Result<()> {
        let mut digests = Vec::with_capacity(2 + result.outputs.len());
        digests.push(result.stdout);
        digests.push(result.stderr);
        digests.extend(result.outputs.values().copied());
        for digest in digests {
            let _ = self.get_blob(&digest).await?;
        }
        Ok(())
    }

    async fn get_blob_remote(&self, digest: &Digest) -> Result<Vec<u8>> {
        let endpoint = self.data_plane_endpoint().await?;
        let url = self.cas_url(&endpoint, &digest.to_hex())?;
        let response = self
            .authorized_request(Method::GET, url)?
            .send()
            .await
            .map_err(|source| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "get blob",
                message: source.to_string(),
            })?;
        match response.status() {
            StatusCode::OK => {
                response
                    .bytes()
                    .await
                    .map(|bytes| bytes.to_vec())
                    .map_err(|source| Error::Remote {
                        provider: PROVIDER_NAME,
                        operation: "get blob body",
                        message: source.to_string(),
                    })
            }
            StatusCode::NOT_FOUND => Err(Error::BlobNotFound(*digest)),
            _ => Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "get blob",
                message: remote_status_message(response).await,
            }),
        }
    }

    async fn put_blob_remote(&self, digest: &Digest, bytes: &[u8]) -> Result<()> {
        let endpoint = self.data_plane_endpoint().await?;
        let url = self.cas_url(&endpoint, &digest.to_hex())?;
        let response = self
            .authorized_request(Method::POST, url)?
            .body(bytes.to_vec())
            .send()
            .await
            .map_err(|source| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "put blob",
                message: source.to_string(),
            })?;
        match response.status() {
            StatusCode::OK | StatusCode::CREATED | StatusCode::NO_CONTENT => Ok(()),
            _ => Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "put blob",
                message: remote_status_message(response).await,
            }),
        }
    }

    async fn get_action_result_remote(
        &self,
        action: &Digest,
    ) -> std::result::Result<Option<ActionResult>, RemoteReadError> {
        let endpoint = self
            .data_plane_endpoint()
            .await
            .map_err(RemoteReadError::fatal)?;
        let url = self
            .key_value_get_url(&endpoint, &action.to_hex())
            .map_err(RemoteReadError::fatal)?;
        let response = self
            .authorized_request(Method::GET, url)
            .map_err(RemoteReadError::fatal)?
            .send()
            .await
            .map_err(|source| RemoteReadError::miss(format!("request failed: {source}")))?;
        match response.status() {
            StatusCode::OK => {
                let payload: KeyValuePayload = response.json().await.map_err(|source| {
                    RemoteReadError::fatal(Error::Remote {
                        provider: PROVIDER_NAME,
                        operation: "decode action result",
                        message: source.to_string(),
                    })
                })?;
                let Some(value) = payload.entries.first() else {
                    return Ok(None);
                };
                let result = serde_json::from_str(&value.value).map_err(|source| {
                    RemoteReadError::fatal(Error::Remote {
                        provider: PROVIDER_NAME,
                        operation: "decode action result payload",
                        message: source.to_string(),
                    })
                })?;
                Ok(Some(result))
            }
            StatusCode::NOT_FOUND => Ok(None),
            status if status.is_server_error() => {
                Err(RemoteReadError::miss(remote_status_message(response).await))
            }
            _ => Err(RemoteReadError::fatal(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "get action result",
                message: remote_status_message(response).await,
            })),
        }
    }

    async fn put_action_result_remote(&self, action: &Digest, result: &ActionResult) -> Result<()> {
        let endpoint = self.data_plane_endpoint().await?;
        let url = self.key_value_put_url(&endpoint)?;
        let body = PutKeyValuePayload {
            cas_id: action.to_hex(),
            entries: vec![PutKeyValueEntry {
                value: serde_json::to_string(result).expect("ActionResult is serializable"),
            }],
        };
        let response = self
            .authorized_request(Method::PUT, url)?
            .json(&body)
            .send()
            .await
            .map_err(|source| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "put action result",
                message: source.to_string(),
            })?;
        match response.status() {
            StatusCode::OK | StatusCode::CREATED | StatusCode::NO_CONTENT => Ok(()),
            _ => Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "put action result",
                message: remote_status_message(response).await,
            }),
        }
    }

    async fn data_plane_endpoint(&self) -> Result<String> {
        let mut cached = self.endpoint_cache.lock().await;
        if let Some(endpoint) = cached.as_ref() {
            return Ok(endpoint.clone());
        }
        let endpoints = self.fetch_endpoints().await?;
        let endpoint = match endpoints.len() {
            0 => {
                return Err(Error::Remote {
                    provider: PROVIDER_NAME,
                    operation: "discover endpoints",
                    message: "Tuist returned no cache endpoints".to_string(),
                });
            }
            1 => endpoints[0].clone(),
            _ => self.pick_fastest_endpoint(&endpoints).await?,
        };
        *cached = Some(endpoint.clone());
        Ok(endpoint)
    }

    async fn fetch_endpoints(&self) -> Result<Vec<String>> {
        let url = self.endpoints_url()?;
        let response = self
            .authorized_request(Method::GET, url)?
            .send()
            .await
            .map_err(|source| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "discover endpoints",
                message: source.to_string(),
            })?;
        match response.status() {
            StatusCode::OK => {
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
        let probes = endpoints
            .iter()
            .map(|endpoint| Self::health_url(endpoint).map(|url| (endpoint.clone(), url)))
            .collect::<Result<Vec<_>>>()?;

        let mut set = tokio::task::JoinSet::new();
        for (endpoint, health) in probes {
            let client = self.client.clone();
            set.spawn(async move {
                let start = Instant::now();
                let outcome = client
                    .get(health)
                    .timeout(Duration::from_secs(5))
                    .send()
                    .await;
                match outcome {
                    Ok(response) if response.status().is_success() => {
                        Some((start.elapsed(), endpoint))
                    }
                    _ => None,
                }
            });
        }

        let mut best: Option<(Duration, String)> = None;
        while let Some(joined) = set.join_next().await {
            let Ok(Some((elapsed, endpoint))) = joined else {
                continue;
            };
            match &best {
                Some((best_elapsed, _)) if *best_elapsed <= elapsed => {}
                _ => best = Some((elapsed, endpoint)),
            }
        }

        best.map(|(_, endpoint)| endpoint)
            .ok_or_else(|| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "pick fastest endpoint",
                message: "no reachable Tuist cache endpoints".to_string(),
            })
    }

    fn authorized_request(&self, method: Method, url: Url) -> Result<reqwest::RequestBuilder> {
        Ok(self
            .client
            .request(method, url)
            .bearer_auth(self.auth.token()?))
    }

    fn account(&self) -> &str {
        self.config
            .account
            .as_deref()
            .expect("validated at construction")
    }

    fn endpoints_url(&self) -> Result<Url> {
        let mut url = self.server_url(ENDPOINTS_PATH)?;
        url.query_pairs_mut()
            .append_pair("account_handle", self.account());
        Ok(url)
    }

    fn cas_url(&self, endpoint: &str, digest: &str) -> Result<Url> {
        let mut url = Self::endpoint_url(endpoint, &format!("{CAS_PATH}/{digest}"))?;
        self.append_scope(&mut url);
        Ok(url)
    }

    fn key_value_get_url(&self, endpoint: &str, action: &str) -> Result<Url> {
        let mut url = Self::endpoint_url(endpoint, &format!("{KEY_VALUE_PATH}/{action}"))?;
        self.append_scope(&mut url);
        Ok(url)
    }

    fn key_value_put_url(&self, endpoint: &str) -> Result<Url> {
        let mut url = Self::endpoint_url(endpoint, KEY_VALUE_PATH)?;
        self.append_scope(&mut url);
        Ok(url)
    }

    fn health_url(endpoint: &str) -> Result<Url> {
        Self::endpoint_url(endpoint, HEALTH_PATH)
    }

    fn append_scope(&self, url: &mut Url) {
        let mut query = url.query_pairs_mut();
        query.append_pair("account_handle", self.account());
        if let Some(project) = self.config.project.as_deref() {
            query.append_pair("project_handle", project);
        }
    }

    fn server_url(&self, path: &str) -> Result<Url> {
        join_url(&self.config.url, path)
    }

    fn endpoint_url(endpoint: &str, path: &str) -> Result<Url> {
        join_url(endpoint, path)
    }
}

#[derive(Debug, Deserialize)]
struct EndpointResponse {
    endpoints: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct KeyValuePayload {
    entries: Vec<KeyValueEntry>,
}

#[derive(Debug, Deserialize)]
struct KeyValueEntry {
    value: String,
}

#[derive(Debug, Serialize)]
struct PutKeyValuePayload {
    cas_id: String,
    entries: Vec<PutKeyValueEntry>,
}

#[derive(Debug, Serialize)]
struct PutKeyValueEntry {
    value: String,
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
