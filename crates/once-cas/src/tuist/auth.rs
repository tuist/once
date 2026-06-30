use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::{blocking::Client, StatusCode, Url};
use schlussel::callback::{build_authorization_url, open_browser};
use schlussel::{
    build_storage_key, CallbackServer, ClientMetadata, DynamicRegistrationClient, FileStorage,
    OAuthClient, OAuthConfig, PkcePair, SchlusselError, SessionStorage, Token, TokenRefresher,
};
use serde::{Deserialize, Serialize};

use super::{env_token, join_url, TuistCacheConfig, PROVIDER_NAME};
use crate::{Digest, Error, Result};

/// Public OAuth client id for Once's built-in Tuist app. This is not a secret.
pub const TUIST_APP_OAUTH_CLIENT_ID: &str = "b3298a92-3deb-4f5e-a526-b7ad324979b5";
pub const TUIST_OAUTH_CLIENT_ID_ENV: &str = "TUIST_OAUTH_CLIENT_ID";
const TUIST_TOKEN_ENV: &str = "TUIST_TOKEN";

const REGISTRATION_PATH: &str = "oauth2/register";
const TOKEN_PATH: &str = "oauth2/token";
const AUTHORIZATION_PATH: &str = "oauth2/authorize";
const OIDC_EXCHANGE_PATH: &str = "api/auth/oidc/token";
const OIDC_AUDIENCE: &str = "tuist";
const OIDC_REQUEST_TIMEOUT_SECONDS: u64 = 30;
const CI_ENVIRONMENT_VARIABLES: &[&str] = &["GITHUB_RUN_ID", "CI", "BUILD_NUMBER"];

#[derive(Debug, Clone)]
pub struct TuistAuth {
    credentials_root: PathBuf,
    config: TuistCacheConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuistAuthPrompt {
    pub authorize_url: String,
    pub redirect_uri: String,
    pub opens_browser: bool,
}

impl TuistAuth {
    pub fn new(credentials_root: impl AsRef<Path>, config: &TuistCacheConfig) -> Self {
        Self {
            credentials_root: credentials_root.as_ref().to_path_buf(),
            config: config.clone(),
        }
    }

    pub fn token(&self) -> Result<String> {
        self.token_with_env(env_token(TUIST_TOKEN_ENV))
    }

    fn token_with_env(&self, env_token: Option<String>) -> Result<String> {
        if let Some(token) = env_token {
            return Ok(token);
        }

        Ok(self.load_valid_token()?.access_token)
    }

    pub fn login(&self, open_browser_after_prompt: bool) -> Result<()> {
        let mut handler = |prompt| print_prompt(&prompt);
        self.login_with_handler(open_browser_after_prompt, &mut handler)
    }

    pub fn login_with_handler(
        &self,
        open_browser_after_prompt: bool,
        handler: &mut dyn FnMut(TuistAuthPrompt),
    ) -> Result<()> {
        if is_ci_environment() {
            return self.login_with_ci_oidc();
        }

        if let Some(client_id) = self.config.oauth_client_id.as_deref() {
            return self.login_with_client_id(client_id, open_browser_after_prompt, handler);
        }

        let server = CallbackServer::new(0)
            .map_err(|source| Self::remote_auth_error("start callback", &source))?;
        let redirect_uri = server.callback_url();
        let registered_client = self.register_client(&redirect_uri)?;
        self.save_registered_client(&registered_client)?;
        self.authorize(
            &registered_client.client_id,
            &redirect_uri,
            &server,
            open_browser_after_prompt,
            handler,
        )
    }

    pub fn logout(&self) -> Result<bool> {
        let key = self.storage_key();
        let storage = self.storage()?;
        let existed = storage
            .load(&key)
            .map_err(|source| Self::remote_auth_error("load auth token", &source))?
            .is_some();
        storage
            .delete(&key)
            .map_err(|source| Self::remote_auth_error("delete auth token", &source))?;
        self.delete_registered_client()?;
        Ok(existed)
    }

    fn load_valid_token(&self) -> Result<Token> {
        let key = self.storage_key();
        let storage = self.storage()?;
        let client = self.oauth_client(
            storage,
            self.resolve_client_id()?,
            "http://127.0.0.1/callback".to_string(),
        )?;
        let refresher = TokenRefresher::new(client)
            .with_file_locking("once")
            .map_err(|source| Self::remote_auth_error("configure auth refresh", &source))?;
        refresher
            .get_valid_token(&key)
            .map_err(|source| self.cached_auth_error(source))
    }

