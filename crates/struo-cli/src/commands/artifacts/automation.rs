use std::path::Path;

use anyhow::Result;
use struo_cas::CacheProvider;
use struo_core::{ResourceLimits, SandboxMode};

use crate::cli::{ArtifactCmd, Output};
use crate::commands::graph::ResolvedConfiguration;

pub async fn after_build(
    workspace: &Path,
    cache: &CacheProvider,
    targets: &[String],
    sandbox: SandboxMode,
    limits: ResourceLimits,
    resolved: &ResolvedConfiguration,
    output: Output,
) -> Result<()> {
    let config = struo_frontend::load_infrastructure_config(workspace)?;
    if config.publish_on_build.is_empty() {
        return Ok(());
    }
    let selected = config
        .publish_on_build
        .iter()
        .map(|target| struo_frontend::normalize_cli_target_from(workspace, workspace, target))
        .collect::<Result<Vec<_>, _>>()?;
    for target in targets {
        if !selected.contains(target)
            || !super::super::graph::artifacts::query(workspace, target)?["supported"]
                .as_bool()
                .unwrap_or(false)
        {
            continue;
        }
        let publication = super::execute_with_policy(
            workspace,
            cache,
            ArtifactCmd::Publish {
                target: target.clone(),
                provider: None,
                track: None,
                allow_on_build: false,
                sandbox,
                config: vec![],
            },
            limits.clone(),
            resolved,
            true,
        )
        .await?;
        if output.show_human_trailers() && !publication.is_null() {
            super::write(output, &publication).await?;
        }
    }
    Ok(())
}
