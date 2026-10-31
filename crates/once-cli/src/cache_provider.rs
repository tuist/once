use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use once_cas::{CacheProvider, TuistCacheConfig, TUIST_OAUTH_CLIENT_ID_ENV};
use once_core::RemoteExecution;
use once_core::Xdg;
use once_frontend::{
    ExecutionProviderConfig, InfrastructureConfig, InfrastructureProviderConfig,
    NamedCacheProviderConfig, DEFAULT_TUIST_URL,
};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedCacheProviderConfig {
    Local,
    Tuist(TuistCacheConfig),
}

const DEFAULT_EXECUTION_PROVIDER: &str = "microsandbox";
const ONCE_CACHE_PROVIDER_ENV: &str = "ONCE_CACHE_PROVIDER";
static TRANSFER_BUDGET: std::sync::OnceLock<once_cas::TransferBudget> = std::sync::OnceLock::new();

pub(crate) fn configure_transfer_budget(budget: once_cas::TransferBudget) {
    let _ = TRANSFER_BUDGET.set(budget);
}

pub fn resolve(workspace: &Path, xdg: &Xdg) -> Result<CacheProvider> {
    let provider = build_provider(xdg, resolve_config(workspace, xdg)?)?;
    Ok(match TRANSFER_BUDGET.get() {
        Some(budget) => provider.with_transfer_budget(budget.clone()),
        None => provider,
    })
}

pub(crate) fn resolve_execution(
    workspace: &Path,
    xdg: &Xdg,
    explicit_provider: Option<&str>,
) -> Result<RemoteExecution> {
    let binding = resolve_execution_unresolved(workspace, xdg, explicit_provider)?;
    Ok(bind_environment_override(binding, |name| {
        std::env::var(name).ok()
    }))
}

fn bind_environment_override(
    mut binding: RemoteExecution,
    lookup: impl Fn(&str) -> Option<String>,
) -> RemoteExecution {
    if binding.environment.is_none() {
        let variable = match binding.executor.as_deref().unwrap_or(&binding.provider) {
            "e2b" => Some("ONCE_E2B_TEMPLATE"),
            "microsandbox" => Some("ONCE_MICROSANDBOX_IMAGE"),
            _ => None,
        };
        binding.environment = variable.and_then(lookup).filter(|value| !value.is_empty());
    }
    binding
}

fn resolve_execution_unresolved(
    workspace: &Path,
    xdg: &Xdg,
    explicit_provider: Option<&str>,
) -> Result<RemoteExecution> {
    let workspace_config =
        once_frontend::load_infrastructure_config(workspace).context("loading infrastructure")?;
    if let Some(provider) = explicit_provider.and_then(non_empty_str) {
        let configured = match workspace_config.execution.clone() {
            Some(binding) => Some(binding),
            None => maybe_load_user_config(xdg)?
                .and_then(|config| config.infrastructure)
                .and_then(|infrastructure| infrastructure.execution)
                .map(UserExecutionProviderBinding::into_execution),
        };
        let properties = configured
            .as_ref()
            .filter(|binding| binding.name == provider)
            .map(|binding| binding.properties.clone())
            .unwrap_or_default();
        return resolve_execution_binding(
            xdg,
            &workspace_config,
            ExecutionProviderConfig {
                name: provider.to_string(),
                account: None,
                project: None,
                properties,
            },
        );
    }
    if let Some(binding) = workspace_config.execution.clone() {
        return resolve_execution_binding(xdg, &workspace_config, binding);
    }

    if let Some(mut config) = maybe_load_user_config(xdg)? {
        if let Some(binding) = config
            .infrastructure
            .take()
            .and_then(|infrastructure| infrastructure.execution)
        {
            return resolve_execution_user_binding(xdg, binding.into_execution(), config);
        }
    }

    Ok(RemoteExecution::provider(DEFAULT_EXECUTION_PROVIDER))
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

    let workspace_config =
        once_frontend::load_infrastructure_config(workspace).context("loading infrastructure")?;
    if let Some(provider) = workspace_config.providers.get(provider_name) {
        return resolve_named_workspace_provider(
            NamedCacheProviderConfig {
                name: provider_name.to_string(),
                account: None,
                project: None,
            },
            provider.clone(),
        );
    }

    if let Some(mut config) = maybe_load_user_config(xdg)? {
        if let Some(provider) = take_named_user_provider(&mut config, provider_name) {
            return Ok(resolve_named_provider(
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
        )));
    }

    bail!(
        "auth provider `{provider_name}` was not found in {}",
        user_config_path(xdg).display()
    )
}