    fn login_with_client_id(
        &self,
        client_id: &str,
        open_browser_after_prompt: bool,
        handler: &mut dyn FnMut(TuistAuthPrompt),
    ) -> Result<()> {
        let server = CallbackServer::new(0)
            .map_err(|source| Self::remote_auth_error("start callback", &source))?;
        let redirect_uri = server.callback_url();
        self.authorize(
            client_id,
            &redirect_uri,
            &server,
            open_browser_after_prompt,
            handler,
        )
    }

    fn authorize(
        &self,
        client_id: &str,
        redirect_uri: &str,
        server: &CallbackServer,
        open_browser_after_prompt: bool,
        handler: &mut dyn FnMut(TuistAuthPrompt),
    ) -> Result<()> {
        let pkce = PkcePair::generate();
        let state = random_state();
        let authorize_url = build_authorization_url(
            &self.authorization_endpoint()?,
            client_id,
            redirect_uri,
            None,
            &state,
            pkce.challenge(),
        )
        .map_err(|source| Self::remote_auth_error("build authorization URL", &source))?;

        handler(TuistAuthPrompt {
            authorize_url: authorize_url.clone(),
            redirect_uri: redirect_uri.to_string(),
            opens_browser: open_browser_after_prompt,
        });
        if open_browser_after_prompt {
            open_browser(&authorize_url)
                .map_err(|source| Self::remote_auth_error("open browser", &source))?;
        }

        let callback = server
            .wait_for_callback(120)
            .map_err(|source| Self::remote_auth_error("wait for auth callback", &source))?;
        if callback.state.as_deref() != Some(state.as_str()) {
            return Err(Self::remote_auth_error(
                "authorize",
                &SchlusselError::InvalidState,
            ));
        }
        if callback.error_code.is_some() {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "authorize",
                message: callback
                    .error_description
                    .or(callback.error_code)
                    .unwrap_or_else(|| "authorization denied".to_string()),
            });
        }
        let code = callback.code.ok_or_else(|| Error::Remote {
            provider: PROVIDER_NAME,
            operation: "authorize",
            message: "missing authorization code".to_string(),
        })?;

