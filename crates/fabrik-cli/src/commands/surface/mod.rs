use anyhow::Result;
use tokio::io::AsyncWriteExt;

use crate::cli::Format;
use crate::render;

mod human;
mod model;

pub async fn print(path: &[&str], format: Format) -> Result<()> {
    let surface = model::load(path)?;
    let body = match format {
        Format::Human => human::render(&surface),
        Format::Json | Format::Toon => render::structured(format, &surface)?,
    };
    let mut out = tokio::io::stdout();
    out.write_all(body.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}