pub(crate) fn credentials_root(xdg: &Xdg) -> PathBuf {
    xdg.config_home.join("once").join("credentials")
}

pub(crate) fn resolve_config(workspace: &Path, xdg: &Xdg) -> Result<ResolvedCacheProviderConfig> {
    resolve_config_with_env(workspace, xdg, std::env::var(ONCE_CACHE_PROVIDER_ENV).ok())
}

fn resolve_config_with_env(
    workspace: &Path,
    xdg: &Xdg,
    cache_provider_override: Option<String>,
) -> Result<ResolvedCacheProviderConfig> {
    let config = once_frontend::resolve_cache_provider(
        workspace,
        &user_config_path(xdg),
        cache_provider_override,
    )
    .context("resolving cache provider")?;
    Ok(match config {
        once_frontend::ResolvedCacheProviderConfig::Local => ResolvedCacheProviderConfig::Local,
        once_frontend::ResolvedCacheProviderConfig::Tuist(config) => {
            ResolvedCacheProviderConfig::Tuist(default_tuist_config(
                config.provider_name,
                config.url,
                config.account,
                config.project,
                config.oauth_client_id,
            ))
        }
    })
}

fn build_provider(xdg: &Xdg, config: ResolvedCacheProviderConfig) -> Result<CacheProvider> {
    match config {
        ResolvedCacheProviderConfig::Local => Ok(CacheProvider::open_local(xdg.once_cas())),
        ResolvedCacheProviderConfig::Tuist(config) => {
            CacheProvider::tuist(xdg.once_cas(), credentials_root(xdg), config)
                .context("configuring Tuist cache provider")
        }
    }
}

fn take_named_user_provider(config: &mut UserConfig, name: &str) -> Option<UserCacheProvider> {
    config
        .infrastructures
        .remove(name)
        .or_else(|| config.cache_providers.remove(name))
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
        } => ResolvedCacheProviderConfig::Tuist(default_tuist_config(
            name,
            url.unwrap_or_else(|| DEFAULT_TUIST_URL.to_string()),
            binding_account.or(account),
            binding_project.or(project),
            oauth_client_id,
        )),
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
        | InfrastructureProviderConfig::Daytona(_)
        | InfrastructureProviderConfig::Docker(_) => {
            bail!("infrastructure `{name}` provides execution but not shared caching")
        }
        InfrastructureProviderConfig::Tuist(config) => {
            Ok(ResolvedCacheProviderConfig::Tuist(default_tuist_config(
                name,
                config.url,
                binding_account.or(config.account),
                binding_project.or(config.project),
                config.oauth_client_id,
            )))
        }
    }
}

fn resolve_execution_binding(
    xdg: &Xdg,
    workspace_config: &InfrastructureConfig,
    binding: ExecutionProviderConfig,
) -> Result<RemoteExecution> {
    if let Some(provider) = workspace_config.providers.get(&binding.name) {
        return Ok(remote_from_workspace_provider(binding, provider.clone()));
    }

    if let Some(config) = maybe_load_user_config(xdg)? {
        return resolve_execution_user_binding(xdg, binding, config);
    }

    if is_builtin_execution_provider(&binding.name) {
        return Ok(remote_from_binding(binding));
    }

    bail!(
        "infrastructure `{}` was not found in {}",
        binding.name,
        user_config_path(xdg).display()
    )
}

fn resolve_execution_user_binding(
    xdg: &Xdg,
    binding: ExecutionProviderConfig,
    mut config: UserConfig,
) -> Result<RemoteExecution> {
    if let Some(provider) = take_named_user_provider(&mut config, &binding.name) {
        return Ok(remote_from_user_provider(binding, provider));
    }
    if is_builtin_execution_provider(&binding.name) {
        return Ok(remote_from_binding(binding));
    }
    bail!(
        "infrastructure `{}` was not found in {}",
        binding.name,
        user_config_path(xdg).display()
    )
}

