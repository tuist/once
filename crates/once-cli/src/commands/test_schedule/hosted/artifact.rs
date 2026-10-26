use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

use anyhow::{ensure, Context, Result};
use base64::engine::general_purpose::STANDARD;
use once_core::{ResourceLimits, ResourcePool, ResourceRequest};
use serde::Deserialize;

pub(super) const MAX_ARTIFACT_BYTES: u64 = 96_000_000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Artifact {
    schema: String,
    #[serde(rename = "shardPlanID")]
    shard_plan_id: String,
    portable_plan: String,
}

pub(super) fn write(path: &Path, id: &str, bytes: &[u8]) -> Result<()> {
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    write!(
        file,
        "{{\"schema\":\"tuist.once_shard_plan.v1\",\"shardPlanID\":{},\"portablePlan\":\"",
        serde_json::to_string(id)?
    )?;
    let mut encoder = base64::write::EncoderWriter::new(&mut file, &STANDARD);
    encoder.write_all(bytes)?;
    encoder.finish()?;
    drop(encoder);
    file.write_all(b"\"}")?;
    file.flush()?;
    ensure!(
        std::fs::metadata(path)?.len() <= MAX_ARTIFACT_BYTES,
        "portable plan exceeds hosted artifact limit"
    );
    Ok(())
}

pub(super) async fn unpack(
    path: &Path,
    id: &str,
    destination: &Path,
    limits: ResourceLimits,
) -> Result<()> {
    let size = std::fs::metadata(path)?.len();
    ensure!(
        size <= MAX_ARTIFACT_BYTES,
        "hosted artifact exceeds size limit"
    );
    let pool = Arc::new(ResourcePool::new(limits));
    let _permit = pool
        .acquire_required(ResourceRequest::new(1, size.saturating_mul(4)))
        .await?;
    let artifact: Artifact =
        serde_json::from_reader(std::io::BufReader::new(std::fs::File::open(path)?))
            .context("reading hosted portable plan artifact")?;
    ensure!(
        artifact.schema == "tuist.once_shard_plan.v1"
            && uuid::Uuid::parse_str(&artifact.shard_plan_id)? == uuid::Uuid::parse_str(id)?,
        "unsupported hosted artifact or mismatched plan UUID"
    );
    let decoder = base64::read::DecoderReader::new(artifact.portable_plan.as_bytes(), &STANDARD);
    let mut bounded = decoder.take(MAX_ARTIFACT_BYTES);
    let mut output = std::fs::File::create(destination)?;
    std::io::copy(&mut bounded, &mut output).context("decoding exact portable plan bytes")?;
    Ok(())
}
