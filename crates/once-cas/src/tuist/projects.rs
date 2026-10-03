//! Provider-owned project discovery and creation.
//!
//! Once resolves and writes an infrastructure binding into `once.toml`
//! without knowing what a provider considers a project. This module owns
//! the Tuist HTTP calls that back that workflow: listing the projects and
//! organizations the stored session can see, and creating a new project.
//!
//! The types here are deliberately not part of the `once` SDK surface.
//! Callers reach them through the CLI's provider seam.

use std::path::Path;
use std::time::Duration;

use reqwest::Method;
use serde::{Deserialize, Serialize};

use super::{join_url, remote_status_message, TuistAuth, TuistCacheConfig, PROVIDER_NAME};
use crate::{Error, Result};

const PROJECTS_PATH: &str = "api/projects";
const ORGANIZATIONS_PATH: &str = "api/organizations";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Failure while creating a project, split by what the caller can do about it.
#[derive(Debug, thiserror::Error)]
pub enum ProjectCreateError {
    /// The provider already has a project with that handle.
    #[error("project already exists")]
    AlreadyExists,
    /// The session is authenticated but not allowed to create projects.
    #[error("the session is not authorized to create projects")]
    PermissionDenied,
    /// Any other provider or transport failure.
    #[error(transparent)]
    Other(#[from] Error),
}

/// A project owned by an infrastructure provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteProject {
    /// Account or organization handle that owns the project.
    pub account: String,
    /// Project handle, unique within the account.
    pub project: String,
}

impl RemoteProject {
    /// `account/project`, the provider's portable project identifier.
    pub fn full_handle(&self) -> String {
        format!("{}/{}", self.account, self.project)
    }
}

/// Project discovery and creation against a Tuist server.
#[derive(Debug, Clone)]
pub struct TuistProjects {
    client: reqwest::Client,
    config: TuistCacheConfig,
    auth: TuistAuth,
}

impl TuistProjects {
    /// Bind project operations to a credential root and server config.
    pub fn new(auth_root: impl AsRef<Path>, config: TuistCacheConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|source| Error::InvalidConfig {
                provider: PROVIDER_NAME,
                message: format!("could not build HTTP client: {source}"),
            })?;
        Ok(Self {
            client,
            auth: TuistAuth::new(auth_root, &config),
            config,
        })
    }

    /// Server URL the project operations talk to.
    pub fn server_url(&self) -> &str {
        &self.config.url
    }

    /// Whether the provider already has a reusable session for this machine.
    pub fn has_session(&self) -> bool {
        self.auth.has_stored_session()
    }

    /// Providers that name their instance in the manifest.
    pub fn provider_name(&self) -> &str {
        &self.config.provider_name
    }

    /// OAuth client id pinned for this provider, when one is configured.
    pub fn oauth_client_id(&self) -> Option<&str> {
        self.config.oauth_client_id.as_deref()
    }

    /// Projects the stored session can see, in provider order.
    pub async fn list_projects(&self) -> Result<Vec<RemoteProject>> {
        let url = self.endpoint(PROJECTS_PATH)?;
        let response = self
            .authorized(Method::GET, url)
            .await?
            .send()
            .await
            .map_err(|source| remote_error("list projects", source))?;
        self.decode_projects(response).await
    }

    /// Create a project and return it.
    ///
    /// When `account` is `Some`, the request is scoped to
    /// `account/project`. When it is `None`, the provider creates the project
    /// under the account that owns the session, such as the user's personal
    /// account, and the response carries the resolved account handle.
    ///
    /// "Already exists" and "not authorized" are reported as typed failures so
    /// callers do not have to match on provider prose.
    pub async fn create_project(
        &self,
        account: Option<&str>,
        project: &str,
    ) -> std::result::Result<RemoteProject, ProjectCreateError> {
        let url = self.endpoint(PROJECTS_PATH)?;
        let body = match account {
            Some(account) => CreateProjectRequest {
                full_handle: Some(format!("{account}/{project}")),
                name: None,
            },
            None => CreateProjectRequest {
                full_handle: None,
                name: Some(project.to_string()),
            },
        };
        let response = self
            .authorized(Method::POST, url)
            .await?
            .json(&body)
            .send()
            .await
            .map_err(|source| remote_error("create project", source))?;
        let response = self.decode_create(response).await?;
        let body: ProjectResponse = response
            .json()
            .await
            .map_err(|source| remote_error("create project", source))?;
        parse_full_handle(&body.full_name).ok_or_else(|| {
            ProjectCreateError::Other(Error::Remote {
                provider: PROVIDER_NAME,
                operation: "create project",
                message: format!(
                    "provider returned an unparseable project handle `{}`",
                    body.full_name
                ),
            })
        })
    }

    async fn decode_create(
        &self,
        response: reqwest::Response,
    ) -> std::result::Result<reqwest::Response, ProjectCreateError> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let message = remote_status_message(response).await;
        if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ProjectCreateError::PermissionDenied);
        }
        if status == reqwest::StatusCode::BAD_REQUEST && is_already_exists_message(&message) {
            return Err(ProjectCreateError::AlreadyExists);
        }
        Err(ProjectCreateError::Other(Error::Remote {
            provider: PROVIDER_NAME,
            operation: "create project",
            message: format!("{status}: {message}"),
        }))
    }

    /// Organizations the stored session belongs to, in provider order.
    pub async fn list_organizations(&self) -> Result<Vec<String>> {
        let url = self.endpoint(ORGANIZATIONS_PATH)?;
        let response = self
            .authorized(Method::GET, url)
            .await?
            .send()
            .await
            .map_err(|source| remote_error("list organizations", source))?;
        self.decode_organizations(response).await
    }

    fn endpoint(&self, path: &str) -> Result<reqwest::Url> {
        join_url(&self.config.url, path)
    }

    async fn authorized(
        &self,
        method: Method,
        url: reqwest::Url,
    ) -> Result<reqwest::RequestBuilder> {
        // Token resolution may refresh stored credentials through a blocking
        // client, so it runs off the async runtime like the cache path does.
        let auth = self.auth.clone();
        let token = tokio::task::spawn_blocking(move || auth.token())
            .await
            .map_err(|source| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "resolve project session",
                message: source.to_string(),
            })?
            .map_err(|source| Error::Remote {
                provider: PROVIDER_NAME,
                operation: "resolve project session",
                message: source.to_string(),
            })?;
        Ok(self.client.request(method, url).bearer_auth(token))
    }

    async fn decode_projects(&self, response: reqwest::Response) -> Result<Vec<RemoteProject>> {
        let response = self.ensure_success("list projects", response).await?;
        let body: ProjectsResponse = response
            .json()
            .await
            .map_err(|source| remote_error("list projects", source))?;
        Ok(body
            .projects
            .into_iter()
            .filter_map(|project| parse_full_handle(&project.full_name))
            .collect())
    }

    async fn decode_organizations(&self, response: reqwest::Response) -> Result<Vec<String>> {
        let response = self.ensure_success("list organizations", response).await?;
        let body: OrganizationsResponse = response
            .json()
            .await
            .map_err(|source| remote_error("list organizations", source))?;
        Ok(body
            .organizations
            .into_iter()
            .map(|organization| organization.name)
            .collect())
    }

    async fn ensure_success(
        &self,
        operation: &'static str,
        response: reqwest::Response,
    ) -> Result<reqwest::Response> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let message = remote_status_message(response).await;
        Err(Error::Remote {
            provider: PROVIDER_NAME,
            operation,
            message: format!("{status}: {message}"),
        })
    }
}

