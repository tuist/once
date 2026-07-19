use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::{Error, NamedCacheProviderConfig, Result};

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct UserConfig {
    pub(super) infrastructure: Option<UserInfrastructureConfig>,
    pub(super) infrastructures: BTreeMap<String, UserCacheProvider>,
    pub(super) cache_provider: Option<UserCacheProviderBinding>,
    pub(super) cache_providers: BTreeMap<String, UserCacheProvider>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct UserInfrastructureConfig {
    pub(super) cache: Option<UserCacheProviderBinding>,
    execution: Option<UserCacheProviderBinding>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UserCacheProviderBinding {
    #[serde(alias = "provider")]
    pub(super) name: String,
    account: Option<String>,
    project: Option<String>,
}

impl UserCacheProviderBinding {
    pub(super) fn into_named(self) -> NamedCacheProviderConfig {
        NamedCacheProviderConfig {
            name: self.name,
            account: self.account,
            project: self.project,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum UserCacheProvider {
    Tuist {
        url: Option<String>,
        oauth_client_id: Option<String>,
        account: Option<String>,
        project: Option<String>,
    },
}

pub(super) fn take_named_user_provider(
    config: &mut UserConfig,
    name: &str,
) -> Option<UserCacheProvider> {
    config
        .infrastructures
        .remove(name)
        .or_else(|| config.cache_providers.remove(name))
}

pub(super) fn load_user_config(path: &Path) -> Result<UserConfig> {
    maybe_load_user_config(path)?.ok_or_else(|| Error::Read {
        path: path.display().to_string(),
        source: std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "user cache configuration does not exist",
        ),
    })
}

pub(super) fn maybe_load_user_config(path: &Path) -> Result<Option<UserConfig>> {
    let src = match std::fs::read_to_string(path) {
        Ok(src) => src,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(Error::Read {
                path: path.display().to_string(),
                source,
            });
        }
    };
    let mut config: UserConfig = toml::from_str(&src).map_err(|source| Error::Parse {
        path: path.display().to_string(),
        message: source.to_string(),
    })?;
    if let Some(binding) = config.cache_provider.as_mut() {
        normalize_binding(binding, path)?;
    }
    if let Some(infrastructure) = config.infrastructure.as_mut() {
        if let Some(binding) = infrastructure.cache.as_mut() {
            normalize_binding(binding, path)?;
        }
        if let Some(binding) = infrastructure.execution.as_mut() {
            normalize_binding(binding, path)?;
        }
    }
    Ok(Some(config))
}

fn normalize_binding(binding: &mut UserCacheProviderBinding, path: &Path) -> Result<()> {
    binding.name = binding.name.trim().to_string();
    if binding.name.is_empty() {
        return Err(Error::Eval {
            path: path.display().to_string(),
            message: "user cache configuration has an empty infrastructure name".to_string(),
        });
    }
    binding.account = non_empty(binding.account.take());
    binding.project = non_empty(binding.project.take());
    Ok(())
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}