fn remote_from_workspace_provider(
    binding: ExecutionProviderConfig,
    provider: InfrastructureProviderConfig,
) -> RemoteExecution {
    let ExecutionProviderConfig {
        name,
        account: binding_account,
        project: binding_project,
        properties,
    } = binding;
    match provider {
        InfrastructureProviderConfig::Local => RemoteExecution {
            provider: name,
            properties,
            executor: Some("local".to_string()),
            environment: None,
            account: binding_account,
            project: binding_project,
        },
        InfrastructureProviderConfig::Microsandbox(config) => RemoteExecution {
            provider: name,
            properties,
            executor: Some("microsandbox".to_string()),
            environment: Some(config.image),
            account: binding_account,
            project: binding_project,
        },
        InfrastructureProviderConfig::E2b(config) => RemoteExecution {
            provider: name,
            properties,
            executor: Some("e2b".to_string()),
            environment: Some(config.template),
            account: binding_account,
            project: binding_project,
        },
        InfrastructureProviderConfig::Daytona(config) => RemoteExecution {
            provider: name,
            properties,
            executor: Some("daytona".to_string()),
            environment: Some(config.image),
            account: binding_account,
            project: binding_project,
        },
        InfrastructureProviderConfig::Docker(config) => RemoteExecution {
            provider: name,
            properties,
            executor: Some("docker".to_string()),
            environment: Some(config.image),
            account: binding_account,
            project: binding_project,
        },
        InfrastructureProviderConfig::Tuist(config) => RemoteExecution {
            provider: name,
            properties,
            executor: Some("tuist".to_string()),
            environment: None,
            account: binding_account.or(config.account),
            project: binding_project.or(config.project),
        },
    }
}

fn remote_from_user_provider(
    binding: ExecutionProviderConfig,
    provider: UserCacheProvider,
) -> RemoteExecution {
    let ExecutionProviderConfig {
        name,
        account: binding_account,
        project: binding_project,
        properties,
    } = binding;
    match provider {
        UserCacheProvider::Tuist {
            account, project, ..
        } => RemoteExecution {
            provider: name,
            properties,
            executor: Some("tuist".to_string()),
            environment: None,
            account: binding_account.or(account),
            project: binding_project.or(project),
        },
    }
}

fn remote_from_binding(binding: ExecutionProviderConfig) -> RemoteExecution {
    RemoteExecution {
        provider: binding.name,
        properties: binding.properties,
        executor: None,
        environment: None,
        account: binding.account,
        project: binding.project,
    }
}

fn is_builtin_execution_provider(name: &str) -> bool {
    matches!(name, "microsandbox" | "e2b" | "daytona" | "docker")
}

