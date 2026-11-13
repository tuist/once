use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::model::MAX_METADATA_BYTES;
use super::{ArtifactDescriptor, ArtifactInfo, ArtifactManifest, ArtifactMember, ArtifactSet};

#[derive(Debug, Deserialize, Serialize)]
pub struct ArtifactExport {
    pub schema: String,
    pub root: ArtifactDescriptor,
    pub directory: PathBuf,
    pub artifacts: ArtifactSet,
    #[serde(default)]
    pub automatic_publication: bool,
}

pub async fn export_artifacts(
    workspace: &Path,
    info: &ArtifactInfo,
    declared_outputs: &[String],
    directory: &Path,
    _permit: &crate::ResourcePermit,
) -> Result<ArtifactExport> {
    info.validate(declared_outputs)?;
    let workspace = tokio::fs::canonicalize(workspace).await?;
    tokio::fs::create_dir_all(workspace.join(".once")).await?;
    let staging = tempfile::tempdir_in(workspace.join(".once"))?;
    let blobs = staging.path().join("blobs/sha256");
    tokio::fs::create_dir_all(&blobs).await?;
    let mut members = BTreeMap::new();
    for artifact in &info.artifacts {
        let mut payloads = BTreeMap::new();
        let mut interfaces = artifact.interfaces.clone();
        for (name, payload) in &artifact.payloads {
            let source = tokio::fs::canonicalize(workspace.join(&payload.path))
                .await
                .with_context(|| format!("opening artifact payload `{}`", payload.path))?;
            ensure!(
                source.starts_with(&workspace),
                "artifact payload escapes workspace through a symlink"
            );
            if payload.optional && tokio::fs::metadata(&source).await?.len() == 0 {
                for interface in interfaces.values_mut() {
                    interface.spec.retain(|key, value| {
                        !key.ends_with("_payload") || value.as_str() != Some(name.as_str())
                    });
                }
                continue;
            }
            let descriptor = copy_content(&source, &blobs, &payload.media_type).await?;
            payloads.insert(name.clone(), descriptor);
        }
        let manifest = ArtifactManifest {
            schema: "once.artifact.v1".into(),
            payloads,
            interfaces,
        };
        let descriptor =
            write_metadata(&manifest, &blobs, "application/vnd.once.artifact.v1+json").await?;
        members.insert(
            artifact.name.clone(),
            ArtifactMember {
                artifact: descriptor,
                selectors: artifact.selectors.clone(),
            },
        );
    }
    let artifacts = ArtifactSet {
        schema: "once.artifact_set.v1".into(),
        members,
    };
    let root = write_metadata(
        &artifacts,
        &blobs,
        "application/vnd.once.artifact-set.v1+json",
    )
    .await?;
    let destination = directory.join(root.digest.trim_start_matches("sha256:"));
    let destination_blobs = destination.join("blobs/sha256");
    tokio::fs::create_dir_all(&destination_blobs).await?;
    let mut entries = tokio::fs::read_dir(&blobs).await?;
    while let Some(entry) = entries.next_entry().await? {
        copy_content(
            &entry.path(),
            &destination_blobs,
            "application/octet-stream",
        )
        .await?;
    }
    let index = serde_json::to_vec(&artifacts)?;
    let temporary = tempfile::NamedTempFile::new_in(&destination)?;
    tokio::fs::write(temporary.path(), &index).await?;
    temporary.persist(destination.join("index.json"))?;
    tracing::info!(artifact_digest = %root.digest, artifacts = artifacts.members.len(), "exported immutable artifact set");
    Ok(ArtifactExport {
        schema: "once.artifact_export.v1".into(),
        root,
        directory: destination,
        artifacts,
        automatic_publication: info.automatic_publication,
    })
}

pub async fn read_artifact_json<T: serde::de::DeserializeOwned>(
    directory: &Path,
    descriptor: &ArtifactDescriptor,
) -> Result<T> {
    ensure!(
        descriptor.size_bytes <= MAX_METADATA_BYTES,
        "artifact metadata exceeds one mebibyte"
    );
    let path = content_path(directory, descriptor)?;
    let file = tokio::fs::File::open(path).await?;
    let mut bytes = Vec::new();
    file.take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .await?;
    ensure!(
        bytes.len() as u64 == descriptor.size_bytes && digest(&bytes) == descriptor.digest,
        "artifact metadata digest or size mismatch"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn content_path(directory: &Path, descriptor: &ArtifactDescriptor) -> Result<PathBuf> {
    let hash = descriptor
        .digest
        .strip_prefix("sha256:")
        .unwrap_or_default();
    ensure!(
        hash.len() == 64
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "invalid artifact content digest"
    );
    Ok(directory.join("blobs/sha256").join(hash))
}

async fn write_metadata(
    value: &impl Serialize,
    blobs: &Path,
    media_type: &str,
) -> Result<ArtifactDescriptor> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(
        bytes.len() as u64 <= MAX_METADATA_BYTES,
        "artifact metadata exceeds one mebibyte"
    );
    let descriptor = ArtifactDescriptor {
        digest: digest(&bytes),
        size_bytes: bytes.len() as u64,
        media_type: media_type.into(),
    };
    tokio::fs::write(
        blobs.join(descriptor.digest.trim_start_matches("sha256:")),
        bytes,
    )
    .await?;
    Ok(descriptor)
}

async fn copy_content(source: &Path, blobs: &Path, media_type: &str) -> Result<ArtifactDescriptor> {
    let mut input = tokio::fs::File::open(source).await?;
    ensure!(
        input.metadata().await?.is_file(),
        "artifact payload must be a regular file; package directories with write_archive"
    );
    let temporary = tempfile::NamedTempFile::new_in(blobs)?;
    let mut output = tokio::fs::File::from_std(temporary.reopen()?);
    let mut hash = Sha256::new();
    let mut size = 0;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        output.write_all(&buffer[..count]).await?;
        size += count as u64;
    }
    output.flush().await?;
    drop(output);
    let digest = format!("sha256:{}", hex::encode(hash.finalize()));
    temporary.persist(blobs.join(digest.trim_start_matches("sha256:")))?;
    Ok(ArtifactDescriptor {
        digest,
        size_bytes: size,
        media_type: media_type.into(),
    })
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests;
