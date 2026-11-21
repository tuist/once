//! A remote tier on any HTTP cache that stores opaque values with
//! `GET`, `HEAD`, and `PUT` on a key path, such as the `WebDAV` endpoints
//! Depot Cache and Namespace expose to CI jobs.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use bytes::Bytes;
use futures::TryStreamExt;
use reqwest::header::{HeaderValue, AUTHORIZATION, CONTENT_LENGTH};
use reqwest::{Method, StatusCode, Url};
use tokio_util::io::{ReaderStream, StreamReader};

use super::{RemoteBody, RemoteStore};
use crate::{Error, ErrorKind, Result};

const PROVIDER: &str = "http_cache";
const NAMESPACE: &str = "v1.";
const UPLOAD_CHUNK_BYTES: usize = 64 * 1024;
const UPLOAD_BUFFERED_CHUNKS: u64 = 4;

/// Credentials sent with every request.
#[derive(Clone, Default)]
pub enum HttpCacheAuth {
    /// No `Authorization` header.
    #[default]
    None,
    /// `Authorization: Bearer <token>`.
    Bearer(String),
    /// HTTP basic authentication.
    Basic {
        /// User name, such as an organization id.
        username: String,
        /// Password or token.
        password: String,
    },
}

impl HttpCacheAuth {
    /// Read credentials from the environment variables a configuration names,
    /// so tokens never live in a manifest. Naming a variable that is unset or
    /// empty is a configuration error rather than an anonymous request.
    pub fn from_env(
        token_env: Option<&str>,
        username_env: Option<&str>,
        password_env: Option<&str>,
    ) -> Result<Self> {
        Self::from_lookup(token_env, username_env, password_env, |name| {
            std::env::var(name).ok()
        })
    }

    fn from_lookup(
        token_env: Option<&str>,
        username_env: Option<&str>,
        password_env: Option<&str>,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self> {
        let read = |name: &str| {
            lookup(name)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| misconfigured(format!("environment variable `{name}` is not set")))
        };
        match (token_env, username_env, password_env) {
            (None, None, None) => Ok(Self::None),
            (Some(token), None, None) => Ok(Self::Bearer(read(token)?)),
            (None, Some(username), Some(password)) => Ok(Self::Basic {
                username: read(username)?,
                password: read(password)?,
            }),
            _ => Err(misconfigured(
                "set either `token_env`, or both `username_env` and `password_env`",
            )),
        }
    }
}

impl fmt::Debug for HttpCacheAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::None => "None",
            Self::Bearer(_) => "Bearer(<redacted>)",
            Self::Basic { .. } => "Basic(<redacted>)",
        })
    }
}

/// An HTTP key-value cache used as a [`RemoteStore`].
///
/// Keys are flattened into one versioned path segment under `url` and
/// `key_prefix` (`cas/<hex>` becomes `<prefix>v1.cas.<hex>`), so `WebDAV`
/// servers never need parent collections. The server must make a `PUT`
/// visible only once the
/// whole body arrived; Struo verifies every downloaded blob regardless.
#[derive(Debug, Clone)]
pub struct HttpCacheRemote {
    client: reqwest::Client,
    base: Url,
    key_prefix: String,
    auth: HttpCacheAuth,
}

impl HttpCacheRemote {
    /// Address a cache at `url`. Credentials are only sent over HTTPS, or
    /// plain HTTP to a loopback host.
    pub fn new(url: &str, key_prefix: Option<&str>, auth: HttpCacheAuth) -> Result<Self> {
        let trimmed = url.trim();
        let with_scheme = if trimmed.contains("://") {
            trimmed.to_string()
        } else {
            format!("https://{trimmed}")
        };
        let mut base =
            Url::parse(&with_scheme).map_err(|_| misconfigured("invalid HTTP cache URL"))?;
        if !base.username().is_empty() || base.password().is_some() {
            return Err(misconfigured(
                "URL credentials are not supported; use credential environment variables",
            ));
        }
        if base.query().is_some() || base.fragment().is_some() {
            return Err(misconfigured(
                "HTTP cache URL must not contain a query or fragment",
            ));
        }
        let secure = match base.scheme() {
            "https" => true,
            "http" => is_loopback(&base),
            other => return Err(misconfigured(format!("unsupported URL scheme `{other}`"))),
        };
        if !secure && !matches!(auth, HttpCacheAuth::None) {
            return Err(misconfigured(
                "refusing to send credentials over plain HTTP; use https",
            ));
        }
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let key_prefix = key_prefix.unwrap_or_default();
        if key_prefix
            .chars()
            .any(|c| c.is_control() || matches!(c, '?' | '#' | ':' | '\\'))
            || key_prefix.starts_with('/')
            || key_prefix.split('/').any(|part| matches!(part, "." | ".."))
        {
            return Err(misconfigured("key prefix must be a relative cache path"));
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(60))
            .build()
            .map_err(|source| misconfigured(format!("building HTTP client: {source}")))?;
        Ok(Self {
            client,
            base,
            key_prefix: key_prefix.to_string(),
            auth,
        })
    }

