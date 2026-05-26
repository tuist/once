use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use fabrik_cas::{CacheProvider, TuistCacheConfig, TUIST_OAUTH_CLIENT_ID_ENV};
use fabrik_core::Xdg;
use fabrik_frontend::{
    CacheProviderConfig, NamedCacheProviderConfig, TuistCacheProviderConfig,
    DEFAULT_TUIST_TOKEN_ENV, DEFAULT_TUIST_URL,
};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedCacheProviderConfig {
    Local,
    Tuist(TuistCacheConfig),
}

pub fn resolve(workspace: &Path, xdg: &Xdg) -> Result<CacheProvider> {
    build_provider(xdg, resolve_config(workspace, xdg)?)
}

pub(crate) fn resolve_auth_provider(
    workspace: &Path,
    xdg: &Xdg,
    provider_name: &str,
) -> Result<ResolvedCacheProviderConfig> {
    let provider_name = provider_name.trim();
    if provider_name.is_empty() {
        bail!("auth provider name must not be empty");
    }
    if provider_name == "workspace" {
        return resolve_config(workspace, xdg);
    }

    if let Some(config) = maybe_load_user_config(xdg)? {
        if let Some(provider) = config.cache_providers.get(provider_name) {
            return Ok(resolve_named_provider(
                provider_name.to_string(),
                NamedCacheProviderConfig {
                    name: provider_name.to_string(),
                    account: None,
                    project: None,
                },
                provider,
            ));
        }
    }

    if provider_name == "tuist" {
        return Ok(ResolvedCacheProviderConfig::Tuist(default_tuist_config(
            provider_name.to_string(),
            DEFAULT_TUIST_URL.to_string(),
            None,
            None,
            None,
            DEFAULT_TUIST_TOKEN_ENV.to_string(),
            None,
        )));
    }

    bail!(
        "auth provider `{provider_name}` was not found in {}",
        user_config_path(xdg).display()
    )
}

pub(crate) fn credentials_root(xdg: &Xdg) -> PathBuf {
    xdg.config_home.join("fabrik").join("credentials")
}

fn resolve_config(workspace: &Path, xdg: &Xdg) -> Result<ResolvedCacheProviderConfig> {
    match fabrik_frontend::load_cache_provider_override(workspace)
        .context("loading cache provider")?
    {
        Some(config) => resolve_provider_config(xdg, config),
        None => resolve_default_provider(xdg),
    }
}

fn build_provider(xdg: &Xdg, config: ResolvedCacheProviderConfig) -> Result<CacheProvider> {
    match config {
        ResolvedCacheProviderConfig::Local => Ok(CacheProvider::open_local(xdg.fabrik_cas())),
        ResolvedCacheProviderConfig::Tuist(config) => {
            CacheProvider::tuist(xdg.fabrik_cas(), credentials_root(xdg), config)
                .context("configuring Tuist cache provider")
        }
    }
}

fn resolve_provider_config(
    xdg: &Xdg,
    config: CacheProviderConfig,
) -> Result<ResolvedCacheProviderConfig> {
    match config {
        CacheProviderConfig::Local => Ok(ResolvedCacheProviderConfig::Local),
        CacheProviderConfig::Tuist(config) => Ok(ResolvedCacheProviderConfig::Tuist(
            direct_tuist_config(config),
        )),
        CacheProviderConfig::Named(binding) => {
            let config = load_user_config(xdg)?;
            let provider = config.cache_providers.get(&binding.name).with_context(|| {
                format!(
                    "cache provider `{}` was not found in {}",
                    binding.name,
                    user_config_path(xdg).display()
                )
            })?;
            Ok(resolve_named_provider(
                binding.name.clone(),
                binding,
                provider,
            ))
        }
    }
}

fn resolve_default_provider(xdg: &Xdg) -> Result<ResolvedCacheProviderConfig> {
    let Some(config) = maybe_load_user_config(xdg)? else {
        return Ok(ResolvedCacheProviderConfig::Local);
    };
    let Some(binding) = config.cache_provider.clone() else {
        return Ok(ResolvedCacheProviderConfig::Local);
    };
    let provider = config.cache_providers.get(&binding.name).with_context(|| {
        format!(
            "cache provider `{}` was not found in {}",
            binding.name,
            user_config_path(xdg).display()
        )
    })?;
    Ok(resolve_named_provider(
        binding.name.clone(),
        binding.into_named(),
        provider,
    ))
}