#[derive(Debug, Serialize)]
struct CreateProjectRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    full_handle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProjectsResponse {
    projects: Vec<ProjectResponse>,
}

#[derive(Debug, Deserialize)]
struct ProjectResponse {
    full_name: String,
}

#[derive(Debug, Deserialize)]
struct OrganizationsResponse {
    organizations: Vec<OrganizationResponse>,
}

#[derive(Debug, Deserialize)]
struct OrganizationResponse {
    name: String,
}

fn parse_full_handle(full_name: &str) -> Option<RemoteProject> {
    let (account, project) = full_name.split_once('/')?;
    if account.is_empty() || project.is_empty() {
        return None;
    }
    Some(RemoteProject {
        account: account.to_string(),
        project: project.to_string(),
    })
}

fn remote_error(operation: &'static str, source: impl std::fmt::Display) -> Error {
    Error::Remote {
        provider: PROVIDER_NAME,
        operation,
        message: source.to_string(),
    }
}

fn is_already_exists_message(message: &str) -> bool {
    message.to_ascii_lowercase().contains("already exist")
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::thread;

    use tempfile::TempDir;

    use super::*;
    use crate::tuist::test_env::ENV_LOCK;

    struct TokenGuard;

    impl TokenGuard {
        fn acquire() -> (Self, std::sync::MutexGuard<'static, ()>) {
            let guard = ENV_LOCK.lock().unwrap();
            std::env::set_var("TUIST_TOKEN", "test-token");
            (Self, guard)
        }
    }

    impl Drop for TokenGuard {
        fn drop(&mut self) {
            std::env::remove_var("TUIST_TOKEN");
        }
    }

    struct OneShotHttpServer {
        base_url: String,
        request: std::sync::Arc<Mutex<String>>,
        join_handle: Option<thread::JoinHandle<()>>,
    }

    impl OneShotHttpServer {
        fn new(status: u16, body: &str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let request = std::sync::Arc::new(Mutex::new(String::new()));
            let request_for_thread = request.clone();
            let body = body.to_string();
            let join_handle = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let captured = read_http_request(&mut stream);
                *request_for_thread.lock().unwrap() = captured;
                let reason = if status == 200 { "OK" } else { "ERROR" };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
            });
            Self {
                base_url: format!("http://{addr}"),
                request,
                join_handle: Some(join_handle),
            }
        }

        fn base_url(&self) -> String {
            self.base_url.clone()
        }

        fn request(mut self) -> String {
            if let Some(handle) = self.join_handle.take() {
                handle.join().unwrap();
            }
            self.request.lock().unwrap().clone()
        }
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 1024];
        loop {
            match stream.read(&mut chunk) {
                Ok(read) if read > 0 => {
                    buffer.extend_from_slice(&chunk[..read]);
                    if let Some(header_end) =
                        buffer.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        let header_end = header_end + 4;
                        let content_length = String::from_utf8_lossy(&buffer[..header_end])
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(str::trim)
                                    .and_then(|value| value.parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if buffer.len() >= header_end + content_length {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
        String::from_utf8_lossy(&buffer).into_owned()
    }

    fn config(url: String) -> TuistCacheConfig {
        TuistCacheConfig {
            url,
            account: Some("acme".to_string()),
            project: Some("app".to_string()),
            oauth_client_id: None,
            provider_name: "tuist".to_string(),
        }
    }

    fn run<F: std::future::Future<Output = ()>>(future: F) {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
            .block_on(future);
    }

    #[test]
    fn list_projects_parses_full_names() {
        let (_guard, _lock) = TokenGuard::acquire();
        run(async {
            let server = OneShotHttpServer::new(
                200,
                r#"{"projects":[{"full_name":"acme/app"},{"full_name":"acme/api"}]}"#,
            );
            let projects =
                TuistProjects::new(TempDir::new().unwrap().path(), config(server.base_url()))
                    .unwrap();

            let listed = projects.list_projects().await.unwrap();

            assert_eq!(
                listed,
                vec![
                    RemoteProject {
                        account: "acme".to_string(),
                        project: "app".to_string(),
                    },
                    RemoteProject {
                        account: "acme".to_string(),
                        project: "api".to_string(),
                    },
                ]
            );
            let request = server.request();
            assert!(request.starts_with("GET /api/projects HTTP/1.1"));
            assert!(request.contains("authorization: Bearer test-token"));
        });
    }

    #[test]
    fn create_project_sends_full_handle() {
        let (_guard, _lock) = TokenGuard::acquire();
        run(async {
            let server = OneShotHttpServer::new(200, r#"{"full_name":"acme/new"}"#);
            let projects =
                TuistProjects::new(TempDir::new().unwrap().path(), config(server.base_url()))
                    .unwrap();

            let created = projects.create_project(Some("acme"), "new").await.unwrap();

            assert_eq!(created.full_handle(), "acme/new");
            let request = server.request();
            assert!(request.starts_with("POST /api/projects HTTP/1.1"));
            assert!(request.contains("\"full_handle\":\"acme/new\""));
        });
    }

    #[test]
    fn create_project_without_account_sends_name() {
        let (_guard, _lock) = TokenGuard::acquire();
        run(async {
            let server = OneShotHttpServer::new(200, r#"{"full_name":"personal/new"}"#);
            let projects =
                TuistProjects::new(TempDir::new().unwrap().path(), config(server.base_url()))
                    .unwrap();

            let created = projects.create_project(None, "new").await.unwrap();

            assert_eq!(created.account, "personal");
            let request = server.request();
            assert!(request.contains("\"name\":\"new\""));
            assert!(!request.contains("full_handle"));
        });
    }

    #[test]
    fn create_project_surfaces_provider_error() {
        let (_guard, _lock) = TokenGuard::acquire();
        run(async {
            let server = OneShotHttpServer::new(400, r#"{"message":"Project already exists."}"#);
            let projects =
                TuistProjects::new(TempDir::new().unwrap().path(), config(server.base_url()))
                    .unwrap();

            let error = projects
                .create_project(Some("acme"), "new")
                .await
                .unwrap_err();

            assert!(error.to_string().contains("already exists"));
        });
    }

    #[test]
    fn list_organizations_parses_names() {
        let (_guard, _lock) = TokenGuard::acquire();
        run(async {
            let server = OneShotHttpServer::new(
                200,
                r#"{"organizations":[{"name":"acme"},{"name":"tuist"}]}"#,
            );
            let projects =
                TuistProjects::new(TempDir::new().unwrap().path(), config(server.base_url()))
                    .unwrap();

            let organizations = projects.list_organizations().await.unwrap();

            assert_eq!(organizations, vec!["acme".to_string(), "tuist".to_string()]);
        });
    }
}
