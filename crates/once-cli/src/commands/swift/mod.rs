//! Swift Package Manager compatibility routing.

mod invocation;
mod passthrough;

use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;
use once_core::{ResourceLimits, SandboxMode, Xdg};

use crate::cli::Output;

pub async fn run(
    workspace: &Path,
    xdg: &Xdg,
    output: Output,
    resource_limits: ResourceLimits,
    argv: Vec<String>,
) -> Result<ExitCode> {
    let Some(invocation) = invocation::Invocation::parse(&argv) else {
        return passthrough::run(&argv);
    };
    if invocation::has_unlocked_remote_dependencies(workspace, &passthrough::system_swift()?)? {
        return passthrough::run(&argv);
    }
    let graph = once_frontend::load_graph_workspace(workspace)?;
    let Some(package) = invocation::Invocation::package(&graph) else {
        return passthrough::run(&argv);
    };
    let resolved = crate::commands::graph::resolve_invocation_configuration(workspace, &[])?;
    let output = Output::new(output.format, output.quiet || invocation.quiet);
    let cache = crate::cache_provider::resolve(workspace, xdg)?;
    match invocation.command {
        invocation::Command::Build => {
            Box::pin(crate::commands::graph::build(
                workspace,
                &cache,
                output,
                &package.build_target,
                SandboxMode::Off,
                resource_limits,
                &resolved,
                false,
            ))
            .await
        }
        invocation::Command::Test if package.test_targets.is_empty() => {
            Box::pin(crate::commands::graph::build(
                workspace,
                &cache,
                output,
                &package.build_target,
                SandboxMode::Off,
                resource_limits,
                &resolved,
                false,
            ))
            .await
        }
        invocation::Command::Test => {
            for target in package.test_targets {
                let status = crate::commands::graph::test(
                    workspace,
                    &cache,
                    output,
                    &target,
                    SandboxMode::Off,
                    resource_limits.clone(),
                    &resolved,
                    false,
                )
                .await?;
                if status != ExitCode::SUCCESS {
                    return Ok(status);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}