fn resolve_named_provider(
    provider_name: String,
    binding: NamedCacheProviderConfig,
    provider: &UserCacheProvider,
) -> ResolvedCacheProviderConfig {
    match provider {
        UserCacheProvider::Tuist {
            url,
            endpoint,
            token_env,
            oauth_client_id,
            account,
            project,
        } => ResolvedCacheProviderConfig::Tuist(default_tuist_config(
            provider_name,
            url.clone().unwrap_or_else(|| DEFAULT_TUIST_URL.to_string()),
            endpoint.clone(),
            binding.account.or_else(|| account.clone()),
            binding.project.or_else(|| project.clone()),
            token_env
                .clone()
                .unwrap_or_else(|| DEFAULT_TUIST_TOKEN_ENV.to_string()),
            oauth_client_id.clone(),
        )),
    }
}

fn direct_tuist_config(config: TuistCacheProviderConfig) -> TuistCacheConfig {
    default_tuist_config(
        "tuist".to_string(),
        config.url,
        config.endpoint,
        config.account,
        config.project,
        config.token_env,
        config.oauth_client_id,
    )
}

fn default_tuist_config(
    provider_name: String,
    url: String,
    endpoint: Option<String>,
    account: Option<String>,
    project: Option<String>,
    token_env: String,
    oauth_client_id: Option<String>,
) -> TuistCacheConfig {
    TuistCacheConfig {
        url,
        endpoint,
        account,
        project,
        token_env,
        oauth_client_id: resolve_tuist_oauth_client_id(oauth_client_id),
        provider_name,
    }
}

