//! Reaching the remote tier: endpoint discovery and its on-disk memo, the
//! fastest-endpoint probe, gRPC channels, and authorized requests.

use super::*;

impl TuistCache {
    pub(super) async fn data_plane_endpoint(&self) -> Result<String> {
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
    /// across Struo processes until the `max-age` the server sends expires, so
    /// most commands never ask. A request that cannot reach Tuist falls back to
    /// the last answer, even an expired one, and otherwise reports the
    /// remote tier as unavailable.
    pub(super) async fn fetch_endpoints(&self) -> Result<Vec<String>> {
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

    pub(super) async fn request_endpoints(&self) -> Result<(Vec<String>, Option<Duration>)> {
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

    pub(super) fn persisted_endpoints_path(&self) -> std::path::PathBuf {
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

    pub(super) async fn persisted_endpoints(&self) -> Option<PersistedEndpoints> {
        let bytes = tokio::fs::read(self.persisted_endpoints_path())
            .await
            .ok()?;
        serde_json::from_slice::<PersistedEndpoints>(&bytes)
            .ok()
            .filter(|persisted| !persisted.endpoints.is_empty())
    }

    /// Best effort: failing to persist only means the next command asks again.
    pub(super) async fn persist_endpoints(&self, endpoints: &[String], max_age: Option<Duration>) {
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

    pub(super) async fn pick_fastest_endpoint(&self, endpoints: &[String]) -> Result<String> {
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

    pub(super) async fn grpc_channel(&self) -> Result<Channel> {
        Ok(self.grpc_channels().await?.requests)
    }

    pub(super) async fn grpc_upload_channel(&self) -> Result<Channel> {
        Ok(self.grpc_channels().await?.uploads)
    }

    /// Connect once, then reuse the channels. A failed discovery or connection
    /// is retried after a cooldown instead of disabling the remote tier for
    /// the rest of the process.
    pub(super) async fn grpc_channels(&self) -> Result<GrpcChannels> {
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
    pub(super) async fn auth_token(&self) -> Result<&str> {
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

    pub(super) async fn authorized_request(
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

    pub(super) async fn authorized_grpc_request<T>(
        &self,
        message: T,
        operation: &'static str,
    ) -> Result<Request<T>> {
        let refreshed = self
            .refreshed_token
            .lock()
            .expect("refreshed token lock is not poisoned")
            .clone();
        let token = match refreshed {
            Some(token) => token,
            None => self.auth_token().await?.to_string(),
        };
        authorized_grpc_request_with_token(message, &token, Some(self.account()))
            .map_err(|message| status::remote(operation, ErrorKind::Misconfigured, message))
    }

    /// Resolve the credential again, refreshing a stored session that is
    /// about to expire, after the service rejected the one in use.
    pub(super) async fn refresh_auth_token(&self) -> Result<()> {
        let auth = self.auth.clone();
        let token = tokio::task::spawn_blocking(move || auth.token())
            .await
            .map_err(|source| {
                status::remote("refresh auth token", ErrorKind::Other, source.to_string())
            })??;
        *self
            .refreshed_token
            .lock()
            .expect("refreshed token lock is not poisoned") = Some(token);
        Ok(())
    }

    pub(super) fn account(&self) -> &str {
        self.config
            .account
            .as_deref()
            .expect("validated at construction")
    }

    pub(super) fn instance_name(&self) -> String {
        let Some(project) = self.config.project.as_deref() else {
            return "default".to_string();
        };
        project.to_string()
    }

    pub(super) fn endpoints_url(&self) -> Result<Url> {
        let mut url = self.server_url(ENDPOINTS_PATH)?;
        url.query_pairs_mut()
            .append_pair("account_handle", self.account());
        Ok(url)
    }

    pub(super) fn server_url(&self, path: &str) -> Result<Url> {
        join_url(&self.config.url, path)
    }
}

/// How long discovered endpoints are reused when the server sends no
/// `Cache-Control: max-age`, matching the Tuist CLI and Gradle plugin.
pub(super) const DEFAULT_ENDPOINTS_MAX_AGE: Duration = Duration::from_hours(1);

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct PersistedEndpoints {
    endpoints: Vec<String>,
    expires_at_unix_ms: u64,
}

impl PersistedEndpoints {
    fn is_fresh(&self) -> bool {
        unix_ms_now() < self.expires_at_unix_ms
    }
}

pub(super) fn unix_ms_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, duration_ms)
}

pub(super) fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Reads `max-age` from `Cache-Control`, ignoring the other directives.
pub(super) fn cache_control_max_age(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
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
pub(super) struct EndpointResponse {
    endpoints: Vec<String>,
}

/// Failures are described without the transport's error text, which can
/// include the full endpoint URI.
pub(super) async fn connect_grpc_endpoint(endpoint: &str) -> std::result::Result<Channel, String> {
    grpc_endpoint(endpoint)?
        .connect()
        .await
        .map_err(|_| "the connection failed".to_string())
}

pub(super) fn grpc_endpoint(endpoint: &str) -> std::result::Result<Endpoint, String> {
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

pub(super) fn authorized_grpc_request_with_token<T>(
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

pub(in crate::tuist) fn grpc_endpoint_url(endpoint: &str) -> String {
    if let Some(rest) = endpoint.strip_prefix("grpc://") {
        format!("http://{rest}")
    } else if let Some(rest) = endpoint.strip_prefix("grpcs://") {
        format!("https://{rest}")
    } else {
        endpoint.to_string()
    }
}