        let key = self.storage_key();
        let storage = self.storage()?;
        let client = self.oauth_client(storage, client_id.to_string(), redirect_uri.to_string())?;
        let token = client
            .exchange_code(&code, pkce.verifier(), redirect_uri)
            .map_err(|source| Self::remote_auth_error("exchange authorization code", &source))?;
        client
            .save_token(&key, &token)
            .map_err(|source| Self::remote_auth_error("store auth token", &source))?;
        Ok(())
    }

    fn login_with_ci_oidc(&self) -> Result<()> {
        let oidc_token = Self::fetch_ci_oidc_token()?;
        let access_token = self.exchange_oidc_token(&oidc_token)?;
        self.store_access_token(&access_token)
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
        let response = Self::oidc_http_client()?
            .post(self.oidc_exchange_endpoint()?)
            .json(&OidcExchangeRequest { token: oidc_token })
            .send()
            .map_err(|source| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "exchange OpenID Connect token",
                message: source.to_string(),
            })?;
        let status = response.status();
        let body = response.text().map_err(|source| Error::Remote {
            provider: PROVIDER_NAME,
            operation: "exchange OpenID Connect token",
            message: source.to_string(),
        })?;
        if !status.is_success() {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "exchange OpenID Connect token",
                message: status_body_message(status, &body),
            });
        }

        let response: OidcExchangeResponse =
            serde_json::from_str(&body).map_err(|source| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "decode OpenID Connect token exchange",
                message: source.to_string(),
            })?;
        let access_token = response.access_token.trim();
        if access_token.is_empty() {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "decode OpenID Connect token exchange",
                message: "response did not include an access token".to_string(),
            });
        }

        Ok(Token::new(access_token, "Bearer").with_expiration(response.expires_in))
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

    fn register_client(&self, redirect_uri: &str) -> Result<RegisteredClient> {
        let client =
            DynamicRegistrationClient::new(self.registration_endpoint()?).map_err(|source| {
                Error::InvalidConfig {
                    provider: PROVIDER_NAME,
                    message: format!("invalid Tuist registration endpoint: {source}"),
                }
            })?;
        let response = client
            .register(&ClientMetadata {
                client_name: "once".to_string(),
                redirect_uris: vec![redirect_uri.to_string()],
                grant_types: vec![
                    "authorization_code".to_string(),
                    "refresh_token".to_string(),
                ],
                response_types: vec!["code".to_string()],
                token_endpoint_auth_method: Some("none".to_string()),
                ..ClientMetadata::default()
            })
            .map_err(|source| Self::remote_auth_error("register auth client", &source))?;
        let client_id = response.client_id.trim();
        if client_id.is_empty() {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "register auth client",
                message: "Tuist returned an empty client_id".to_string(),
            });
        }
        Ok(RegisteredClient {
            client_id: client_id.to_string(),
        })
    }

    fn oauth_client(
        &self,
        storage: FileStorage,
        client_id: String,
        redirect_uri: String,
    ) -> Result<OAuthClient<FileStorage>> {
        OAuthClient::new(self.oauth_config(client_id, redirect_uri)?, storage).map_err(|source| {
            Error::InvalidConfig {
                provider: PROVIDER_NAME,
                message: format!("invalid Tuist auth configuration: {source}"),
            }
        })
    }

    fn oauth_config(&self, client_id: String, redirect_uri: String) -> Result<OAuthConfig> {
        Ok(OAuthConfig {
            client_id,
            client_secret: None,
            authorization_endpoint: self.authorization_endpoint()?,
            token_endpoint: self.token_endpoint()?,
            redirect_uri,
            scope: None,
            device_authorization_endpoint: None,
        })
    }

    fn storage(&self) -> Result<FileStorage> {
        FileStorage::with_path(&self.credentials_root).map_err(|source| Error::Remote {
            provider: PROVIDER_NAME,
            operation: "open auth storage",
            message: source.to_string(),
        })
    }

    fn storage_key(&self) -> String {
        build_storage_key(
            PROVIDER_NAME,
            Some("authorization_code"),
            Some(self.identity().as_str()),
        )
    }

    fn resolve_client_id(&self) -> Result<String> {
        if let Some(client_id) = self.config.oauth_client_id.clone() {
            return Ok(client_id);
        }
        let registered_client =
            self.load_registered_client()?
                .ok_or_else(|| Error::InvalidConfig {
                    provider: PROVIDER_NAME,
                    message: self.login_hint(),
                })?;
        Ok(registered_client.client_id)
    }

    fn registration_endpoint(&self) -> Result<String> {
        Ok(join_url(&self.config.url, REGISTRATION_PATH)?.to_string())
    }

    fn authorization_endpoint(&self) -> Result<String> {
        Ok(join_url(&self.config.url, AUTHORIZATION_PATH)?.to_string())
    }

    fn token_endpoint(&self) -> Result<String> {
        Ok(join_url(&self.config.url, TOKEN_PATH)?.to_string())
    }

    fn oidc_exchange_endpoint(&self) -> Result<Url> {
        join_url(&self.config.url, OIDC_EXCHANGE_PATH)
    }

    fn load_registered_client(&self) -> Result<Option<RegisteredClient>> {
        let path = self.registration_path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(Error::Remote {
                    provider: PROVIDER_NAME,
                    operation: "load auth registration",
                    message: source.to_string(),
                });
            }
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| Error::InvalidConfig {
                provider: PROVIDER_NAME,
                message: format!(
                    "corrupt Tuist auth registration at {}: {source}",
                    path.display()
                ),
            })
    }

    fn save_registered_client(&self, registered_client: &RegisteredClient) -> Result<()> {
        let path = self.registration_path();
        let Some(parent) = path.parent() else {
            return Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "store auth registration",
                message: "registration path had no parent directory".to_string(),
            });
        };
        fs::create_dir_all(parent).map_err(|source| Error::Remote {
            provider: PROVIDER_NAME,
            operation: "store auth registration",
            message: source.to_string(),
        })?;
        let bytes =
            serde_json::to_vec_pretty(registered_client).map_err(|source| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "store auth registration",
                message: source.to_string(),
            })?;
        fs::write(path, bytes).map_err(|source| Error::Remote {
            provider: PROVIDER_NAME,
            operation: "store auth registration",
            message: source.to_string(),
        })
    }

    fn delete_registered_client(&self) -> Result<()> {
        let path = self.registration_path();
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "delete auth registration",
                message: source.to_string(),
            }),
        }
    }

    fn registration_path(&self) -> PathBuf {
        let key = Digest::of_bytes(self.identity().as_bytes()).to_hex();
        self.credentials_root
            .join("registrations")
            .join(format!("{key}.json"))
    }

    fn identity(&self) -> String {
        self.config.url.trim_end_matches('/').to_string()
    }

    fn login_hint(&self) -> String {
        format!(
            "set {} or run `once auth login --provider {}`",
            TUIST_TOKEN_ENV, self.config.provider_name
        )
    }

    fn cached_auth_error(&self, source: SchlusselError) -> Error {
        match source {
            SchlusselError::TokenNotFound(_)
            | SchlusselError::NoRefreshToken
            | SchlusselError::TokenExpired => Error::InvalidConfig {
                provider: PROVIDER_NAME,
                message: self.login_hint(),
            },
            other => Self::remote_auth_error("load auth token", &other),
        }
    }

    fn remote_auth_error(operation: &'static str, source: &SchlusselError) -> Error {
        Error::Remote {
            provider: PROVIDER_NAME,
            operation,
            message: source.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct RegisteredClient {
    client_id: String,
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

fn random_state() -> String {
    PkcePair::generate().verifier()[..22].to_string()
}

fn is_ci_environment() -> bool {
    CI_ENVIRONMENT_VARIABLES
        .iter()
        .any(|name| std::env::var_os(name).is_some())
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

fn print_prompt(prompt: &TuistAuthPrompt) {
    eprintln!();
    if prompt.opens_browser {
        eprintln!("Opening browser for authorization...");
        eprintln!("If the browser does not open, visit:");
    } else {
        eprintln!("Visit the following URL to authorize:");
    }
    eprintln!("{}", prompt.authorize_url);
    eprintln!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::thread;

    use tempfile::TempDir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    const TEST_ENV_KEYS: &[&str] = &[
        "GITHUB_RUN_ID",
        "CI",
        "BUILD_NUMBER",
        "GITHUB_ACTIONS",
        "ACTIONS_ID_TOKEN_REQUEST_URL",
        "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
        "CIRCLECI",
        "CIRCLE_OIDC_TOKEN_V2",
        "CIRCLE_OIDC_TOKEN",
        "BITRISE_IO",
        "BITRISE_OIDC_ID_TOKEN",
        "BITRISE_IDENTITY_TOKEN",
        "TUIST_TOKEN",
    ];

    fn static_config(url: String) -> TuistCacheConfig {
        TuistCacheConfig {
            url,
            account: Some("acme".to_string()),
            project: Some("demo".to_string()),
            oauth_client_id: Some(TUIST_APP_OAUTH_CLIENT_ID.to_string()),
            provider_name: "acme".to_string(),
        }
    }

    fn dynamic_config(url: String) -> TuistCacheConfig {
        TuistCacheConfig {
            oauth_client_id: None,
            ..static_config(url)
        }
    }

    #[test]
    fn reads_stored_token_from_file_storage() {
        let temp = TempDir::new().unwrap();
        let auth = TuistAuth::new(temp.path(), &static_config("https://tuist.dev".to_string()));
        let storage = auth.storage().unwrap();
        let key = auth.storage_key();
        storage
            .save(
                &key,
                &Token::new("access-1", "Bearer").with_expiration(Some(3600)),
            )
            .unwrap();

        let token = auth.token().unwrap();

        assert_eq!(token, "access-1");
    }

    #[test]
    fn missing_token_points_to_once_auth_login() {
        let temp = TempDir::new().unwrap();
        let auth = TuistAuth::new(temp.path(), &static_config("https://tuist.dev".to_string()));

        let error = auth.token().unwrap_err();

        assert!(error
            .to_string()
            .contains("run `once auth login --provider acme`"));
    }

    #[test]
    fn registers_dynamic_client_with_exact_loopback_redirect() {
        let server = OneShotHttpServer::new(
            201,
            r#"{
  "client_id": "dynamic-client-id",
  "client_secret": "dynamic-client-secret",
  "token_endpoint_auth_method": "none"
}"#,
        );
        let temp = TempDir::new().unwrap();
        let auth = TuistAuth::new(temp.path(), &dynamic_config(server.base_url()));

        let client = auth
            .register_client("http://127.0.0.1:4317/callback")
            .unwrap();
        let request = server.request();

        assert_eq!(client.client_id, "dynamic-client-id");
        assert!(request.starts_with("POST /oauth2/register "));
        assert!(request.contains("\"redirect_uris\":[\"http://127.0.0.1:4317/callback\"]"));
        assert!(request.contains("\"grant_types\":[\"authorization_code\",\"refresh_token\"]"));
        assert!(request.contains("\"response_types\":[\"code\"]"));
        assert!(request.contains("\"token_endpoint_auth_method\":\"none\""));
    }

    #[test]
    fn refreshes_expired_token_from_tuist_oauth_endpoint() {
        let server = OneShotHttpServer::new(
            200,
            r#"{
  "access_token": "access-2",
  "token_type": "Bearer",
  "expires_in": 3600
}"#,
        );
        let temp = TempDir::new().unwrap();
        let auth = TuistAuth::new(temp.path(), &dynamic_config(server.base_url()));
        let storage = auth.storage().unwrap();
        let key = auth.storage_key();
        auth.save_registered_client(&RegisteredClient {
            client_id: "dynamic-client-id".to_string(),
        })
        .unwrap();
        storage
            .save(
                &key,
                &Token {
                    access_token: "access-1".to_string(),
                    token_type: "Bearer".to_string(),
                    refresh_token: Some("refresh-1".to_string()),
                    expires_in: Some(1),
                    expires_at: Some(0),
                    scope: None,
                    id_token: None,
                },
            )
            .unwrap();

        let token = auth.token().unwrap();
        let request = server.request();
        let stored = storage.load(&key).unwrap().unwrap();

        assert_eq!(token, "access-2");
        assert!(request.starts_with("POST /oauth2/token "));
        assert!(request.contains("grant_type=refresh_token"));
        assert!(request.contains("refresh_token=refresh-1"));
        assert!(request.contains("client_id=dynamic-client-id"));
        assert_eq!(stored.access_token, "access-2");
        assert_eq!(stored.refresh_token.as_deref(), Some("refresh-1"));
    }

    #[test]
    fn env_token_takes_precedence_over_storage() {
        let temp = TempDir::new().unwrap();
        let auth = TuistAuth::new(temp.path(), &static_config("https://tuist.dev".to_string()));

        let token = auth.token_with_env(Some("env-token".to_string())).unwrap();

        assert_eq!(token, "env-token");
    }

    #[test]
    fn token_reads_account_token_from_environment() {
        let temp = TempDir::new().unwrap();
        let auth = TuistAuth::new(temp.path(), &static_config("https://tuist.dev".to_string()));

        let token = with_ci_env(
            &[
                ("CI", "true".to_string()),
                ("TUIST_TOKEN", "account-token".to_string()),
            ],
            || auth.token().unwrap(),
        );

        assert_eq!(token, "account-token");
    }

    #[test]
    fn github_actions_openid_connect_login_fetches_and_exchanges_token() {
        let github_server = OneShotHttpServer::new(200, r#"{"value":"github-identity-token"}"#);
        let exchange_server = OneShotHttpServer::new(
            200,
            r#"{"access_token":"tuist-access-token","expires_in":3600}"#,
        );
        let temp = TempDir::new().unwrap();
        let auth = TuistAuth::new(temp.path(), &static_config(exchange_server.base_url()));

        with_ci_env(
            &[
                ("CI", "true".to_string()),
                ("GITHUB_ACTIONS", "true".to_string()),
                (
                    "ACTIONS_ID_TOKEN_REQUEST_URL",
                    github_server.endpoint("/identity-token?existing=1"),
                ),
                (
                    "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
                    "github-request-token".to_string(),
                ),
            ],
            || {
                let mut handler = |_| panic!("browser prompt should not be used in runner auth");
                auth.login_with_handler(false, &mut handler).unwrap();
            },
        );

        let github_request = github_server.request();
        let exchange_request = exchange_server.request();
        let stored = auth
            .storage()
            .unwrap()
            .load(&auth.storage_key())
            .unwrap()
            .unwrap();

        assert!(github_request.starts_with("GET /identity-token?existing=1&audience=tuist "));
        assert!(github_request
            .to_ascii_lowercase()
            .contains("authorization: bearer github-request-token"));
        assert!(exchange_request.starts_with("POST /api/auth/oidc/token "));
        assert!(exchange_request.contains(r#""token":"github-identity-token""#));
        assert_eq!(stored.access_token, "tuist-access-token");
        assert_eq!(stored.expires_in, Some(3600));
        assert!(stored.expires_at.is_some());
    }

    #[test]
    fn circle_ci_openid_connect_login_exchanges_environment_token() {
        let exchange_server = OneShotHttpServer::new(
            200,
            r#"{"access_token":"tuist-access-token","expires_in":3600}"#,
        );
        let temp = TempDir::new().unwrap();
        let auth = TuistAuth::new(temp.path(), &static_config(exchange_server.base_url()));

        with_ci_env(
            &[
                ("CI", "true".to_string()),
                ("CIRCLECI", "true".to_string()),
                ("CIRCLE_OIDC_TOKEN_V2", "circle-identity-token".to_string()),
            ],
            || {
                let mut handler = |_| panic!("browser prompt should not be used in runner auth");
                auth.login_with_handler(false, &mut handler).unwrap();
            },
        );

        let exchange_request = exchange_server.request();
        let stored = auth
            .storage()
            .unwrap()
            .load(&auth.storage_key())
            .unwrap()
            .unwrap();

        assert!(exchange_request.starts_with("POST /api/auth/oidc/token "));
        assert!(exchange_request.contains(r#""token":"circle-identity-token""#));
        assert_eq!(stored.access_token, "tuist-access-token");
    }

    #[test]
    fn github_actions_openid_connect_login_reports_missing_permissions() {
        let temp = TempDir::new().unwrap();
        let auth = TuistAuth::new(temp.path(), &static_config("https://tuist.dev".to_string()));

        let error = with_ci_env(
            &[
                ("CI", "true".to_string()),
                ("GITHUB_ACTIONS", "true".to_string()),
            ],
            || {
                let mut handler = |_| panic!("browser prompt should not be used in runner auth");
                auth.login_with_handler(false, &mut handler).unwrap_err()
            },
        );

        assert!(error.to_string().contains("permissions: id-token: write"));
    }

    fn with_ci_env<T>(vars: &[(&'static str, String)], test: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap();
        let _env = EnvGuard::new(TEST_ENV_KEYS);
        for (name, value) in vars {
            std::env::set_var(name, value);
        }
        test()
    }

    struct EnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn new(names: &'static [&'static str]) -> Self {
            let saved = names
                .iter()
                .map(|name| (*name, std::env::var(name).ok()))
                .collect();
            for name in names {
                std::env::remove_var(name);
            }
            Self { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in &self.saved {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    struct OneShotHttpServer {
        _temp: TempDir,
        base_url: String,
        request_file: PathBuf,
        join_handle: Option<thread::JoinHandle<()>>,
    }

    impl OneShotHttpServer {
        fn new(status: u16, body: &'static str) -> Self {
            let temp = TempDir::new().unwrap();
            let request_file = temp.path().join("request.txt");
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let request_file_for_thread = request_file.clone();
            let join_handle = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_request(&mut stream);
                std::fs::write(&request_file_for_thread, request).unwrap();
                let response = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
            });

            Self {
                _temp: temp,
                base_url: format!("http://{addr}"),
                request_file,
                join_handle: Some(join_handle),
            }
        }

        fn base_url(&self) -> String {
            self.base_url.clone()
        }

        fn endpoint(&self, path: &str) -> String {
            format!("{}{}", self.base_url, path)
        }

        fn request(mut self) -> String {
            if let Some(join_handle) = self.join_handle.take() {
                join_handle.join().unwrap();
            }
            std::fs::read_to_string(&self.request_file).unwrap()
        }
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 1024];
        loop {
            let read = stream.read(&mut chunk).unwrap();
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..read]);
            if let Some(header_len) = header_length(&bytes) {
                let content_length = content_length(&bytes[..header_len]);
                while bytes.len() < header_len + content_length {
                    let read = stream.read(&mut chunk).unwrap();
                    if read == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..read]);
                }
                break;
            }
        }
        String::from_utf8(bytes).unwrap()
    }

    fn header_length(bytes: &[u8]) -> Option<usize> {
        bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|pos| pos + 4)
    }

    fn content_length(headers: &[u8]) -> usize {
        let headers = String::from_utf8_lossy(headers);
        headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.eq_ignore_ascii_case("content-length") {
                    value.trim().parse().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0)
    }
}
