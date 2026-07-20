use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

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
        let api_key = std::env::var("ONCE_E2B_API_KEY")
            .or_else(|_| std::env::var("E2B_API_KEY"))
            .map_err(|_| Error::RemoteProviderConfig {
                provider: "e2b".to_string(),
                message: "set ONCE_E2B_API_KEY or E2B_API_KEY".to_string(),
            })?;
        Ok(Self {
            control_url: std::env::var("ONCE_E2B_API_URL")
                .unwrap_or_else(|_| "https://api.e2b.app".to_string()),
            sandbox_url: std::env::var("ONCE_E2B_SANDBOX_URL")
                .unwrap_or_else(|_| "https://sandbox.e2b.app".to_string()),
            api_key,
            default_template: std::env::var("ONCE_E2B_TEMPLATE")
                .unwrap_or_else(|_| "base".to_string()),
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
}

#[derive(Serialize)]
struct AutoResume {
    enabled: bool,
}

impl Client {
    pub(super) fn new(config: Config) -> Result<Self> {
        let http = reqwest::Client::builder()
            .pool_max_idle_per_host(0)
            .build()
            .map_err(http_error)?;
        Ok(Self { http, config })
    }

    pub(super) async fn create(
        &self,
        template: Option<&str>,
        timeout_ms: Option<u64>,
    ) -> Result<Sandbox> {
        let template = template.unwrap_or(&self.config.default_template);
        let timeout = timeout_ms
            .map_or(600, |value| value.div_ceil(1000).saturating_add(120))
            .max(1);
        let response = self
            .http
            .post(format!(
                "{}/sandboxes",
                self.config.control_url.trim_end_matches('/')
            ))
            .header("X-API-Key", &self.config.api_key)
            .timeout(Duration::from_mins(1))
            .json(&CreateRequest {
                template_id: template,
                timeout,
                secure: true,
                allow_internet_access: true,
                auto_pause: false,
                auto_resume: AutoResume { enabled: false },
            })
            .send()
            .await
            .map_err(http_error)?;
        let sandbox: Sandbox = decode(response).await?;
        tracing::debug!(provider = "e2b", sandbox = %sandbox.id, "created remote sandbox");
        Ok(sandbox)
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
        expect_success(response).await?;
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

async fn decode<T: for<'de> Deserialize<'de>>(response: reqwest::Response) -> Result<T> {
    checked(response).await?.json().await.map_err(http_error)
}

pub(super) async fn checked(response: reqwest::Response) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Err(api_error(format!("HTTP {status}: {body}")))
}

pub(super) async fn expect_success(response: reqwest::Response) -> Result<()> {
    checked(response).await.map(|_| ())
}

pub(super) fn http_error(source: reqwest::Error) -> Error {
    Error::RemoteProviderHttp {
        provider: "e2b".to_string(),
        source,
    }
}

pub(super) fn api_error(message: String) -> Error {
    Error::RemoteProviderApi {
        provider: "e2b".to_string(),
        message,
    }
}
