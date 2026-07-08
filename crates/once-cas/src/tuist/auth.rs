//! Tuist authentication: token retrieval plus the login and logout entry
//! points. The interactive browser OAuth flow lives in [`oauth`], the
//! Continuous Integration `OpenID Connect` exchange in [`oidc`], and
//! dynamic client registration with its persistence in [`registration`].
//! This module owns the shared state, endpoint resolution, and credential
//! storage those flows build on.

mod oauth;
mod oidc;
mod registration;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use reqwest::Url;
use schlussel::{
    build_storage_key, CallbackServer, FileStorage, SchlusselError, SessionStorage, Token,
};

use super::{env_token, join_url, TuistCacheConfig, PROVIDER_NAME};
use crate::{Error, Result};

/// Public OAuth client id for Once's built-in Tuist app. This is not a secret.
pub const TUIST_APP_OAUTH_CLIENT_ID: &str = "b3298a92-3deb-4f5e-a526-b7ad324979b5";
pub const TUIST_OAUTH_CLIENT_ID_ENV: &str = "TUIST_OAUTH_CLIENT_ID";
const TUIST_TOKEN_ENV: &str = "TUIST_TOKEN";

const REGISTRATION_PATH: &str = "oauth2/register";
const TOKEN_PATH: &str = "oauth2/token";
const AUTHORIZATION_PATH: &str = "oauth2/authorize";
const OIDC_EXCHANGE_PATH: &str = "api/auth/oidc/token";
const OIDC_TOKEN_REFRESH_WINDOW_SECONDS: u64 = 60;
const CI_ENVIRONMENT_VARIABLES: &[&str] = &["GITHUB_RUN_ID", "CI", "BUILD_NUMBER"];

