use anyhow::Result;
use once_cas::{CacheProvider, Digest};
use once_core::Outcome;
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use super::runtime_descriptor::RuntimeDescriptor;
use crate::cli::{Format, Output};
use crate::commands::util::write_cache_blob;
use crate::render;

#[derive(Serialize)]
pub(super) struct RunRecord<'a> {
    target: &'a str,
    kind: &'a str,
    action_digest: String,
    cache: &'a str,
    exit_code: i32,
    output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime: Option<RuntimeDescriptor>,
}

impl<'a> RunRecord<'a> {
    pub(super) fn new(
        target: &'a str,
        kind: &'a str,
        outcome: &Outcome,
        cache: &'a str,
        output: String,
        runtime: Option<RuntimeDescriptor>,
    ) -> Self {
        Self {
            target,
            kind,
            action_digest: outcome.action.to_string(),
            cache,
            exit_code: outcome.result.exit_code,
            output,
            runtime,
        }
    }
}

pub(super) async fn render(
    output: Output,
    cache: &CacheProvider,
    stdout: Option<&Digest>,
    stderr: Option<&Digest>,
    record: &RunRecord<'_>,
    streams_live: bool,
) -> Result<()> {
    match output.format {
        Format::Human => render_human(output, cache, stdout, stderr, record, streams_live).await,
        Format::Json | Format::Toon => {
            render_structured(output.format, cache, stderr, record).await
        }
    }
}

async fn render_human(
    output: Output,
    cache: &CacheProvider,
    stdout: Option<&Digest>,
    stderr: Option<&Digest>,
    record: &RunRecord<'_>,
    streams_live: bool,
) -> Result<()> {
    let mut out = tokio::io::stdout();
    if !streams_live {
        if let Some(digest) = stdout {
            write_cache_blob(cache, digest, &mut out).await?;
        }
    }
    out.flush().await?;

    let mut err = tokio::io::stderr();
    if !streams_live {
        if let Some(digest) = stderr {
            write_cache_blob(cache, digest, &mut err).await?;
        }
    }
    if output.show_human_trailers() {
        err.write_all(
            format!(
                "once: ran {} (cache {}, exit={})\n",
                record.target, record.cache, record.exit_code
            )
            .as_bytes(),
        )
        .await?;
        if let Some(runtime) = &record.runtime {
            err.write_all(runtime_trailer(runtime).as_bytes()).await?;
        }
    }
    err.flush().await?;
    Ok(())
}

async fn render_structured(
    format: Format,
    cache: &CacheProvider,
    stderr: Option<&Digest>,
    record: &RunRecord<'_>,
) -> Result<()> {
    let mut err = tokio::io::stderr();
    if let Some(digest) = stderr {
        write_cache_blob(cache, digest, &mut err).await?;
    }
    err.flush().await?;

    let mut out = tokio::io::stdout();
    out.write_all(render::structured(format, record)?.as_bytes())
        .await?;
    out.flush().await?;
    Ok(())
}

fn runtime_trailer(runtime: &RuntimeDescriptor) -> String {
    let names = runtime
        .interfaces
        .iter()
        .map(|interface| interface.name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "once: runtime kind={} subject={} interfaces={names}\n",
        runtime.kind, runtime.subject
    )
}