fn resolve_tuist_oauth_client_id(config_value: Option<String>) -> Option<String> {
    non_empty(std::env::var(TUIST_OAUTH_CLIENT_ID_ENV).ok())
        .or_else(|| config_value.and_then(|value| non_empty(Some(value))))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct UserConfig {
    cache_provider: Option<UserCacheProviderBinding>,
    cache_providers: BTreeMap<String, UserCacheProvider>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct UserCacheProviderBinding {
    name: String,
    account: Option<String>,
    project: Option<String>,
}

impl UserCacheProviderBinding {
    fn into_named(self) -> NamedCacheProviderConfig {
        NamedCacheProviderConfig {
            name: self.name,
            account: self.account,
            project: self.project,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum UserCacheProvider {
    Tuist {
        url: Option<String>,
        endpoint: Option<String>,
        token_env: Option<String>,
        oauth_client_id: Option<String>,
        account: Option<String>,
        project: Option<String>,
    },
}

fn load_user_config(xdg: &Xdg) -> Result<UserConfig> {
    maybe_load_user_config(xdg)?.with_context(|| {
        format!(
            "user cache config {} does not exist",
            user_config_path(xdg).display()
        )
    })
}

fn maybe_load_user_config(xdg: &Xdg) -> Result<Option<UserConfig>> {
    let path = user_config_path(xdg);
    let src = match std::fs::read_to_string(&path) {
        Ok(src) => src,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(source) => {
            return Err(source)
                .with_context(|| format!("reading user cache config {}", path.display()));
        }
    };
    let mut config: UserConfig = toml::from_str(&src)
        .with_context(|| format!("parsing user cache config {}", path.display()))?;
    if let Some(binding) = config.cache_provider.as_mut() {
        binding.name = binding.name.trim().to_string();
        if binding.name.is_empty() {
            bail!(
                "user cache config {} has an empty cache_provider name",
                path.display()
            );
        }
        binding.account = non_empty(binding.account.take());
        binding.project = non_empty(binding.project.take());
    }
    Ok(Some(config))
}

fn user_config_path(xdg: &Xdg) -> PathBuf {
    xdg.config_home.join("fabrik").join("config.toml")
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
    use tempfile::TempDir;

    fn xdg_under(root: &Path) -> Xdg {
        Xdg {
            cache_home: root.join("cache"),
            state_home: root.join("state"),
            data_home: root.join("data"),
            config_home: root.join("config"),
            runtime_dir: root.join("runtime"),
        }
    }

    fn write_workspace(root: &Path, body: &str) {
        std::fs::write(root.join("fabrik.toml"), body).unwrap();
    }

    fn write_user_config(xdg: &Xdg, body: &str) {
        let dir = xdg.config_home.join("fabrik");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), body).unwrap();
    }

    fn expect_tuist(config: ResolvedCacheProviderConfig) -> fabrik_cas::TuistCacheConfig {
        match config {
            ResolvedCacheProviderConfig::Tuist(config) => config,
            ResolvedCacheProviderConfig::Local => panic!("expected tuist cache provider"),
        }
    }

    #[test]
    fn resolve_defaults_to_local_without_workspace_or_user_config() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());

        let provider = resolve_config(tmp.path(), &xdg).unwrap();

        assert_eq!(provider, ResolvedCacheProviderConfig::Local);
    }

    #[test]
    fn resolve_uses_user_default_provider_when_workspace_is_unspecified() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_user_config(
            &xdg,
            r#"
[cache_provider]
name = "tuist"
project = "workspace-app"

[cache_providers.tuist]
kind = "tuist"
url = "https://cache.tuist.dev"
account = "acme"
"#,
        );

        let config = expect_tuist(resolve_config(tmp.path(), &xdg).unwrap());
        assert_eq!(config.url, "https://cache.tuist.dev");
        assert_eq!(config.account.as_deref(), Some("acme"));
        assert_eq!(config.project.as_deref(), Some("workspace-app"));
        assert_eq!(config.token_env, DEFAULT_TUIST_TOKEN_ENV);
        assert_eq!(config.oauth_client_id, None);
        assert_eq!(config.provider_name, "tuist");
    }

    #[test]
    fn explicit_local_workspace_beats_user_default_provider() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_workspace(
            tmp.path(),
            r#"
[cache_provider]
kind = "local"
"#,
        );
        write_user_config(
            &xdg,
            r#"
[cache_provider]
name = "tuist"

[cache_providers.tuist]
kind = "tuist"
account = "acme"
project = "app"
"#,
        );

        let provider = resolve_config(tmp.path(), &xdg).unwrap();
        assert_eq!(provider, ResolvedCacheProviderConfig::Local);
    }

    #[test]
    fn workspace_named_provider_overrides_user_default_scope() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_workspace(
            tmp.path(),
            r#"
[cache_provider]
name = "tuist"
project = "repo-app"
"#,
        );
        write_user_config(
            &xdg,
            r#"
[cache_provider]
name = "tuist"
project = "default-app"

[cache_providers.tuist]
kind = "tuist"
url = "https://cache.tuist.dev"
account = "acme"
"#,
        );

        let config = expect_tuist(resolve_config(tmp.path(), &xdg).unwrap());
        assert_eq!(config.account.as_deref(), Some("acme"));
        assert_eq!(config.project.as_deref(), Some("repo-app"));
    }

    #[test]
    fn resolve_auth_provider_uses_workspace_provider_when_requested() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_workspace(
            tmp.path(),
            r#"
[cache_provider]
kind = "tuist"
url = "https://self-hosted.example.com"
account = "acme"
project = "app"
"#,
        );

        let config = expect_tuist(resolve_auth_provider(tmp.path(), &xdg, "workspace").unwrap());

        assert_eq!(config.url, "https://self-hosted.example.com");
        assert_eq!(config.provider_name, "tuist");
    }

    #[test]
    fn resolve_auth_provider_falls_back_to_named_user_provider() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_user_config(
            &xdg,
            r#"
[cache_providers.acme]
kind = "tuist"
url = "https://cache.acme.dev"
account = "acme"
"#,
        );

        let config = expect_tuist(resolve_auth_provider(tmp.path(), &xdg, "acme").unwrap());

        assert_eq!(config.url, "https://cache.acme.dev");
        assert_eq!(config.provider_name, "acme");
    }

    #[test]
    fn resolve_auth_provider_supports_builtin_tuist() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());

        let config = expect_tuist(resolve_auth_provider(tmp.path(), &xdg, "tuist").unwrap());

        assert_eq!(config.url, DEFAULT_TUIST_URL);
        assert_eq!(config.provider_name, "tuist");
    }
}