    fn url(&self, key: &str) -> Result<Url> {
        let path = format!("{}{NAMESPACE}{}", self.key_prefix, key.replace('/', "."));
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|()| misconfigured("HTTP cache URL cannot contain path segments"))?
            .pop_if_empty()
            .extend(path.split('/'));
        Ok(url)
    }

    fn request(&self, method: Method, key: &str) -> Result<reqwest::RequestBuilder> {
        let request = self.client.request(method, self.url(key)?);
        Ok(match &self.auth {
            HttpCacheAuth::None => request,
            HttpCacheAuth::Bearer(token) => {
                let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|_| misconfigured("token is not a valid header value"))?;
                value.set_sensitive(true);
                request.header(AUTHORIZATION, value)
            }
            HttpCacheAuth::Basic { username, password } => {
                request.basic_auth(username, Some(password))
            }
        })
    }

    async fn send(
        &self,
        operation: &'static str,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response> {
        request.send().await.map_err(|error| {
            Error::remote(
                PROVIDER,
                operation,
                ErrorKind::Unavailable,
                // `without_url` keeps key prefixes, which may embed
                // tenant names, out of logs.
                error.without_url().to_string(),
            )
        })
    }

    fn finish_put(response: &reqwest::Response) -> Result<()> {
        let status = response.status();
        // An existing or in-flight entry for the same content-addressed key
        // already holds (or will hold) the same bytes.
        if status.is_success() || status == StatusCode::CONFLICT {
            return Ok(());
        }
        Err(status_error("put", status))
    }
}

#[async_trait::async_trait]
impl RemoteStore for HttpCacheRemote {
    fn provider(&self) -> &'static str {
        PROVIDER
    }

    async fn head(&self, key: &str) -> Result<Option<u64>> {
        let response = self.send("head", self.request(Method::HEAD, key)?).await?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(None),
            status if status.is_success() => content_length(&response, "head").map(Some),
            status => Err(status_error("head", status)),
        }
    }

    async fn get(&self, key: &str) -> Result<Option<RemoteBody>> {
        let response = self.send("get", self.request(Method::GET, key)?).await?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(None),
            status if status.is_success() => {
                let size = content_length(&response, "get")?;
                let stream = response
                    .bytes_stream()
                    .map_err(|error| std::io::Error::other(error.without_url()));
                Ok(Some(RemoteBody {
                    size,
                    reader: Box::pin(StreamReader::new(stream)),
                }))
            }
            status => Err(status_error("get", status)),
        }
    }

    async fn put_bytes(&self, key: &str, bytes: Bytes) -> Result<()> {
        let request = self
            .request(Method::PUT, key)?
            .header(CONTENT_LENGTH, bytes.len())
            .body(bytes);
        let response = self.send("put", request).await?;
        Self::finish_put(&response)
    }

    async fn put_file(&self, key: &str, path: &Path, size: u64) -> Result<()> {
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|source| Error::Io {
                path: path.to_path_buf(),
                source,
            })?;
        let body = reqwest::Body::wrap_stream(ReaderStream::with_capacity(
            tokio::io::AsyncReadExt::take(file, size),
            UPLOAD_CHUNK_BYTES,
        ));
        let request = self
            .request(Method::PUT, key)?
            .header(CONTENT_LENGTH, size)
            .body(body);
        let response = self.send("put", request).await?;
        Self::finish_put(&response)
    }

    fn upload_memory(&self, size: u64) -> u64 {
        size.min(UPLOAD_CHUNK_BYTES as u64 * UPLOAD_BUFFERED_CHUNKS)
    }
}

fn content_length(response: &reqwest::Response, operation: &'static str) -> Result<u64> {
    response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| {
            Error::remote(
                PROVIDER,
                operation,
                ErrorKind::Protocol,
                "response has no Content-Length",
            )
        })
}

fn status_error(operation: &'static str, status: StatusCode) -> Error {
    let kind = match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => ErrorKind::Refused,
        StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS => ErrorKind::Unavailable,
        status if status.is_server_error() => ErrorKind::Unavailable,
        _ => ErrorKind::Protocol,
    };
    Error::remote(
        PROVIDER,
        operation,
        kind,
        format!("HTTP {}", status.as_u16()),
    )
}

fn misconfigured(message: impl Into<String>) -> Error {
    Error::InvalidConfig {
        provider: PROVIDER,
        message: message.into(),
    }
}