#[derive(Debug, Clone)]
pub struct TuistAuth {
    credentials_root: PathBuf,
    config: TuistCacheConfig,
    cached_ci_token: Arc<StdMutex<Option<Token>>>,
    ci_exchange_lock: Arc<StdMutex<()>>,
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
            cached_ci_token: Arc::new(StdMutex::new(None)),
            ci_exchange_lock: Arc::new(StdMutex::new(())),
        }
    }

    pub fn token(&self) -> Result<String> {
        self.token_with_env(env_token(TUIST_TOKEN_ENV))
    }

    fn token_with_env(&self, env_token: Option<String>) -> Result<String> {
        if let Some(token) = env_token {
            return Ok(token);
        }

        if let Some(token) = self.cached_ci_access_token()? {
            return Ok(token);
        }

        if let Some(token) = self.load_stored_access_token()? {
            return Ok(token);
        }

        match self.load_valid_token() {
            Ok(token) => Ok(token.access_token),
            Err(_) if is_ci_environment() => {
                Ok(self.exchange_and_store_ci_oidc_token()?.access_token)
            }
            Err(error) => Err(error),
        }
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
        if env_token(TUIST_TOKEN_ENV).is_some() {
            return Ok(());
        }

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

fn is_ci_environment() -> bool {
    CI_ENVIRONMENT_VARIABLES
        .iter()
        .any(|name| std::env::var_os(name).is_some())
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
    use super::registration::RegisteredClient;
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
    fn reads_stored_token_without_registered_client() {
        let temp = TempDir::new().unwrap();
        let auth = TuistAuth::new(
            temp.path(),
            &dynamic_config("https://tuist.dev".to_string()),
        );
        let storage = auth.storage().unwrap();
        let key = auth.storage_key();
        storage
            .save(
                &key,
                &Token::new("access-1", "Bearer").with_expiration(Some(3600)),
            )
            .unwrap();

        let token = with_ci_env(&[], || auth.token().unwrap());

        assert_eq!(token, "access-1");
    }

    #[test]
    fn missing_token_points_to_once_auth_login() {
        let temp = TempDir::new().unwrap();
        let auth = TuistAuth::new(temp.path(), &static_config("https://tuist.dev".to_string()));

        let error = with_ci_env(&[], || auth.token().unwrap_err());

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
    fn login_succeeds_with_account_token_in_environment() {
        let temp = TempDir::new().unwrap();
        let auth = TuistAuth::new(temp.path(), &static_config("https://tuist.dev".to_string()));

        with_ci_env(
            &[
                ("CI", "true".to_string()),
                ("TUIST_TOKEN", "account-token".to_string()),
            ],
            || {
                let mut handler =
                    |_| panic!("browser prompt should not be used with account token");
                auth.login_with_handler(false, &mut handler).unwrap();
            },
        );
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
    fn token_fetches_and_exchanges_github_actions_openid_connect_when_storage_is_missing() {
        let github_server = OneShotHttpServer::new(200, r#"{"value":"github-identity-token"}"#);
        let exchange_server = OneShotHttpServer::new(
            200,
            r#"{"access_token":"tuist-access-token","expires_in":3600}"#,
        );
        let temp = TempDir::new().unwrap();
        let auth = TuistAuth::new(temp.path(), &static_config(exchange_server.base_url()));

        let token = with_ci_env(
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
            || auth.token().unwrap(),
        );

        let github_request = github_server.request();
        let exchange_request = exchange_server.request();
        let stored = auth
            .storage()
            .unwrap()
            .load(&auth.storage_key())
            .unwrap()
            .unwrap();

        assert_eq!(token, "tuist-access-token");
        assert!(github_request.starts_with("GET /identity-token?existing=1&audience=tuist "));
        assert!(exchange_request.starts_with("POST /api/auth/oidc/token "));
        assert_eq!(stored.access_token, "tuist-access-token");
    }

    #[test]
    fn openid_connect_exchange_retries_retryable_tuist_failures() {
        let github_server = OneShotHttpServer::new(200, r#"{"value":"github-identity-token"}"#);
        let exchange_server = MultiShotHttpServer::new(vec![
            (502, "error code: 502"),
            (
                200,
                r#"{"access_token":"tuist-access-token","expires_in":3600}"#,
            ),
        ]);
        let temp = TempDir::new().unwrap();
        let auth = TuistAuth::new(temp.path(), &static_config(exchange_server.base_url()));

        let token = with_ci_env(
            &[
                ("CI", "true".to_string()),
                ("GITHUB_ACTIONS", "true".to_string()),
                (
                    "ACTIONS_ID_TOKEN_REQUEST_URL",
                    github_server.endpoint("/identity-token"),
                ),
                (
                    "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
                    "github-request-token".to_string(),
                ),
            ],
            || auth.token().unwrap(),
        );

        let github_request = github_server.request();
        let exchange_requests = exchange_server.requests();
        assert_eq!(token, "tuist-access-token");
        assert!(github_request.starts_with("GET /identity-token?audience=tuist "));
        assert_eq!(exchange_requests.len(), 2);
        assert!(exchange_requests[0].starts_with("POST /api/auth/oidc/token "));
        assert!(exchange_requests[1].starts_with("POST /api/auth/oidc/token "));
    }

    #[test]
    fn token_reuses_cached_openid_connect_token_when_storage_is_missing() {
        let github_server = OneShotHttpServer::new(200, r#"{"value":"github-identity-token"}"#);
        let exchange_server = OneShotHttpServer::new(
            200,
            r#"{"access_token":"tuist-access-token","expires_in":3600}"#,
        );
        let temp = TempDir::new().unwrap();
        let auth = TuistAuth::new(temp.path(), &static_config(exchange_server.base_url()));

        let (first, second) = with_ci_env(
            &[
                ("CI", "true".to_string()),
                ("GITHUB_ACTIONS", "true".to_string()),
                (
                    "ACTIONS_ID_TOKEN_REQUEST_URL",
                    github_server.endpoint("/identity-token"),
                ),
                (
                    "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
                    "github-request-token".to_string(),
                ),
            ],
            || {
                let first = auth.token().unwrap();
                auth.storage().unwrap().delete(&auth.storage_key()).unwrap();
                let second = auth.token().unwrap();
                (first, second)
            },
        );

        assert_eq!(first, "tuist-access-token");
        assert_eq!(second, "tuist-access-token");
        assert!(github_server
            .request()
            .starts_with("GET /identity-token?audience=tuist "));
        assert!(exchange_server
            .request()
            .starts_with("POST /api/auth/oidc/token "));
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

    struct MultiShotHttpServer {
        _temp: TempDir,
        base_url: String,
        request_dir: PathBuf,
        request_count: usize,
        join_handle: Option<thread::JoinHandle<()>>,
    }

    impl MultiShotHttpServer {
        fn new(responses: Vec<(u16, &'static str)>) -> Self {
            let temp = TempDir::new().unwrap();
            let request_dir = temp.path().join("requests");
            std::fs::create_dir_all(&request_dir).unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let request_dir_for_thread = request_dir.clone();
            let request_count = responses.len();
            let join_handle = thread::spawn(move || {
                for (index, (status, body)) in responses.into_iter().enumerate() {
                    let (mut stream, _) = listener.accept().unwrap();
                    let request = read_http_request(&mut stream);
                    std::fs::write(
                        request_dir_for_thread.join(format!("request-{index}.txt")),
                        request,
                    )
                    .unwrap();
                    let response = format!(
                        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).unwrap();
                }
            });

            Self {
                _temp: temp,
                base_url: format!("http://{addr}"),
                request_dir,
                request_count,
                join_handle: Some(join_handle),
            }
        }

        fn base_url(&self) -> String {
            self.base_url.clone()
        }

        fn requests(mut self) -> Vec<String> {
            if let Some(join_handle) = self.join_handle.take() {
                join_handle.join().unwrap();
            }
            (0..self.request_count)
                .map(|index| {
                    std::fs::read_to_string(self.request_dir.join(format!("request-{index}.txt")))
                        .unwrap()
                })
                .collect()
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
