use std::time::{Duration, Instant};

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::super::lifecycle::{self, Lookup, Marker, MARKER_KEY};
use crate::{Error, RemoteIoLimits, ResourceRequest, Result};

#[derive(Clone)]
pub(super) struct Config {
    control_url: String,
    pub(super) toolbox_url: String,
    pub(super) api_key: String,
}

impl Config {
    pub(super) fn from_env() -> Result<Self> {
        Ok(Self {
            control_url: std::env::var("STRUO_DAYTONA_CONTROL_URL")
                .unwrap_or_else(|_| "https://app.daytona.io/api".to_string()),
            toolbox_url: std::env::var("STRUO_DAYTONA_TOOLBOX_URL")
                .or_else(|_| std::env::var("STRUO_DAYTONA_API_URL"))
                .unwrap_or_else(|_| "https://proxy.app.daytona.io/toolbox".to_string()),
            api_key: std::env::var("STRUO_DAYTONA_API_KEY")
                .or_else(|_| std::env::var("DAYTONA_API_KEY"))
                .map_err(|_| Error::RemoteProviderConfig {
                    provider: "daytona".to_string(),
                    message: "set STRUO_DAYTONA_API_KEY or DAYTONA_API_KEY".to_string(),
                })?,
        })
    }

    #[cfg(test)]
    pub(super) fn for_test(control_url: String, toolbox_url: String) -> Self {
        Self {
            control_url,
            toolbox_url,
            api_key: "test-key".to_string(),
        }
    }
}

