use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{Error, ResourceRequest, Result};

#[derive(Clone)]
pub(super) struct Config {
    control_url: String,
    pub(super) toolbox_url: String,
    pub(super) api_key: String,
}

impl Config {
    pub(super) fn from_env() -> Result<Self> {
        Ok(Self {
            control_url: std::env::var("ONCE_DAYTONA_CONTROL_URL")
                .unwrap_or_else(|_| "https://app.daytona.io/api".to_string()),
            toolbox_url: std::env::var("ONCE_DAYTONA_TOOLBOX_URL")
                .or_else(|_| std::env::var("ONCE_DAYTONA_API_URL"))
                .unwrap_or_else(|_| "https://proxy.app.daytona.io/toolbox".to_string()),
            api_key: std::env::var("ONCE_DAYTONA_API_KEY")
                .or_else(|_| std::env::var("DAYTONA_API_KEY"))
                .map_err(|_| Error::RemoteProviderConfig {
                    provider: "daytona".to_string(),
                    message: "set ONCE_DAYTONA_API_KEY or DAYTONA_API_KEY".to_string(),
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
            .build()
            .map_err(http_error)?;
        Ok(Self { http, config })
    }

    pub(super) async fn create(
        &self,
        image: Option<&str>,
        resources: &ResourceRequest,
        timeout_ms: Option<u64>,
    ) -> Result<Sandbox> {
        let auto_stop_interval = timeout_ms
            .map_or(15, |value| value.div_ceil(60_000).saturating_add(5))
            .max(1);
        let memory = (resources.memory_bytes > 0)
            .then(|| resources.memory_bytes.div_ceil(1024 * 1024 * 1024));
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
            .json(&CreateRequest {
                auto_delete_interval: 0,
                auto_stop_interval,
                cpu: resources.cpu_slots.max(1),
                memory,
                build_info,
            })
            .send()
            .await
            .map_err(http_error)?;
        let mut sandbox: Sandbox = decode(response).await?;
        tracing::debug!(provider = "daytona", sandbox = %sandbox.id, "created remote sandbox");
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
        decode(response).await
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
        expect_success(response).await?;
        tracing::debug!(provider = "daytona", sandbox = id, "deleted remote sandbox");
        Ok(())
    }

    pub(super) fn toolbox_base(&self, sandbox: &Sandbox) -> String {
        let root = sandbox
            .toolbox_proxy_url
            .as_deref()
            .unwrap_or(&self.config.toolbox_url);
        format!("{}/{}", root.trim_end_matches('/'), sandbox.id)
    }
}

fn timeout_secs(timeout_ms: Option<u64>) -> u64 {
    timeout_ms
        .map_or(600, |value| value.div_ceil(1000).saturating_add(120))
        .max(1)
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
        provider: "daytona".to_string(),
        source,
    }
}

fn api_error(message: String) -> Error {
    Error::RemoteProviderApi {
        provider: "daytona".to_string(),
        message,
    }
}