fn is_loopback(url: &Url) -> bool {
    match url.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use axum::body::Body;
    use axum::extract::{Request, State};
    use axum::http::StatusCode as AxumStatus;
    use axum::response::Response;
    use tempfile::TempDir;

    use super::*;
    use crate::remote::{action_key, blob_key, RemoteAccess};
    use crate::{ActionResult, CacheProvider, Digest};

    #[derive(Default)]
    struct Server {
        objects: HashMap<String, Vec<u8>>,
        authorizations: Vec<Option<String>>,
        forced: Option<u16>,
    }

    type Shared = Arc<Mutex<Server>>;

    async fn handle(State(state): State<Shared>, request: Request) -> Response {
        let path = request.uri().path().to_string();
        let method = request.method().clone();
        let authorization = request
            .headers()
            .get("authorization")
            .map(|value| value.to_str().unwrap().to_string());
        let forced = {
            let mut server = state.lock().unwrap();
            server.authorizations.push(authorization);
            server.forced
        };
        if let Some(code) = forced {
            return status(code);
        }
        if method == axum::http::Method::PUT {
            let body = axum::body::to_bytes(request.into_body(), usize::MAX)
                .await
                .unwrap();
            state.lock().unwrap().objects.insert(path, body.to_vec());
            return status(201);
        }
        let object = state.lock().unwrap().objects.get(&path).cloned();
        match object {
            None => status(404),
            Some(bytes) => Response::builder()
                .header("content-length", bytes.len())
                .body(if method == axum::http::Method::HEAD {
                    Body::empty()
                } else {
                    Body::from(bytes)
                })
                .unwrap(),
        }
    }

    fn status(code: u16) -> Response {
        Response::builder()
            .status(AxumStatus::from_u16(code).unwrap())
            .body(Body::empty())
            .unwrap()
    }

    async fn serve() -> (String, Shared) {
        let state = Shared::default();
        let app = axum::Router::new()
            .fallback(handle)
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}/cache"), state)
    }

    fn remote(url: &str) -> Arc<HttpCacheRemote> {
        Arc::new(
            HttpCacheRemote::new(
                url,
                Some("team/"),
                HttpCacheAuth::Bearer("secret-token".into()),
            )
            .unwrap(),
        )
    }

    #[tokio::test]
    async fn a_second_client_reuses_results_through_flattened_keys() {
        let (url, state) = serve().await;
        let (first_root, second_root) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let first = CacheProvider::remote(first_root.path(), remote(&url), RemoteAccess::ReadWrite);
        // Larger than one upload chunk, so the streamed file body runs.
        let large: Vec<u8> = (0..3 * 1024 * 1024 + 5)
            .map(|i: u32| u8::try_from(i % 249).unwrap())
            .collect();
        let output = first.put_stream(&large[..]).await.unwrap();
        let stdout = first.put_blob(b"log").await.unwrap();
        let action = Digest::of_bytes(b"action");
        let result = ActionResult {
            exit_code: 2,
            stdout: Some(stdout),
            stderr: None,
            outputs: [("out/a".to_string(), output)].into(),
        };
        first.put_action_result(&action, &result).await.unwrap();

        {
            let server = state.lock().unwrap();
            let expected = format!("/cache/team/v1.cas.{}", output.to_hex());
            assert_eq!(server.objects.get(&expected), Some(&large));
            assert!(server
                .objects
                .contains_key(&format!("/cache/team/v1.ac.{}", action.to_hex())));
            assert!(server
                .authorizations
                .iter()
                .all(|value| value.as_deref() == Some("Bearer secret-token")));
        }

        let second =
            CacheProvider::remote(second_root.path(), remote(&url), RemoteAccess::ReadWrite);
        let hit = second.lookup_action_result(&action).await.unwrap().unwrap();
        assert!(hit.blobs_available);
        assert_eq!(hit.result, result);
        let restored = second_root.path().join("restored");
        second.copy_blob_to_file(&output, &restored).await.unwrap();
        assert_eq!(std::fs::read(restored).unwrap(), large);
    }

    #[tokio::test]
    async fn statuses_keep_their_failure_category() {
        let (url, state) = serve().await;
        let store = remote(&url);
        let key = blob_key(&Digest::of_bytes(b"x"));
        assert_eq!(store.head(&key).await.unwrap(), None);
        assert!(store
            .get(&action_key(&Digest::of_bytes(b"a")))
            .await
            .unwrap()
            .is_none());
        for (code, kind) in [
            (401, ErrorKind::Refused),
            (403, ErrorKind::Refused),
            (429, ErrorKind::Unavailable),
            (503, ErrorKind::Unavailable),
            (400, ErrorKind::Protocol),
        ] {
            state.lock().unwrap().forced = Some(code);
            assert_eq!(store.head(&key).await.unwrap_err().kind(), kind, "{code}");
            assert_eq!(
                store
                    .put_bytes(&key, Bytes::from_static(b"x"))
                    .await
                    .unwrap_err()
                    .kind(),
                kind,
                "{code}"
            );
        }
        state.lock().unwrap().forced = Some(409);
        store
            .put_bytes(&key, Bytes::from_static(b"x"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn an_unreachable_server_is_unavailable_not_a_miss() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let error = remote(&url).head("cas/x").await.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Unavailable);
        assert!(!error.to_string().contains("secret-token"));
    }

    #[test]
    fn prefixes_cannot_escape_the_configured_cache_path() {
        for prefix in [
            "http://other.example/",
            "https://other.example/",
            "//other.example/",
            "\\\\other.example\\",
            "../other/",
            "team/../../other/",
            "team/./",
        ] {
            let error = HttpCacheRemote::new(
                "https://cache.example/base",
                Some(prefix),
                HttpCacheAuth::Bearer("secret-token".into()),
            )
            .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Misconfigured, "{prefix}");
        }
        let store = HttpCacheRemote::new(
            "https://cache.example/base%20path",
            Some("team/%2e%2e/"),
            HttpCacheAuth::None,
        )
        .unwrap();
        assert_eq!(
            store.url("cas/abc").unwrap().as_str(),
            "https://cache.example/base%20path/team/%252e%252e/v1.cas.abc"
        );
    }

    #[test]
    fn urls_cannot_hide_credentials_or_leak_them_in_errors() {
        for url in [
            "http://user:private-password@cache.example/",
            "https://user:private-password@cache.example/",
            "https://user@cache.example/",
            "https://cache.example/?token=private-password",
            "https://cache.example/#private-password",
            "https://user:private-password@invalid host/",
        ] {
            let error = HttpCacheRemote::new(url, None, HttpCacheAuth::None).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Misconfigured);
            assert!(!format!("{error:?} {error}").contains("private-password"));
        }
    }

    #[tokio::test]
    async fn redirects_do_not_send_requests_to_another_cache() {
        let (target_url, target) = serve().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().fallback(move || async move {
            Response::builder()
                .status(307)
                .header("location", target_url)
                .body(Body::empty())
                .unwrap()
        });
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        for auth in [
            HttpCacheAuth::Bearer("secret-token".into()),
            HttpCacheAuth::Basic {
                username: "user".into(),
                password: "secret-token".into(),
            },
        ] {
            let store = HttpCacheRemote::new(&format!("http://{address}"), None, auth).unwrap();
            assert_eq!(
                store.head("cas/abc").await.unwrap_err().kind(),
                ErrorKind::Protocol
            );
            assert_eq!(
                store
                    .put_bytes("cas/abc", Bytes::from_static(b"private output"))
                    .await
                    .unwrap_err()
                    .kind(),
                ErrorKind::Protocol
            );
        }
        assert!(target.lock().unwrap().authorizations.is_empty());
        server.abort();
    }

    #[test]
    fn credentials_come_only_from_named_environment_variables() {
        let lookup = |name: &str| match name {
            "TOKEN" => Some("t".to_string()),
            "USER" => Some("org".to_string()),
            "EMPTY" => Some("  ".to_string()),
            _ => None,
        };
        assert!(matches!(
            HttpCacheAuth::from_lookup(Some("TOKEN"), None, None, lookup).unwrap(),
            HttpCacheAuth::Bearer(token) if token == "t"
        ));
        assert!(matches!(
            HttpCacheAuth::from_lookup(None, Some("USER"), Some("TOKEN"), lookup).unwrap(),
            HttpCacheAuth::Basic { username, password } if username == "org" && password == "t"
        ));
        for (token, username, password) in [
            (Some("MISSING"), None, None),
            (Some("EMPTY"), None, None),
            (None, Some("USER"), None),
            (Some("TOKEN"), Some("USER"), Some("TOKEN")),
        ] {
            let error = HttpCacheAuth::from_lookup(token, username, password, lookup).unwrap_err();
            assert_eq!(
                error.kind(),
                ErrorKind::Misconfigured,
                "{token:?} {username:?}"
            );
        }
    }

    #[test]
    fn credentials_are_never_sent_in_clear_text_to_remote_hosts() {
        let auth = HttpCacheAuth::Bearer("secret-token".into());
        let error =
            HttpCacheRemote::new("http://cache.example.com", None, auth.clone()).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Misconfigured);
        HttpCacheRemote::new("http://[::1]:8080", None, auth.clone()).unwrap();
        let bare = HttpCacheRemote::new("cache.depot.dev", None, auth).unwrap();
        assert_eq!(bare.base.scheme(), "https");
        assert!(!format!("{bare:?}").contains("secret-token"));
    }
}