fn default_tuist_config(
    provider_name: String,
    url: String,
    account: Option<String>,
    project: Option<String>,
    oauth_client_id: Option<String>,
) -> TuistCacheConfig {
    TuistCacheConfig {
        url,
        account,
        project,
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
    infrastructure: Option<UserInfrastructureConfig>,
    infrastructures: BTreeMap<String, UserCacheProvider>,
    cache_provider: Option<UserCacheProviderBinding>,
    cache_providers: BTreeMap<String, UserCacheProvider>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct UserInfrastructureConfig {
    cache: Option<UserCacheProviderBinding>,
    execution: Option<UserExecutionProviderBinding>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct UserExecutionProviderBinding {
    #[serde(alias = "provider")]
    name: String,
    account: Option<String>,
    project: Option<String>,
    #[serde(default)]
    properties: BTreeMap<String, serde_json::Value>,
}

impl UserExecutionProviderBinding {
    fn into_execution(self) -> ExecutionProviderConfig {
        ExecutionProviderConfig {
            name: self.name,
            account: self.account,
            project: self.project,
            properties: self.properties,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct UserCacheProviderBinding {
    #[serde(alias = "provider")]
    name: String,
    account: Option<String>,
    project: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum UserCacheProvider {
    Tuist {
        url: Option<String>,
        oauth_client_id: Option<String>,
        account: Option<String>,
        project: Option<String>,
    },
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
        normalize_binding(binding, &path)?;
    }
    if let Some(binding) = config
        .infrastructure
        .as_mut()
        .and_then(|infrastructure| infrastructure.cache.as_mut())
    {
        normalize_binding(binding, &path)?;
    }
    if let Some(binding) = config
        .infrastructure
        .as_mut()
        .and_then(|infrastructure| infrastructure.execution.as_mut())
    {
        let mut named = UserCacheProviderBinding {
            name: binding.name.clone(),
            account: binding.account.take(),
            project: binding.project.take(),
        };
        normalize_binding(&mut named, &path)?;
        binding.name = named.name;
        binding.account = named.account;
        binding.project = named.project;
    }
    Ok(Some(config))
}

fn normalize_binding(binding: &mut UserCacheProviderBinding, path: &Path) -> Result<()> {
    binding.name = binding.name.trim().to_string();
    if binding.name.is_empty() {
        bail!(
            "user cache config {} has an empty infrastructure name",
            path.display()
        );
    }
    binding.account = non_empty(binding.account.take());
    binding.project = non_empty(binding.project.take());
    Ok(())
}

fn user_config_path(xdg: &Xdg) -> PathBuf {
    xdg.config_home.join("once").join("config.toml")
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

fn non_empty_str(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// The Tuist account this workspace's cache is configured against.
///
/// Read from the resolved cache provider config so the live reporter's
/// gRPC target and the credentials it uses are the same the cache leg
/// of the workspace is already using. Returns `None` when the workspace
/// has no Tuist cache provider configured.
pub fn account(workspace: &std::path::Path) -> Option<String> {
    let xdg = once_core::Xdg::from_env();
    match resolve_config(workspace, &xdg).ok()? {
        ResolvedCacheProviderConfig::Tuist(config) => config.account,
        ResolvedCacheProviderConfig::Local => None,
    }
}

/// The Tuist project this workspace's cache is configured against.
///
/// Mirrors [`account`] - same resolution path, same `None` meaning.
pub fn project(workspace: &std::path::Path) -> Option<String> {
    let xdg = once_core::Xdg::from_env();
    match resolve_config(workspace, &xdg).ok()? {
        ResolvedCacheProviderConfig::Tuist(config) => config.project,
        ResolvedCacheProviderConfig::Local => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn provider_environment_overrides_are_bound_before_cache_identity() {
        for (adapter, variable) in [
            ("e2b", "ONCE_E2B_TEMPLATE"),
            ("microsandbox", "ONCE_MICROSANDBOX_IMAGE"),
        ] {
            let mut binding = RemoteExecution::provider("named-fleet");
            binding.executor = Some(adapter.into());
            let first = bind_environment_override(binding.clone(), |name| {
                assert_eq!(name, variable);
                Some("environment-v1".into())
            });
            let second = bind_environment_override(binding, |_| Some("environment-v2".into()));
            assert_ne!(first, second);
            assert_eq!(first.environment.as_deref(), Some("environment-v1"));
        }
    }

    #[test]
    fn empty_provider_environment_override_is_unset() {
        for provider in ["e2b", "microsandbox"] {
            let binding = RemoteExecution::provider(provider);
            assert_eq!(
                bind_environment_override(binding.clone(), |_| Some(String::new())),
                binding
            );
        }
    }

    #[test]
    fn explicit_environment_and_native_bindings_ignore_provider_defaults() {
        let mut binding = RemoteExecution::provider("e2b");
        binding.environment = Some("explicit-environment".into());
        assert_eq!(
            bind_environment_override(binding.clone(), |_| panic!("must not read defaults")),
            binding
        );
        let local = RemoteExecution::provider("local");
        assert_eq!(
            bind_environment_override(local.clone(), |_| panic!(
                "native execution has no template"
            )),
            local
        );
    }

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
        std::fs::write(root.join("once.toml"), body).unwrap();
    }

    fn write_tuist_swift(root: &Path, body: &str) {
        std::fs::write(root.join("Tuist.swift"), body).unwrap();
    }

    fn write_tuist_toml(root: &Path, body: &str) {
        std::fs::write(root.join("tuist.toml"), body).unwrap();
    }

    fn write_user_config(xdg: &Xdg, body: &str) {
        let dir = xdg.config_home.join("once");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), body).unwrap();
    }

    fn expect_tuist(config: ResolvedCacheProviderConfig) -> once_cas::TuistCacheConfig {
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
    fn resolve_uses_user_default_infrastructure_when_workspace_is_unspecified() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_user_config(
            &xdg,
            r#"
[infrastructure.cache]
name = "tuist"
project = "workspace-app"

[infrastructures.tuist]
kind = "tuist"
url = "https://cache.tuist.dev"
account = "acme"
"#,
        );

        let config = expect_tuist(resolve_config(tmp.path(), &xdg).unwrap());
        assert_eq!(config.url, "https://cache.tuist.dev");
        assert_eq!(config.account.as_deref(), Some("acme"));
        assert_eq!(config.project.as_deref(), Some("workspace-app"));
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

[infrastructures.tuist]
kind = "tuist"
account = "acme"
"#,
        );

        let provider = resolve_config(tmp.path(), &xdg).unwrap();
        assert_eq!(provider, ResolvedCacheProviderConfig::Local);
    }

    #[test]
    fn env_cache_provider_override_beats_workspace_config() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_workspace(
            tmp.path(),
            r#"
[infrastructure.cache]
provider = "tuist"

[infrastructures.tuist]
kind = "tuist"
account = "acme"
project = "repo-app"
"#,
        );

        let provider =
            resolve_config_with_env(tmp.path(), &xdg, Some("local".to_string())).unwrap();

        assert_eq!(provider, ResolvedCacheProviderConfig::Local);
    }

    #[test]
    fn env_cache_provider_override_resolves_named_workspace_provider() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_workspace(
            tmp.path(),
            r#"
[infrastructure.cache]
provider = "local"

[infrastructures.tuist]
kind = "tuist"
url = "https://cache.tuist.dev"
account = "acme"
project = "repo-app"
"#,
        );

        let config = expect_tuist(
            resolve_config_with_env(tmp.path(), &xdg, Some("tuist".to_string())).unwrap(),
        );

        assert_eq!(config.url, "https://cache.tuist.dev");
        assert_eq!(config.account.as_deref(), Some("acme"));
        assert_eq!(config.project.as_deref(), Some("repo-app"));
        assert_eq!(config.provider_name, "tuist");
    }

    #[test]
    fn explicit_local_workspace_beats_tuist_workspace_config() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_workspace(
            tmp.path(),
            r#"
[cache_provider]
kind = "local"
"#,
        );
        write_tuist_swift(
            tmp.path(),
            r#"
import ProjectDescription

let tuist = Tuist(
    fullHandle: "tuist/app",
    url: "https://canary.tuist.dev",
    project: .xcode()
)
"#,
        );

        let provider = resolve_config(tmp.path(), &xdg).unwrap();
        assert_eq!(provider, ResolvedCacheProviderConfig::Local);
    }

    #[test]
    fn resolve_uses_tuist_swift_when_workspace_is_unspecified() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_tuist_swift(
            tmp.path(),
            r#"
import ProjectDescription

let tuist = Tuist(
    fullHandle: "tuist/app",
    url: "https://canary.tuist.dev",
    project: .xcode()
)
"#,
        );

        let config = expect_tuist(resolve_config(tmp.path(), &xdg).unwrap());
        assert_eq!(config.url, "https://canary.tuist.dev");
        assert_eq!(config.account.as_deref(), Some("tuist"));
        assert_eq!(config.project.as_deref(), Some("app"));
        assert_eq!(config.provider_name, "tuist");
    }

    #[test]
    fn resolve_uses_tuist_swift_default_url() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_tuist_swift(
            tmp.path(),
            r#"
import ProjectDescription

let tuist = Tuist(
    fullHandle: "tuist/app",
    project: .xcode()
)
"#,
        );

        let config = expect_tuist(resolve_config(tmp.path(), &xdg).unwrap());
        assert_eq!(config.url, DEFAULT_TUIST_URL);
        assert_eq!(config.account.as_deref(), Some("tuist"));
        assert_eq!(config.project.as_deref(), Some("app"));
    }

    #[test]
    fn resolve_ignores_commented_tuist_swift_handle() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_tuist_swift(
            tmp.path(),
            r#"
import ProjectDescription

let tuist = Tuist(
    // fullHandle: "{account_handle}/{project_handle}",
    project: .xcode()
)
"#,
        );

        let provider = resolve_config(tmp.path(), &xdg).unwrap();
        assert_eq!(provider, ResolvedCacheProviderConfig::Local);
    }

    #[test]
    fn resolve_reads_tuist_swift_values_from_tuist_constructor_only() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_tuist_swift(
            tmp.path(),
            r#"
import ProjectDescription

let fixture = "fullHandle: \"acme/from-string\""
let unrelated = Workspace(
    fullHandle: "acme/from-workspace",
    url: "https://ignored.example.com"
)
/*
let ignored = Tuist(
    fullHandle: "acme/from-comment",
    url: "https://ignored.example.com"
)
*/
let tuist = Tuist(
    url: "https://canary.tuist.dev",
    project: .xcode(name: "fullHandle: \"acme/from-nested\""),
    fullHandle: "tuist/app"
)
"#,
        );

        let config = expect_tuist(resolve_config(tmp.path(), &xdg).unwrap());
        assert_eq!(config.url, "https://canary.tuist.dev");
        assert_eq!(config.account.as_deref(), Some("tuist"));
        assert_eq!(config.project.as_deref(), Some("app"));
    }

    #[test]
    fn resolve_uses_tuist_toml_when_workspace_is_unspecified() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_tuist_toml(
            tmp.path(),
            r#"
project = "tuist/gradle-plugin"
url = "https://canary.tuist.dev"
"#,
        );

        let config = expect_tuist(resolve_config(tmp.path(), &xdg).unwrap());
        assert_eq!(config.url, "https://canary.tuist.dev");
        assert_eq!(config.account.as_deref(), Some("tuist"));
        assert_eq!(config.project.as_deref(), Some("gradle-plugin"));
        assert_eq!(config.provider_name, "tuist");
    }

    #[test]
    fn tuist_toml_beats_tuist_swift_when_both_exist() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_tuist_toml(
            tmp.path(),
            r#"
project = "tuist/toml-app"
"#,
        );
        write_tuist_swift(
            tmp.path(),
            r#"
import ProjectDescription

let tuist = Tuist(
    fullHandle: "tuist/swift-app",
    project: .xcode()
)
"#,
        );

        let config = expect_tuist(resolve_config(tmp.path(), &xdg).unwrap());
        assert_eq!(config.project.as_deref(), Some("toml-app"));
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

