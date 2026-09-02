use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;
use once_core::{ResourceLimits, Xdg};

use crate::cli::Output;

pub async fn run(
    workspace: &Path,
    xdg: &Xdg,
    output: Output,
    resource_limits: ResourceLimits,
    argv: Vec<String>,
) -> Result<ExitCode> {
    Box::pin(crate::commands::xcodebuild::run(
        workspace,
        xdg,
        output,
        resource_limits,
        argv,
    ))
    .await
}
