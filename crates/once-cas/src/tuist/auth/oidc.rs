//! Continuous Integration authentication via `OpenID Connect`. On CI,
//! Once obtains a token from the supported provider and exchanges it with
//! Tuist for a short-lived access token, caching the result in memory for
//! the life of the process.

use std::thread;
use std::time::Duration;

use reqwest::{blocking::Client, StatusCode, Url};
use schlussel::{SessionStorage, Token};
use serde::{Deserialize, Serialize};

use super::{env_token, TuistAuth, OIDC_TOKEN_REFRESH_WINDOW_SECONDS, PROVIDER_NAME};
use crate::{Error, Result};

const OIDC_AUDIENCE: &str = "tuist";
const OIDC_REQUEST_TIMEOUT_SECONDS: u64 = 30;
const OIDC_EXCHANGE_MAX_RETRIES: usize = 3;
const OIDC_EXCHANGE_RETRY_BASE_DELAY_MS: u64 = 100;

impl TuistAuth {
    pub(super) fn login_with_ci_oidc(&self) -> Result<()> {
        self.exchange_and_store_ci_oidc_token().map(|_| ())
    }

    pub(super) fn exchange_and_store_ci_oidc_token(&self) -> Result<Token> {
        let _guard = self
            .ci_exchange_lock
            .lock()
            .map_err(|source| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "lock Continuous Integration auth token exchange",
                message: source.to_string(),
            })?;
        if let Some(token) = self.cached_ci_token()? {
            return Ok(token);
        }
        if let Ok(token) = self.load_valid_token() {
            self.remember_ci_token(&token)?;
            return Ok(token);
        }
        let oidc_token = Self::fetch_ci_oidc_token()?;
        let access_token = self.exchange_oidc_token(&oidc_token)?;
        self.store_access_token(&access_token)?;
        self.remember_ci_token(&access_token)?;
        Ok(access_token)
    }

    fn fetch_ci_oidc_token() -> Result<String> {
        if env_is_truthy("GITHUB_ACTIONS") {
            let request_url = env_token("ACTIONS_ID_TOKEN_REQUEST_URL").ok_or_else(|| {
                Error::InvalidConfig {
                    provider: PROVIDER_NAME,
                    message: "GitHub Actions OpenID Connect token request variables are not set. Set `permissions: id-token: write` in the workflow.".to_string(),
                }
            })?;
            let request_token =
                env_token("ACTIONS_ID_TOKEN_REQUEST_TOKEN").ok_or_else(|| Error::InvalidConfig {
                    provider: PROVIDER_NAME,
                    message: "GitHub Actions OpenID Connect token request variables are not set. Set `permissions: id-token: write` in the workflow.".to_string(),
                })?;
            return Self::fetch_github_actions_oidc_token(&request_url, &request_token);
        }

        if env_is_truthy("CIRCLECI") {
            return env_token("CIRCLE_OIDC_TOKEN_V2")
                .or_else(|| env_token("CIRCLE_OIDC_TOKEN"))
                .ok_or_else(|| Error::InvalidConfig {
                    provider: PROVIDER_NAME,
                    message: "CircleCI OpenID Connect token was not found. Enable OpenID Connect for the CircleCI project.".to_string(),
                });
        }

        if env_is_truthy("BITRISE_IO") {
            return env_token("BITRISE_OIDC_ID_TOKEN")
                .or_else(|| env_token("BITRISE_IDENTITY_TOKEN"))
                .ok_or_else(|| Error::InvalidConfig {
                    provider: PROVIDER_NAME,
                    message: "Bitrise OpenID Connect token was not found. Add the Bitrise identity token step before this step.".to_string(),
                });
        }

        Err(Error::InvalidConfig {
            provider: PROVIDER_NAME,
            message: "OpenID Connect authentication is not supported in this environment. Supported Continuous Integration providers: GitHub Actions, CircleCI, Bitrise.".to_string(),
        })
    }

    fn fetch_github_actions_oidc_token(request_url: &str, request_token: &str) -> Result<String> {
        let mut url = Url::parse(request_url).map_err(|source| Error::InvalidConfig {
            provider: PROVIDER_NAME,
            message: format!(
                "invalid GitHub Actions OpenID Connect token request URL `{request_url}`: {source}"
            ),
        })?;
        url.query_pairs_mut().append_pair("audience", OIDC_AUDIENCE);
        let response = Self::oidc_http_client()?
            .get(url)
            .bearer_auth(request_token)
            .send()
            .map_err(|source| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "fetch GitHub Actions OpenID Connect token",
                message: source.to_string(),
            })?;
        let status = response.status();
        let body = response.text().map_err(|source| Error::Remote {
            provider: PROVIDER_NAME,
            operation: "fetch GitHub Actions OpenID Connect token",
            message: source.to_string(),
        })?;
        if !status.is_success() {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "fetch GitHub Actions OpenID Connect token",
                message: status_body_message(status, &body),
            });
        }

        let token_response: OidcTokenResponse =
            serde_json::from_str(&body).map_err(|source| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "decode GitHub Actions OpenID Connect token",
                message: source.to_string(),
            })?;
        let token = token_response.value.trim();
        if token.is_empty() {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "decode GitHub Actions OpenID Connect token",
                message: "response did not include a token".to_string(),
            });
        }
        Ok(token.to_string())
    }

    fn exchange_oidc_token(&self, oidc_token: &str) -> Result<Token> {
        let client = Self::oidc_http_client()?;
        let endpoint = self.oidc_exchange_endpoint()?;

        for retry in 0..=OIDC_EXCHANGE_MAX_RETRIES {
            match Self::exchange_oidc_token_once(&client, endpoint.clone(), oidc_token) {
                Ok(token) => return Ok(token),
                Err(error) if retry < OIDC_EXCHANGE_MAX_RETRIES && error.retryable => {
                    thread::sleep(oidc_exchange_retry_delay(retry));
                }
                Err(error) => return Err(error.into_remote_error()),
            }
        }

        unreachable!("OpenID Connect exchange retry loop must return")
    }

    fn exchange_oidc_token_once(
        client: &Client,
        endpoint: Url,
        oidc_token: &str,
    ) -> std::result::Result<Token, OidcExchangeAttemptError> {
        let response = client
            .post(endpoint)
            .json(&OidcExchangeRequest { token: oidc_token })
            .send()
            .map_err(|source| OidcExchangeAttemptError {
                message: source.to_string(),
                retryable: true,
            })?;
        let status = response.status();
        let body = response.text().map_err(|source| OidcExchangeAttemptError {
            message: source.to_string(),
            retryable: oidc_status_is_retryable(status),
        })?;
        if !status.is_success() {
            return Err(OidcExchangeAttemptError {
                message: status_body_message(status, &body),
                retryable: oidc_status_is_retryable(status),
            });
        }

        let response: OidcExchangeResponse =
            serde_json::from_str(&body).map_err(|source| OidcExchangeAttemptError {
                message: source.to_string(),
                retryable: false,
            })?;
        let access_token = response.access_token.trim();
        if access_token.is_empty() {
            return Err(OidcExchangeAttemptError {
                message: "response did not include an access token".to_string(),
                retryable: false,
            });
        }

        Ok(Token::new(access_token, "Bearer").with_expiration(response.expires_in))
    }

    pub(super) fn cached_ci_access_token(&self) -> Result<Option<String>> {
        Ok(self.cached_ci_token()?.map(|token| token.access_token))
    }

    fn cached_ci_token(&self) -> Result<Option<Token>> {
        let mut cached = self
            .cached_ci_token
            .lock()
            .map_err(|source| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "load cached Continuous Integration auth token",
                message: source.to_string(),
            })?;
        let Some(token) = cached.as_ref() else {
            return Ok(None);
        };
        if !token.expires_within(OIDC_TOKEN_REFRESH_WINDOW_SECONDS) {
            return Ok(Some(token.clone()));
        }
        *cached = None;
        Ok(None)
    }

    fn remember_ci_token(&self, token: &Token) -> Result<()> {
        let mut cached = self
            .cached_ci_token
            .lock()
            .map_err(|source| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "store cached Continuous Integration auth token",
                message: source.to_string(),
            })?;
        *cached = Some(token.clone());
        Ok(())
    }

    fn store_access_token(&self, token: &Token) -> Result<()> {
        self.storage()?
            .save(&self.storage_key(), token)
            .map_err(|source| Self::remote_auth_error("store auth token", &source))
    }

    fn oidc_http_client() -> Result<Client> {
        Client::builder()
            .timeout(Duration::from_secs(OIDC_REQUEST_TIMEOUT_SECONDS))
            .build()
            .map_err(|source| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "configure OpenID Connect client",
                message: source.to_string(),
            })
    }
}

#[derive(Debug, Deserialize)]
struct OidcTokenResponse {
    value: String,
}

#[derive(Debug, Serialize)]
struct OidcExchangeRequest<'a> {
    token: &'a str,
}

#[derive(Debug, Deserialize)]
struct OidcExchangeResponse {
    access_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Debug)]
struct OidcExchangeAttemptError {
    message: String,
    retryable: bool,
}

impl OidcExchangeAttemptError {
    fn into_remote_error(self) -> Error {
        Error::Remote {
            provider: PROVIDER_NAME,
            operation: "exchange OpenID Connect token",
            message: self.message,
        }
    }
}

fn env_is_truthy(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

fn status_body_message(status: StatusCode, body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        format!("HTTP {}", status.as_u16())
    } else {
        format!("HTTP {}: {body}", status.as_u16())
    }
}

fn oidc_status_is_retryable(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

fn oidc_exchange_retry_delay(retry: usize) -> Duration {
    let multiplier = (0..retry).fold(1_u64, |multiplier, _| multiplier.saturating_mul(2));
    Duration::from_millis(OIDC_EXCHANGE_RETRY_BASE_DELAY_MS.saturating_mul(multiplier))
}
