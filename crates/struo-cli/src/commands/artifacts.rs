mod automation;
mod publication;
mod service;
mod tuist;

pub use automation::after_build;

use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::Value;
use struo_cas::CacheProvider;
use struo_core::artifacts::ArtifactDescriptor;
use struo_core::{ResourceLimits, ResourcePool, ResourceRequest};
use struo_frontend::{InfrastructureProviderConfig, NamedCacheProviderConfig};

use crate::cli::{ArtifactCmd, Output};
use crate::commands::graph::ResolvedConfiguration;

#[derive(Debug, Serialize)]
pub struct Publication {
    pub schema: &'static str,
    pub artifact: ArtifactDescriptor,
    pub target: String,
    pub track: Option<String>,
    pub git_commit: Option<String>,
    pub git_branch: Option<String>,
    pub provider: String,
    pub url: String,
}

pub async fn execute(
    workspace: &Path,
    cache: &CacheProvider,
    cmd: ArtifactCmd,
    limits: ResourceLimits,
    resolved: &ResolvedConfiguration,
) -> Result<Value> {
    execute_with_policy(workspace, cache, cmd, limits, resolved, false).await
}

async fn execute_with_policy(
    workspace: &Path,
    cache: &CacheProvider,
    cmd: ArtifactCmd,
    limits: ResourceLimits,
    resolved: &ResolvedConfiguration,
    automatic: bool,
) -> Result<Value> {
    let mut allow_on_build = false;
    let (target, destination, sandbox, binding, track) = match cmd {
        ArtifactCmd::Export {
            target,
            output,
            sandbox,
            ..
        } => (
            target,
            output.unwrap_or_else(|| workspace.join(".struo/artifacts")),
            sandbox,
            None,
            None,
        ),
        ArtifactCmd::Publish {
            target,
            provider,
            track,
            sandbox,
            allow_on_build: allow,
            ..
        } => {
            allow_on_build = allow;
            let binding = resolve_provider(workspace, provider.as_deref())?;
            (
                target,
                workspace.join(".struo/artifacts"),
                sandbox,
                Some(binding),
                track,
            )
        }
    };
    let target = struo_frontend::normalize_cli_target(workspace, &target)?;
    let destination = if destination.is_absolute() {
        destination
    } else {
        workspace.join(destination)
    };
    let package = crate::commands::graph::artifacts::export(
        workspace,
        cache,
        &target,
        &destination,
        sandbox,
        limits.clone(),
        resolved,
    )
    .await?;
    let Some((binding, provider)) = binding else {
        return Ok(serde_json::to_value(package)?);
    };
    if automatic && !package.automatic_publication {
        tracing::info!(%target, "producer did not approve automatic publication");
        return Ok(Value::Null);
    }
    let pool = Arc::new(ResourcePool::new(limits));
    let _permit = pool
        .acquire_required(ResourceRequest::new(1, 4 * 1024 * 1024))
        .await?;
    let mut publication = Publication {
        schema: "struo.artifact_publication.v1",
        artifact: package.root.clone(),
        target,
        track,
        git_commit: git_value(workspace, &["rev-parse", "--verify", "HEAD"]),
        git_branch: git_value(workspace, &["symbolic-ref", "--quiet", "--short", "HEAD"]),
        provider: binding.name.clone(),
        url: String::new(),
    };
    let Some(url) = publication::publish(
        workspace,
        &package,
        &publication,
        binding,
        provider,
        automatic,
        allow_on_build,
    )
    .await?
    else {
        return Ok(Value::Null);
    };
    publication.url = url;
    persist_receipt(workspace, &publication).await?;
    tracing::info!(artifact_digest = %publication.artifact.digest, provider = %publication.provider, target = %publication.target, "published artifact set");
    Ok(serde_json::to_value(publication)?)
}

pub async fn write(output: Output, value: &Value) -> Result<()> {
    crate::commands::query::write_body(
        output,
        || {
            if let Some(url) = value.get("url").and_then(Value::as_str) {
                format!("struo: artifact published\n{url}\n")
            } else if value["schema"] == "struo.artifact_contract.v1" {
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(value).unwrap_or_default()
                )
            } else {
                format!(
                    "struo: artifact exported\n{}\n",
                    value["directory"].as_str().unwrap_or_default()
                )
            }
        },
        value,
    )
    .await
}

fn resolve_provider(
    workspace: &Path,
    explicit: Option<&str>,
) -> Result<(NamedCacheProviderConfig, InfrastructureProviderConfig)> {
    let config = struo_frontend::load_infrastructure_config(workspace)?;
    let binding = explicit.map(|name| NamedCacheProviderConfig { name: name.into(), account: None, project: None })
        .or(config.artifacts).context("artifact publication is not configured; set [infrastructure.artifacts] name = <provider>, or pass --provider local")?;
    let provider = if binding.name == "local" {
        InfrastructureProviderConfig::Local
    } else {
        config
            .providers
            .get(&binding.name)
            .cloned()
            .with_context(|| {
                format!(
                    "artifact infrastructure `{}` is not defined in [infrastructures]",
                    binding.name
                )
            })?
    };
    if !matches!(
        provider,
        InfrastructureProviderConfig::Local | InfrastructureProviderConfig::Tuist(_)
    ) {
        bail!("infrastructure `{}` does not support artifact publication; use a local or Tuist infrastructure", binding.name);
    }
    Ok((binding, provider))
}

async fn persist_receipt(workspace: &Path, receipt: &Publication) -> Result<()> {
    let bytes = serde_json::to_vec(receipt)?;
    let id = struo_cas::Digest::of_bytes(&bytes);
    let directory = workspace.join(".struo/publications");
    tokio::fs::create_dir_all(&directory).await?;
    let temporary = tempfile::NamedTempFile::new_in(&directory)?;
    tokio::fs::write(temporary.path(), bytes).await?;
    temporary.persist(directory.join(format!("{id}.json")))?;
    Ok(())
}

fn git_value(workspace: &Path, args: &[&str]) -> Option<String> {
    let mut command = std::process::Command::new("git");
    command.current_dir(workspace).args(args);
    let result = crate::commands::util::capture_command_output(&mut command).ok()?;
    if !result.status.success() || result.stdout_truncated {
        return None;
    }
    String::from_utf8(result.stdout)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
