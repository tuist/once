//! `fabrik toolchain inspect` - print the project toolchain contract.

use std::path::Path;

use anyhow::Result;
use tokio::io::AsyncWriteExt;

use crate::cli::Format;
use crate::render;

mod contract;
mod human;
mod lock;
mod mise;
mod platform;

pub async fn inspect(workspace: &Path, format: Format, platform: Option<&str>) -> Result<()> {
    let contract = mise::inspect_workspace(workspace, platform)?;
    let body = match format {
        Format::Human => human::render(&contract),
        Format::Json | Format::Toon => render::structured(format, &contract)?,
    };

    let mut out = tokio::io::stdout();
    out.write_all(body.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}
