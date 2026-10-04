use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
pub(crate) struct Discovery {
    #[serde(default)]
    pub events: Vec<String>,
}

pub(crate) async fn fetch(base: &str) -> Result<Option<Discovery>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut response = client
        .get(format!("{base}/.well-known/once"))
        .send()
        .await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    response = response.error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > 64 * 1024 {
            bail!("Once discovery document exceeds 64 kibibytes");
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .context("decoding Once discovery")
        .map(Some)
}