[infrastructures.tuist]
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
    fn workspace_cache_binding_uses_root_provider_defaults() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_workspace(
            tmp.path(),
            r#"
[infrastructure.cache]
provider = "tuist"
project = "cache-app"

[infrastructures.tuist]
kind = "tuist"
url = "https://cache.tuist.dev"
account = "acme"
project = "default-app"
"#,
        );

        let config = expect_tuist(resolve_config(tmp.path(), &xdg).unwrap());

        assert_eq!(config.url, "https://cache.tuist.dev");
        assert_eq!(config.account.as_deref(), Some("acme"));
        assert_eq!(config.project.as_deref(), Some("cache-app"));
        assert_eq!(config.provider_name, "tuist");
    }

    #[test]
    fn resolve_execution_uses_root_provider_with_capability_overrides() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_workspace(
            tmp.path(),
            r#"
[infrastructure.execution]
provider = "tuist"
project = "remote-builds"

[infrastructures.tuist]
kind = "tuist"
url = "https://cache.tuist.dev"
account = "acme"
project = "default-app"
"#,
        );

        let remote = resolve_execution(tmp.path(), &xdg, None).unwrap();

        assert_eq!(
            remote,
            RemoteExecution {
                provider: "tuist".to_string(),
                executor: Some("tuist".to_string()),
                environment: None,
                account: Some("acme".to_string()),
                project: Some("remote-builds".to_string()),
                properties: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn resolve_execution_defaults_to_microsandbox_without_config() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());

        let remote = resolve_execution(tmp.path(), &xdg, None).unwrap();

        assert_eq!(remote, RemoteExecution::provider("microsandbox"));
    }

    #[test]
    fn execution_properties_follow_only_the_selected_workspace_provider() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_workspace(
            tmp.path(),
            r#"
[infrastructure.execution]
provider = "workers"
properties = { program = "/worker/tool", version = 1 }
[infrastructures.workers]
kind = "local"
[infrastructures.other]
kind = "local"
"#,
        );
        let configured = resolve_execution_unresolved(tmp.path(), &xdg, None).unwrap();
        assert_eq!(configured.properties["version"], serde_json::json!(1));
        assert_eq!(
            configured.properties,
            resolve_execution_unresolved(tmp.path(), &xdg, Some("workers"))
                .unwrap()
                .properties
        );
        assert!(
            resolve_execution_unresolved(tmp.path(), &xdg, Some("other"))
                .unwrap()
                .properties
                .is_empty()
        );
    }

    #[test]
    fn execution_properties_follow_only_the_selected_user_provider() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_user_config(
            &xdg,
            r#"
[infrastructure.execution]
provider = "e2b"
properties = { program = "/worker/tool", version = 1 }
"#,
        );
        let configured = resolve_execution_unresolved(tmp.path(), &xdg, None).unwrap();
        assert_eq!(configured.properties["version"], serde_json::json!(1));
        assert_eq!(
            configured.properties,
            resolve_execution_unresolved(tmp.path(), &xdg, Some("e2b"))
                .unwrap()
                .properties
        );
        assert!(
            resolve_execution_unresolved(tmp.path(), &xdg, Some("daytona"))
                .unwrap()
                .properties
                .is_empty()
        );
        assert!(resolve_config(tmp.path(), &xdg).is_ok());
    }

    #[test]
    fn resolve_execution_uses_named_microsandbox_image() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_workspace(
            tmp.path(),
            r#"
[infrastructures.remote_tests]
kind = "microsandbox"
image = "node:22.18.0-alpine"
"#,
        );

        let remote = resolve_execution(tmp.path(), &xdg, Some("remote_tests")).unwrap();

        assert_eq!(
            remote,
            RemoteExecution {
                provider: "remote_tests".to_string(),
                executor: Some("microsandbox".to_string()),
                environment: Some("node:22.18.0-alpine".to_string()),
                account: None,
                project: None,
                properties: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn resolve_execution_uses_named_e2b_template() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_workspace(
            tmp.path(),
            r#"
[infrastructures.remote_tests]
kind = "e2b"
template = "vitest-v1"
"#,
        );

        let remote = resolve_execution(tmp.path(), &xdg, Some("remote_tests")).unwrap();

        assert_eq!(
            remote,
            RemoteExecution {
                provider: "remote_tests".to_string(),
                executor: Some("e2b".to_string()),
                environment: Some("vitest-v1".to_string()),
                account: None,
                project: None,
                properties: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn resolve_execution_uses_named_daytona_image() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_workspace(
            tmp.path(),
            r#"
[infrastructures.remote_tests]
kind = "daytona"
image = "node:22-bookworm"
"#,
        );

        let remote = resolve_execution(tmp.path(), &xdg, Some("remote_tests")).unwrap();

        assert_eq!(
            remote,
            RemoteExecution {
                provider: "remote_tests".to_string(),
                executor: Some("daytona".to_string()),
                environment: Some("node:22-bookworm".to_string()),
                account: None,
                project: None,
                properties: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn resolve_execution_uses_named_docker_image() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_workspace(
            tmp.path(),
            r#"
[infrastructure.execution]
provider = "container_tests"
account = "acme"
project = "tests"

[infrastructures.container_tests]
kind = "docker"
image = "node:22-bookworm"
"#,
        );

        let remote = resolve_execution(tmp.path(), &xdg, None).unwrap();
        assert_eq!(
            remote,
            RemoteExecution {
                provider: "container_tests".to_string(),
                executor: Some("docker".to_string()),
                environment: Some("node:22-bookworm".to_string()),
                account: Some("acme".to_string()),
                project: Some("tests".to_string()),
                properties: BTreeMap::new(),
            }
        );
        let explicit = resolve_execution(tmp.path(), &xdg, Some("container_tests")).unwrap();
        assert_eq!(explicit.executor, remote.executor);
        assert_eq!(explicit.environment, remote.environment);
        assert_eq!(explicit.account, None);
        assert_eq!(explicit.project, None);
    }

    #[test]
    fn docker_provider_rejects_cache_and_auth_roles() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_workspace(
            tmp.path(),
            r#"
[infrastructure.cache]
provider = "container_tests"

[infrastructures.container_tests]
kind = "docker"
image = "node:22-bookworm"
"#,
        );

        let error = resolve_config_with_env(tmp.path(), &xdg, None).unwrap_err();
        assert!(format!("{error:#}").contains("not a cache"));
        let error = resolve_auth_provider(tmp.path(), &xdg, "container_tests").unwrap_err();
        assert!(error.to_string().contains("not shared caching"));
    }

    #[test]
    fn resolve_execution_supports_builtin_docker() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        assert_eq!(
            resolve_execution(tmp.path(), &xdg, Some("docker")).unwrap(),
            RemoteExecution::provider("docker")
        );
    }

    #[test]
    fn resolve_execution_explicit_provider_beats_workspace_config() {
        let tmp = TempDir::new().unwrap();
        let xdg = xdg_under(tmp.path());
        write_workspace(
            tmp.path(),
            r#"
[infrastructure.execution]
provider = "tuist"

[infrastructures.tuist]
kind = "tuist"
account = "acme"
"#,
        );

        let remote = resolve_execution(tmp.path(), &xdg, Some("daytona")).unwrap();

        assert_eq!(remote, RemoteExecution::provider("daytona"));
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
[infrastructures.acme]
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
