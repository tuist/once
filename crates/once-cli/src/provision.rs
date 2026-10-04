//! Provider-neutral project provisioning for `once connect`.
//!
//! Once resolves an infrastructure provider by name and asks it to create or
//! list projects. The provider owns every protocol detail; this module only
//! defines the shape the CLI and the manifest writer consume, and maps a
//! resolved provider config onto a concrete provisioner. It lives in the CLI
//! rather than the cache crate so provisioning never leaks into the `once` SDK.

use std::path::Path;

use anyhow::{bail, Context, Result};
use once_cas::{ProjectCreateError, TuistProjects};
use once_core::Xdg;

use crate::cache_provider::{self, ResolvedCacheProviderConfig};

/// Provider-neutral failure while creating a project.
#[derive(Debug, thiserror::Error)]
pub enum CreateProjectError {
    /// The provider already has a project with that handle.
    #[error("the provider already has a project named `{0}`")]
    AlreadyExists(String),
    /// The connected session is authenticated but not allowed to create projects.
    #[error("the connected session is not allowed to create projects")]
    PermissionDenied,
    /// Any other provider or transport failure.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// A project owned by an infrastructure provider.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProjectRef {
    /// Account or organization handle that owns the project.
    pub account: String,
    /// Project handle within the account.
    pub project: String,
}

impl ProjectRef {
    /// `account/project`, the provider's portable project identifier.
    pub fn full_handle(&self) -> String {
        format!("{}/{}", self.account, self.project)
    }

    /// Provider-neutral handle parsed from `account/project`.
    pub fn parse(full_handle: &str) -> Option<Self> {
        let (account, project) = full_handle.split_once('/')?;
        if account.trim().is_empty() || project.trim().is_empty() {
            return None;
        }
        Some(Self {
            account: account.to_string(),
            project: project.to_string(),
        })
    }
}

/// A provider that can provision a project and be written into `once.toml`.
///
/// Dispatch is intentionally a closed enum: the only provisioning provider
/// today is Tuist, and pretending otherwise with a trait would hide that a new
/// provider still needs a matching arm here and in the cache resolver.
pub enum Provisioner {
    /// Tuist-hosted or self-hosted server.
    Tuist(TuistProjects),
}

impl Provisioner {
    /// Instance name the provider is known by in `once.toml`.
    pub fn provider_name(&self) -> &str {
        match self {
            Self::Tuist(projects) => projects.provider_name(),
        }
    }

    /// Provider kind written as `kind = "<kind>"` in `once.toml`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Tuist(_) => "tuist",
        }
    }

    /// Server URL the binding records.
    pub fn server_url(&self) -> &str {
        match self {
            Self::Tuist(projects) => projects.server_url(),
        }
    }

    /// OAuth client id the binding should preserve, when the provider pins one.
    pub fn oauth_client_id(&self) -> Option<&str> {
        match self {
            Self::Tuist(projects) => projects.oauth_client_id(),
        }
    }

    /// Whether the provider already has a reusable session for this machine.
    pub fn has_session(&self) -> bool {
        match self {
            Self::Tuist(projects) => projects.has_session(),
        }
    }

    /// Projects the stored session can see.
    pub async fn list_projects(&self) -> Result<Vec<ProjectRef>> {
        match self {
            Self::Tuist(projects) => projects
                .list_projects()
                .await
                .map(|projects| {
                    projects
                        .into_iter()
                        .map(|project| ProjectRef {
                            account: project.account,
                            project: project.project,
                        })
                        .collect()
                })
                .context("listing projects"),
        }
    }

    /// Create `account/project`. When `account` is `None` the provider chooses
    /// the account that owns the session, such as the user's personal account.
    pub async fn create_project(
        &self,
        account: Option<&str>,
        project: &str,
    ) -> std::result::Result<ProjectRef, CreateProjectError> {
        match self {
            Self::Tuist(projects) => projects
                .create_project(account, project)
                .await
                .map(|project| ProjectRef {
                    account: project.account,
                    project: project.project,
                })
                .map_err(|error| match error {
                    ProjectCreateError::AlreadyExists => {
                        CreateProjectError::AlreadyExists(project.to_string())
                    }
                    ProjectCreateError::PermissionDenied => CreateProjectError::PermissionDenied,
                    ProjectCreateError::Other(error) => CreateProjectError::Other(
                        anyhow::Error::new(error).context("creating project"),
                    ),
                }),
        }
    }
}

/// Resolve a named provider into a provisioner.
///
/// Reuses the same provider resolution as `once auth login`, so a workspace
/// provider, a named user provider, or the built-in provider all work here with
/// the same credentials.
pub fn resolve(workspace: &Path, xdg: &Xdg, provider: &str) -> Result<Provisioner> {
    match resolve_provider_config(workspace, xdg, provider)? {
        ResolvedCacheProviderConfig::Local => bail!(
            "provider `{provider}` provides a local cache and cannot create projects. \
             Choose a project-hosting provider, for example `--provider tuist`."
        ),
        ResolvedCacheProviderConfig::Tuist(config) => Ok(Provisioner::Tuist(TuistProjects::new(
            cache_provider::credentials_root(xdg),
            config,
        )?)),
    }
}

/// Resolve the provider config connect should use.
///
/// For the built-in `tuist` provider, prefer the server and client id already
/// discovered for the workspace from `Tuist.swift` or `tuist.toml`. Without
/// this, connect would retarget a self-hosted workspace at the public server
/// and write a binding that then takes precedence over those files.
fn resolve_provider_config(
    workspace: &Path,
    xdg: &Xdg,
    provider: &str,
) -> Result<ResolvedCacheProviderConfig> {
    if provider.trim() == "tuist" {
        if let Ok(ResolvedCacheProviderConfig::Tuist(config)) =
            cache_provider::resolve_config(workspace, xdg)
        {
            if config.provider_name == "tuist" {
                return Ok(ResolvedCacheProviderConfig::Tuist(config));
            }
        }
    }
    cache_provider::resolve_auth_provider(workspace, xdg, provider)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_handle_round_trips() {
        let project = ProjectRef::parse("acme/app").expect("parse");
        assert_eq!(project.full_handle(), "acme/app");
    }

    #[test]
    fn parse_rejects_missing_parts() {
        assert!(ProjectRef::parse("acme").is_none());
        assert!(ProjectRef::parse("/app").is_none());
        assert!(ProjectRef::parse("acme/").is_none());
    }
}