#[derive(Clone)]
pub(super) struct Client {
    pub(super) http: reqwest::Client,
    pub(super) config: Config,
    pub(super) limits: RemoteIoLimits,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Sandbox {
    pub id: String,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    toolbox_proxy_url: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateRequest {
    auto_delete_interval: i32,
    auto_stop_interval: u64,
    cpu: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    memory: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    build_info: Option<BuildInfo>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    labels: BTreeMap<&'static str, String>,
}

#[derive(Deserialize)]
struct ListedSandbox {
    id: String,
    #[serde(default)]
    labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    state: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BuildInfo {
    dockerfile_content: String,
}

impl Client {
    pub(super) fn new(config: Config) -> Result<Self> {
        let http = reqwest::Client::builder()
            .pool_max_idle_per_host(0)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(http_error)?;
        Ok(Self {
            http,
            config,
            limits: RemoteIoLimits::default(),
        })
    }

    /// Creates a sandbox tagged with the lifecycle marker in its labels.
    pub(super) async fn create_marked(
        &self,
        image: Option<&str>,
        resources: &ResourceRequest,
        timeout_ms: Option<u64>,
        marker: Option<&Marker>,
    ) -> Result<Sandbox> {
        let auto_stop_interval = auto_stop_minutes(timeout_ms);
        let labels = marker
            .map(|marker| BTreeMap::from([(MARKER_KEY, marker.nonce.clone())]))
            .unwrap_or_default();
        let memory = (resources.memory_bytes > 0)
            .then(|| resources.memory_bytes.div_ceil(1024 * 1024 * 1024));
        if let Some(image) = image {
            super::super::http::check_size(
                u64::try_from(image.len()).unwrap_or(u64::MAX),
                self.limits.control_bytes(),
                "daytona",
                "request metadata bytes",
            )?;
        }
        let build_info = image.map(|image| BuildInfo {
            dockerfile_content: format!("FROM {image}"),
        });
        let response = self
            .http
            .post(format!(
                "{}/sandbox",
                self.config.control_url.trim_end_matches('/')
            ))
            .bearer_auth(&self.config.api_key)
            .timeout(Duration::from_mins(1))
            .header("Content-Type", "application/json")
            .body(super::super::http::encode(
                &CreateRequest {
                    auto_delete_interval: 0,
                    auto_stop_interval,
                    cpu: resources.cpu_slots.max(1),
                    memory,
                    build_info,
                    labels,
                },
                "daytona",
                self.limits.control_bytes(),
            )?)
            .send()
            .await
            .map_err(http_error)?;
        let sandbox: Sandbox = super::super::http::decode(
            response,
            "daytona",
            self.limits.control_bytes(),
            self.limits,
        )
        .await?;
        tracing::debug!(provider = "daytona", sandbox = %sandbox.id, "created remote sandbox");
        Ok(sandbox)
    }

    /// Polls a created sandbox until it starts. Callers register cleanup
    /// before waiting so a failed or cancelled startup still deletes it.
    pub(super) async fn wait_started(
        &self,
        mut sandbox: Sandbox,
        timeout_ms: Option<u64>,
    ) -> Result<Sandbox> {
        let deadline = Instant::now() + Duration::from_secs(timeout_secs(timeout_ms));
        while sandbox.state.as_deref() != Some("started") {
            if matches!(
                sandbox.state.as_deref(),
                Some("error" | "build_failed" | "destroyed")
            ) {
                return Err(api_error(format!(
                    "sandbox {} entered state {} while starting",
                    sandbox.id,
                    sandbox.state.as_deref().unwrap_or("unknown")
                )));
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout(Duration::from_secs(timeout_secs(
                    timeout_ms,
                ))));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
            sandbox = self.get(&sandbox.id).await?;
        }
        Ok(sandbox)
    }

    async fn get(&self, id: &str) -> Result<Sandbox> {
        let response = self
            .http
            .get(format!(
                "{}/sandbox/{id}",
                self.config.control_url.trim_end_matches('/')
            ))
            .bearer_auth(&self.config.api_key)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(http_error)?;
        super::super::http::decode(
            response,
            "daytona",
            self.limits.control_bytes(),
            self.limits,
        )
        .await
    }

    /// Lists sandboxes whose labels carry `nonce`. Destroyed sandboxes count
    /// as gone.
    pub(super) async fn find_marked(&self, nonce: &str) -> Result<Lookup> {
        let labels = serde_json::json!({ MARKER_KEY: nonce }).to_string();
        let mut url = reqwest::Url::parse(&format!(
            "{}/sandbox",
            self.config.control_url.trim_end_matches('/')
        ))
        .map_err(|_| api_error("invalid control URL".to_string()))?;
        url.query_pairs_mut().append_pair("labels", &labels);
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.config.api_key)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(http_error)?;
        let listed: Vec<ListedSandbox> = super::super::http::decode(
            response,
            "daytona",
            self.limits.control_bytes(),
            self.limits,
        )
        .await?;
        Ok(lifecycle::classify(
            listed.iter().map(|sandbox| {
                let marker = sandbox
                    .labels
                    .as_ref()
                    .and_then(|labels| labels.get(MARKER_KEY))
                    .map(String::as_str);
                let live = sandbox.state.as_deref() != Some("destroyed");
                (sandbox.id.as_str(), marker, live)
            }),
            nonce,
            false,
        ))
    }

    pub(super) fn scope(&self) -> String {
        lifecycle::scope("daytona", &self.config.control_url)
    }

    pub(super) fn credential(&self) -> String {
        lifecycle::credential("daytona", &self.config.api_key)
    }

    pub(super) async fn delete(&self, id: &str) -> Result<()> {
        let response = self
            .http
            .delete(format!(
                "{}/sandbox/{id}",
                self.config.control_url.trim_end_matches('/')
            ))
            .bearer_auth(&self.config.api_key)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(http_error)?;
        if response.status() != reqwest::StatusCode::NOT_FOUND {
            expect_success(response)?;
        }
        tracing::debug!(provider = "daytona", sandbox = id, "deleted remote sandbox");
        Ok(())
    }

    pub(super) fn toolbox_base(&self, sandbox: &Sandbox) -> Result<String> {
        let root = sandbox
            .toolbox_proxy_url
            .as_deref()
            .unwrap_or(&self.config.toolbox_url);
        let candidate = reqwest::Url::parse(root).map_err(|_| Self::untrusted_toolbox())?;
        let trusted = [&self.config.control_url, &self.config.toolbox_url]
            .iter()
            .any(|url| {
                reqwest::Url::parse(url).is_ok_and(|trusted| trusted.origin() == candidate.origin())
            });
        if !trusted
            || !candidate.username().is_empty()
            || candidate.password().is_some()
            || candidate.query().is_some()
            || candidate.fragment().is_some()
        {
            return Err(Self::untrusted_toolbox());
        }
        Ok(format!("{}/{}", root.trim_end_matches('/'), sandbox.id))
    }

    fn untrusted_toolbox() -> Error {
        Error::RemoteProviderConfig {
            provider: "daytona".to_string(),
            message: "toolbox proxy must use a configured control or toolbox origin without user-info, query, or fragment".to_string(),
        }
    }
}

fn auto_stop_minutes(timeout_ms: Option<u64>) -> u64 {
    timeout_ms
        .map_or(15, |value| value.div_ceil(60_000).saturating_add(5))
        .max(1)
}

/// How long a created sandbox may take to start and then idle until the
/// provider stops and, with a zero auto-delete interval, deletes it.
pub(super) fn lifetime(timeout_ms: Option<u64>) -> Duration {
    Duration::from_secs(
        timeout_secs(timeout_ms).saturating_add(auto_stop_minutes(timeout_ms).saturating_mul(60)),
    )
}

fn timeout_secs(timeout_ms: Option<u64>) -> u64 {
    timeout_ms
        .map_or(600, |value| value.div_ceil(1000).saturating_add(120))
        .max(1)
}

pub(super) fn checked(response: reqwest::Response) -> Result<reqwest::Response> {
    super::super::http::checked(response, "daytona")
}

pub(super) fn expect_success(response: reqwest::Response) -> Result<()> {
    checked(response).map(|_| ())
}

pub(super) fn http_error(source: reqwest::Error) -> Error {
    super::super::http::http_error("daytona", source)
}

fn api_error(message: String) -> Error {
    Error::RemoteProviderApi {
        provider: "daytona".to_string(),
        message,
    }
}
