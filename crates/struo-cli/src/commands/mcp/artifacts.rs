use std::path::PathBuf;

use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;
use struo_core::SandboxMode;

use super::{parse_args, run_async_result, Server};
use crate::cli::ArtifactCmd;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportArgs {
    target: String,
    #[serde(default)]
    output: Option<PathBuf>,
    #[serde(default)]
    config: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishArgs {
    target: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    track: Option<String>,
    #[serde(default)]
    config: Vec<String>,
}

impl Server {
    pub(super) fn tool_query_artifacts(&self, args: &Value) -> Result<Value> {
        let args: super::TargetExecutionArgs = parse_args(args)?;
        crate::commands::graph::artifacts::query(&self.workspace, &args.target)
    }

    pub(super) fn tool_artifacts(&self, args: &Value, publish: bool) -> Result<Value> {
        let cmd = if publish {
            let args: PublishArgs = parse_args(args)?;
            ArtifactCmd::Publish {
                target: struo_frontend::normalize_cli_target_from(
                    &self.workspace,
                    &self.workspace,
                    &args.target,
                )?,
                provider: args.provider,
                track: args.track,
                allow_on_build: false,
                config: args.config,
                sandbox: SandboxMode::Off,
            }
        } else {
            let args: ExportArgs = parse_args(args)?;
            ArtifactCmd::Export {
                target: struo_frontend::normalize_cli_target_from(
                    &self.workspace,
                    &self.workspace,
                    &args.target,
                )?,
                output: args.output,
                config: args.config,
                sandbox: SandboxMode::Off,
            }
        };
        let config = match &cmd {
            ArtifactCmd::Export { config, .. } | ArtifactCmd::Publish { config, .. } => config,
        };
        let resolved =
            crate::commands::graph::resolve_invocation_configuration(&self.workspace, config)?;
        let workspace = self.workspace.clone();
        let limits = self.resource_limits.clone();
        let cache = crate::cache_provider::resolve(&workspace, &struo_core::Xdg::from_env())?;
        run_async_result(async move {
            crate::commands::artifacts::execute(&workspace, &cache, cmd, limits, &resolved).await
        })
    }
}
