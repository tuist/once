use std::time::Duration;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::super::lifecycle::{self, Lookup, Marker, MARKER_KEY};
use crate::{Error, RemoteIoLimits, Result};

const ENV_DAEMON_PORT: &str = "49983";

#[derive(Clone)]
pub(super) struct Config {
    control_url: String,
    pub(super) sandbox_url: String,
    pub(super) api_key: String,
    default_template: String,
}

impl Config {
    pub(super) fn from_env() -> Result<Self> {
        let api_key = std::env::var("STRUO_E2B_API_KEY")
            .or_else(|_| std::env::var("E2B_API_KEY"))
            .map_err(|_| Error::RemoteProviderConfig {
                provider: "e2b".to_string(),
                message: "set STRUO_E2B_API_KEY or E2B_API_KEY".to_string(),
            })?;
        Ok(Self {
            control_url: std::env::var("STRUO_E2B_API_URL")
                .unwrap_or_else(|_| "https://api.e2b.app".to_string()),
            sandbox_url: std::env::var("STRUO_E2B_SANDBOX_URL")
                .unwrap_or_else(|_| "https://sandbox.e2b.app".to_string()),
            api_key,
            default_template: std::env::var("STRUO_E2B_TEMPLATE")
                .ok()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "base".to_string()),
        })
    }

    #[cfg(test)]
    pub(super) fn for_test(control_url: String, sandbox_url: String) -> Self {
        Self {
            control_url,
            sandbox_url,
            api_key: "test-key".to_string(),
            default_template: "base".to_string(),
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
    #[serde(rename = "sandboxID")]
    pub id: String,
    #[serde(rename = "envdAccessToken")]
    pub(super) access_token: Option<String>,
}

#[derive(Serialize)]
struct CreateRequest<'a> {
    #[serde(rename = "templateID")]
    template_id: &'a str,
    timeout: u64,
    secure: bool,
    allow_internet_access: bool,
    #[serde(rename = "autoPause")]
    auto_pause: bool,
    #[serde(rename = "autoResume")]
    auto_resume: AutoResume,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    metadata: BTreeMap<&'a str, &'a str>,
}

#[derive(Deserialize)]
struct ListedSandbox {
    #[serde(rename = "sandboxID")]
    id: String,
    #[serde(default)]
    metadata: Option<BTreeMap<String, String>>,
}

#[derive(Serialize)]
struct AutoResume {
    enabled: bool,
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

    /// Creates a sandbox tagged with the lifecycle marker in its metadata.
    pub(super) async fn create_marked(
        &self,
        template: Option<&str>,
        timeout_ms: Option<u64>,
        marker: Option<&Marker>,
    ) -> Result<Sandbox> {
        let template = template.unwrap_or(&self.config.default_template);
        let timeout = lifetime(timeout_ms).as_secs();
        let metadata = marker
            .map(|marker| BTreeMap::from([(MARKER_KEY, marker.nonce.as_str())]))
            .unwrap_or_default();
        let response = self
            .http
            .post(format!(
                "{}/sandboxes",
                self.config.control_url.trim_end_matches('/')
            ))
            .header("X-API-Key", &self.config.api_key)
            .timeout(Duration::from_mins(1))
            .header("Content-Type", "application/json")
            .body(super::super::http::encode(
                &CreateRequest {
                    template_id: template,
                    timeout,
                    secure: true,
                    allow_internet_access: true,
                    auto_pause: false,
                    auto_resume: AutoResume { enabled: false },
                    metadata,
                },
                "e2b",
                self.limits.control_bytes(),
            )?)
            .send()
            .await
            .map_err(http_error)?;
        let sandbox: Sandbox =
            super::super::http::decode(response, "e2b", self.limits.control_bytes(), self.limits)
                .await?;
        tracing::debug!(provider = "e2b", sandbox = %sandbox.id, "created remote sandbox");
        Ok(sandbox)
    }

    /// Lists running or paused sandboxes whose metadata carries `nonce`.
    pub(super) async fn find_marked(&self, nonce: &str) -> Result<Lookup> {
        let mut url = reqwest::Url::parse(&format!(
            "{}/v2/sandboxes",
            self.config.control_url.trim_end_matches('/')
        ))
        .map_err(|_| api_error("invalid control URL".to_string()))?;
        url.query_pairs_mut()
            .append_pair("metadata", &format!("{MARKER_KEY}={nonce}"))
            .append_pair("state", "running,paused")
            .append_pair("limit", "100");
        let response = self
            .http
            .get(url)
            .header("X-API-Key", &self.config.api_key)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(http_error)?;
        let truncated = response
            .headers()
            .get("x-next-token")
            .is_some_and(|token| !token.is_empty());
        let listed: Vec<ListedSandbox> =
            super::super::http::decode(response, "e2b", self.limits.control_bytes(), self.limits)
                .await?;
        Ok(lifecycle::classify(
            listed.iter().map(|sandbox| {
                let marker = sandbox
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get(MARKER_KEY))
                    .map(String::as_str);
                (sandbox.id.as_str(), marker, true)
            }),
            nonce,
            truncated,
        ))
    }

    pub(super) fn scope(&self) -> String {
        lifecycle::scope("e2b", &self.config.control_url)
    }

    pub(super) fn credential(&self) -> String {
        lifecycle::credential("e2b", &self.config.api_key)
    }

    pub(super) async fn delete(&self, id: &str) -> Result<()> {
        let response = self
            .http
            .delete(format!(
                "{}/sandboxes/{id}",
                self.config.control_url.trim_end_matches('/')
            ))
            .header("X-API-Key", &self.config.api_key)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(http_error)?;
        if response.status() != reqwest::StatusCode::NOT_FOUND {
            expect_success(response)?;
        }
        tracing::debug!(provider = "e2b", sandbox = id, "deleted remote sandbox");
        Ok(())
    }

    pub(super) fn sandbox_request(
        &self,
        method: reqwest::Method,
        sandbox: &Sandbox,
        path: &str,
    ) -> reqwest::RequestBuilder {
        let mut request = self
            .http
            .request(
                method,
                format!("{}{}", self.config.sandbox_url.trim_end_matches('/'), path),
            )
            .header("E2b-Sandbox-Id", &sandbox.id)
            .header("E2b-Sandbox-Port", ENV_DAEMON_PORT);
        if let Some(token) = &sandbox.access_token {
            request = request.header("X-Access-Token", token);
        }
        request
    }
}

/// The provider-enforced sandbox lifetime requested at creation.
pub(super) fn lifetime(timeout_ms: Option<u64>) -> Duration {
    Duration::from_secs(
        timeout_ms
            .map_or(600, |value| value.div_ceil(1000).saturating_add(120))
            .max(1),
    )
}

pub(super) fn checked(response: reqwest::Response) -> Result<reqwest::Response> {
    super::super::http::checked(response, "e2b")
}

pub(super) fn expect_success(response: reqwest::Response) -> Result<()> {
    checked(response).map(|_| ())
}

pub(super) fn http_error(source: reqwest::Error) -> Error {
    super::super::http::http_error("e2b", source)
}

pub(super) fn api_error(message: String) -> Error {
    Error::RemoteProviderApi {
        provider: "e2b".to_string(),
        message,
    }
}
