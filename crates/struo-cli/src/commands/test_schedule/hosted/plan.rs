use std::path::Path;

use anyhow::{ensure, Context, Result};
use serde_json::json;

use super::{artifact, client, metadata};
use crate::cli::{Format, Output};

pub(crate) async fn plan(
    workspace: &Path,
    output: Output,
    changed_paths: &[String],
    target: Option<&str>,
    shards: u32,
    reference: Option<String>,
) -> Result<()> {
    ensure!(shards > 0, "--shards must be positive");
    let reference = reference
        .or_else(|| metadata::variable("TUIST_SHARD_REFERENCE"))
        .or_else(|| metadata::ci_run().map(|(provider, id)| format!("{provider}-{id}")))
        .filter(|value| !value.trim().is_empty())
        .context("pass --reference or set TUIST_SHARD_REFERENCE")?;
    let portable = crate::commands::query::test_plan_record(workspace, changed_paths, target)?;
    let value = if portable.plan.batches.is_empty() {
        json!({"shard_plan_id": "", "shards": []})
    } else {
        let host = client::Host::resolve(workspace)?;
        let created = host.post("shards", &json!({"reference": reference, "modules": portable.plan.batches.iter().map(|batch| &batch.id).collect::<Vec<_>>(), "granularity": "module", "shard_total": shards})).await?;
        let id =
            uuid::Uuid::parse_str(created["id"].as_str().context("missing hosted plan UUID")?)?
                .to_string();
        let count = created["shard_count"]
            .as_u64()
            .context("missing shard count")?;
        ensure!(
            count > 0 && count <= u64::from(shards),
            "invalid hosted shard count"
        );
        let file = tempfile::NamedTempFile::new()?;
        let bytes = crate::render::structured(Format::Json, &portable)?;
        artifact::write(file.path(), &id, bytes.as_bytes())?;
        host.upload(file.path(), &id, &reference).await?;
        tracing::info!(shard_plan_id = %id, shard_count = count, batch_count = portable.plan.batches.len(), "uploaded hosted test plan");
        json!({"shard_plan_id": id, "shards": (0..count).collect::<Vec<_>>()})
    };
    crate::commands::query::write_body(output, || format!("{value}\n"), &value).await
}
