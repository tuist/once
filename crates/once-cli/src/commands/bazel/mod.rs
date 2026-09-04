//! Bazel command compatibility routing.

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
    let graph = once_frontend::load_graph_workspace(workspace)?;
    let Some(target) = invocation.target(&graph) else {
        return passthrough::run(&argv);
    };
    let resolved = crate::commands::graph::resolve_invocation_configuration(workspace, &[])?;
    let cache = crate::cache_provider::resolve(workspace, xdg)?;
    let output = Output::new(output.format, output.quiet)
        .with_color(output.color)
        .with_verbose(output.verbose);
    match invocation.command {
        invocation::Command::Build => {
            Box::pin(crate::commands::graph::build(
                workspace,
                &cache,
                output,
                &target,
                SandboxMode::Off,
                resource_limits,
                &resolved,
                false,
            ))
            .await
        }
        invocation::Command::Test => {
            Box::pin(crate::commands::graph::test(
                workspace,
                &cache,
                output,
                &target,
                SandboxMode::Off,
                resource_limits,
                &resolved,
                false,
            ))
            .await
        }
    }
}
