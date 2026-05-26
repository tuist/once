use serde::Deserialize;

use crate::error::{Error, Result};

pub const DEFAULT_TUIST_URL: &str = "https://tuist.dev";
pub const DEFAULT_TUIST_TOKEN_ENV: &str = "TUIST_TOKEN";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheProviderConfig {
    Local,
    Named(NamedCacheProviderConfig),
    Tuist(TuistCacheProviderConfig),
}

impl Default for CacheProviderConfig {
    fn default() -> Self {
        Self::Local
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedCacheProviderConfig {
    pub name: String,
    pub account: Option<String>,
    pub project: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuistCacheProviderConfig {
    pub url: String,
    pub endpoint: Option<String>,
    pub account: Option<String>,
    pub project: Option<String>,
    pub token_env: String,
    pub oauth_client_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum CacheProviderToml {
    Named(NamedCacheProviderToml),
    Direct(DirectCacheProviderToml),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NamedCacheProviderToml {
    name: String,
    account: Option<String>,
    project: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum DirectCacheProviderToml {
    Local,
    Tuist {
        url: Option<String>,
        endpoint: Option<String>,
        account: Option<String>,
        project: Option<String>,
        token_env: Option<String>,
        oauth_client_id: Option<String>,
    },
}

impl CacheProviderToml {
    pub(crate) fn into_config(self, display_name: &str) -> Result<CacheProviderConfig> {
        match self {
            Self::Named(named) => {
                let name = named.name.trim().to_string();
                if name.is_empty() {
                    return Err(Error::Eval {
                        path: display_name.to_string(),
                        message: "cache_provider name must not be empty".to_string(),
                    });
                }
                Ok(CacheProviderConfig::Named(NamedCacheProviderConfig {
                    name,
                    account: non_empty(named.account),
                    project: non_empty(named.project),
                }))
            }
            Self::Direct(DirectCacheProviderToml::Local) => Ok(CacheProviderConfig::Local),
            Self::Direct(DirectCacheProviderToml::Tuist {
                url,
                endpoint,
                account,
                project,
                token_env,
                oauth_client_id,
            }) => {
                let url = non_empty(url).unwrap_or_else(|| DEFAULT_TUIST_URL.to_string());
                let token_env =
                    non_empty(token_env).unwrap_or_else(|| DEFAULT_TUIST_TOKEN_ENV.to_string());
                if !token_env
                    .chars()
                    .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
                {
                    return Err(Error::Eval {
                        path: display_name.to_string(),
                        message: format!(
                            "cache_provider token_env `{token_env}` must be an uppercase env var name"
                        ),
                    });
                }
                Ok(CacheProviderConfig::Tuist(TuistCacheProviderConfig {
                    url,
                    endpoint: non_empty(endpoint),
                    account: non_empty(account),
                    project: non_empty(project),
                    token_env,
                    oauth_client_id: non_empty(oauth_client_id),
                }))
            }
        }
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tuist_config_defaults_url_and_token_env() {
        let config = CacheProviderToml::Direct(DirectCacheProviderToml::Tuist {
            url: None,
            endpoint: None,
            account: Some("acme".to_string()),
            project: Some("app".to_string()),
            token_env: None,
            oauth_client_id: None,
        })
        .into_config("fabrik.toml")
        .unwrap();

        let CacheProviderConfig::Tuist(config) = config else {
            panic!("expected tuist config");
        };
        assert_eq!(config.url, DEFAULT_TUIST_URL);
        assert_eq!(config.token_env, DEFAULT_TUIST_TOKEN_ENV);
        assert_eq!(config.oauth_client_id, None);
        assert_eq!(config.account.as_deref(), Some("acme"));
        assert_eq!(config.project.as_deref(), Some("app"));
    }

    #[test]
    fn rejects_invalid_token_env_name() {
        let err = CacheProviderToml::Direct(DirectCacheProviderToml::Tuist {
            url: None,
            endpoint: None,
            account: None,
            project: None,
            token_env: Some("tuist-token".to_string()),
            oauth_client_id: None,
        })
        .into_config("fabrik.toml")
        .unwrap_err();

        assert!(err
            .to_string()
            .contains("cache_provider token_env `tuist-token`"));
    }

    #[test]
    fn named_config_keeps_scope_in_workspace() {
        let config = CacheProviderToml::Named(NamedCacheProviderToml {
            name: "tuist".to_string(),
            account: Some("acme".to_string()),
            project: Some("app".to_string()),
        })
        .into_config("fabrik.toml")
        .unwrap();

        assert_eq!(
            config,
            CacheProviderConfig::Named(NamedCacheProviderConfig {
                name: "tuist".to_string(),
                account: Some("acme".to_string()),
                project: Some("app".to_string()),
            })
        );
    }
}
