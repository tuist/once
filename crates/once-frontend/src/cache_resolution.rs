use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::{
    load_infrastructure_config, CacheProviderConfig, Error, InfrastructureProviderConfig,
    NamedCacheProviderConfig, Result, TuistCacheProviderConfig, DEFAULT_TUIST_URL,
};

mod tuist_swift;
mod user;

use user::{load_user_config, maybe_load_user_config, take_named_user_provider, UserCacheProvider};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedCacheProviderConfig {
    Local,
    Tuist(ResolvedTuistCacheProviderConfig),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTuistCacheProviderConfig {
    pub provider_name: String,
    pub url: String,
    pub account: Option<String>,
    pub project: Option<String>,
    pub oauth_client_id: Option<String>,
}

pub fn resolve_cache_provider(
    workspace: &Path,
    user_config_path: &Path,
    provider_override: Option<String>,
) -> Result<ResolvedCacheProviderConfig> {
    let workspace_config = load_infrastructure_config(workspace)?;
    if let Some(config) = cache_provider_config_from_override(provider_override) {
        return resolve_provider_config(user_config_path, &workspace_config.providers, config);
    }
    if let Some(config) = workspace_config.cache {
        return resolve_provider_config(user_config_path, &workspace_config.providers, config);
    }

    if let Some(config) = resolve_tuist_workspace_config(workspace)? {
        Ok(ResolvedCacheProviderConfig::Tuist(config))
    } else {
        resolve_default_provider(user_config_path)
    }
}

fn cache_provider_config_from_override(value: Option<String>) -> Option<CacheProviderConfig> {
    let name = non_empty(value)?;
    if name == "local" {
        Some(CacheProviderConfig::Local)
    } else {
        Some(CacheProviderConfig::Named(NamedCacheProviderConfig {
            name,
            account: None,
            project: None,
        }))
    }
}

fn resolve_provider_config(
    user_config_path: &Path,
    workspace_providers: &BTreeMap<String, InfrastructureProviderConfig>,
    config: CacheProviderConfig,
) -> Result<ResolvedCacheProviderConfig> {
    match config {
        CacheProviderConfig::Local => Ok(ResolvedCacheProviderConfig::Local),
        CacheProviderConfig::Tuist(config) => Ok(ResolvedCacheProviderConfig::Tuist(
            direct_tuist_config(config),
        )),
        CacheProviderConfig::Named(binding) => {
            if let Some(provider) = workspace_providers.get(&binding.name) {
                return resolve_named_workspace_provider(binding, provider.clone());
            }
            let mut config = load_user_config(user_config_path)?;
            let provider =
                take_named_user_provider(&mut config, &binding.name).ok_or_else(|| {
                    Error::Eval {
                        path: user_config_path.display().to_string(),
                        message: format!(
                            "infrastructure `{}` was not found in {}",
                            binding.name,
                            user_config_path.display()
                        ),
                    }
                })?;
            Ok(resolve_named_provider(binding, provider))
        }
    }
}

fn resolve_default_provider(user_config_path: &Path) -> Result<ResolvedCacheProviderConfig> {
    let Some(mut config) = maybe_load_user_config(user_config_path)? else {
        return Ok(ResolvedCacheProviderConfig::Local);
    };
    let binding = config
        .infrastructure
        .take()
        .and_then(|infrastructure| infrastructure.cache)
        .or_else(|| config.cache_provider.take());
    let Some(binding) = binding else {
        return Ok(ResolvedCacheProviderConfig::Local);
    };
    let provider =
        take_named_user_provider(&mut config, &binding.name).ok_or_else(|| Error::Eval {
            path: user_config_path.display().to_string(),
            message: format!(
                "infrastructure `{}` was not found in {}",
                binding.name,
                user_config_path.display()
            ),
        })?;
    Ok(resolve_named_provider(binding.into_named(), provider))
}

fn resolve_named_provider(
    binding: NamedCacheProviderConfig,
    provider: UserCacheProvider,
) -> ResolvedCacheProviderConfig {
    let NamedCacheProviderConfig {
        name,
        account: binding_account,
        project: binding_project,
    } = binding;
    match provider {
        UserCacheProvider::Tuist {
            url,
            oauth_client_id,
            account,
            project,
        } => ResolvedCacheProviderConfig::Tuist(ResolvedTuistCacheProviderConfig {
            provider_name: name,
            url: url.unwrap_or_else(|| DEFAULT_TUIST_URL.to_string()),
            account: binding_account.or(account),
            project: binding_project.or(project),
            oauth_client_id,
        }),
    }
}

fn resolve_named_workspace_provider(
    binding: NamedCacheProviderConfig,
    provider: InfrastructureProviderConfig,
) -> Result<ResolvedCacheProviderConfig> {
    let NamedCacheProviderConfig {
        name,
        account: binding_account,
        project: binding_project,
    } = binding;
    match provider {
        InfrastructureProviderConfig::Local => Ok(ResolvedCacheProviderConfig::Local),
        InfrastructureProviderConfig::Microsandbox(_)
        | InfrastructureProviderConfig::E2b(_)
        | InfrastructureProviderConfig::Daytona(_) => Err(Error::Eval {
            path: name.clone(),
            message: format!("infrastructure `{name}` provides execution but not shared caching"),
        }),
        InfrastructureProviderConfig::Tuist(config) => Ok(ResolvedCacheProviderConfig::Tuist(
            ResolvedTuistCacheProviderConfig {
                provider_name: name,
                url: config.url,
                account: binding_account.or(config.account),
                project: binding_project.or(config.project),
                oauth_client_id: config.oauth_client_id,
            },
        )),
    }
}

fn direct_tuist_config(config: TuistCacheProviderConfig) -> ResolvedTuistCacheProviderConfig {
    ResolvedTuistCacheProviderConfig {
        provider_name: "tuist".to_string(),
        url: config.url,
        account: config.account,
        project: config.project,
        oauth_client_id: config.oauth_client_id,
    }
}

fn resolve_tuist_workspace_config(
    workspace: &Path,
) -> Result<Option<ResolvedTuistCacheProviderConfig>> {
    let Some(config) =
        load_tuist_toml_config(workspace).or_else(|| load_tuist_swift_config(workspace))
    else {
        return Ok(None);
    };
    let (account, project) = split_full_handle(&config.full_handle).ok_or_else(|| Error::Eval {
        path: workspace.display().to_string(),
        message: format!(
            "Tuist project handle `{}` must have the form `account/project`",
            config.full_handle
        ),
    })?;
    Ok(Some(ResolvedTuistCacheProviderConfig {
        provider_name: "tuist".to_string(),
        url: config.url.unwrap_or_else(|| DEFAULT_TUIST_URL.to_string()),
        account: Some(account),
        project: Some(project),
        oauth_client_id: None,
    }))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TuistWorkspaceConfig {
    full_handle: String,
    url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TuistTomlConfig {
    project: Option<String>,
    url: Option<String>,
}

fn load_tuist_toml_config(workspace: &Path) -> Option<TuistWorkspaceConfig> {
    let src = std::fs::read_to_string(workspace.join("tuist.toml")).ok()?;
    let config: TuistTomlConfig = toml::from_str(&src).ok()?;
    Some(TuistWorkspaceConfig {
        full_handle: non_empty(config.project)?,
        url: non_empty(config.url),
    })
}

fn load_tuist_swift_config(workspace: &Path) -> Option<TuistWorkspaceConfig> {
    let src = std::fs::read_to_string(workspace.join("Tuist.swift")).ok()?;
    let (full_handle, url) = tuist_swift::parse_tuist_config(&src)?;
    Some(TuistWorkspaceConfig { full_handle, url })
}

fn split_full_handle(full_handle: &str) -> Option<(String, String)> {
    let mut parts = full_handle.split('/');
    let account = non_empty(parts.next().map(str::to_string))?;
    let project = non_empty(parts.next().map(str::to_string))?;
    if parts.next().is_some() {
        return None;
    }
    Some((account, project))
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

#[cfg(test)]
mod tests;
