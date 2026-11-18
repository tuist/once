use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use reqwest::{Client, Response, Url};
use serde::Deserialize;
use struo_core::artifacts::{
    content_path, read_artifact_json, ArtifactDescriptor, ArtifactExport, ArtifactManifest,
    PublicationIntent, PublicationReceipt,
};
use struo_core::Xdg;
use tokio_util::io::ReaderStream;

pub(super) struct Service {
    client: Client,
    base: Url,
    token: String,
    destination: String,
}

#[derive(Deserialize)]
struct Capabilities {
    schema: String,
    destination: String,
}

impl Service {
    pub(super) async fn connect(base: Url, token: String) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_mins(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let capabilities: Capabilities =
            read_json(client.get(base.clone()).bearer_auth(&token).send().await?).await?;
        // The Tuist server still writes the legacy `once` service schema.
        ensure!(
            capabilities.schema == "once.publication_service.v1"
                && !capabilities.destination.is_empty(),
            "service does not support the artifact publication protocol"
        );
        Ok(Self {
            client,
            base,
            token,
            destination: capabilities.destination,
        })
    }

    fn endpoint(&self, parts: &[&str]) -> Result<Url> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|()| anyhow::anyhow!("invalid publication URL"))?
            .pop_if_empty()
            .extend(parts);
        Ok(url)
    }

    pub(super) fn require_destination(&self, approved: &str) -> Result<()> {
        ensure!(self.destination == approved, "artifact destination changed; explicitly authorize the new destination before publishing from builds");
        Ok(())
    }

    fn validate_receipt(
        &self,
        receipt: &PublicationReceipt,
        intent: &PublicationIntent,
    ) -> Result<()> {
        receipt.validate(intent, &self.destination)?;
        let url =
            Url::parse(&receipt.url).context("publication receipt has no usable absolute URL")?;
        ensure!(
            url.scheme() == "https"
                || (url.scheme() == "http" && url.origin() == self.base.origin()),
            "publication receipt has an insecure or unsupported URL"
        );
        ensure!(
            url.username().is_empty() && url.password().is_none(),
            "publication receipt URL must not contain credentials"
        );
        Ok(())
    }

    pub(super) async fn publish(
        &self,
        package: &ArtifactExport,
        intent: &PublicationIntent,
    ) -> Result<PublicationReceipt> {
        let key = intent.key()?;
        let receipt_url = self.endpoint(&["publications", &key])?;
        let response = self
            .client
            .get(receipt_url.clone())
            .bearer_auth(&self.token)
            .send()
            .await?;
        if response.status().is_success() {
            let receipt: PublicationReceipt = read_json(response).await?;
            self.validate_receipt(&receipt, intent)?;
            return Ok(receipt);
        }
        ensure!(
            response.status() == reqwest::StatusCode::NOT_FOUND,
            "publication lookup returned HTTP {}",
            response.status().as_u16()
        );
        let mut closure = BTreeMap::new();
        insert_descriptor(&mut closure, package.root.clone())?;
        for member in package.artifacts.members.values() {
            insert_descriptor(&mut closure, member.artifact.clone())?;
            let manifest: ArtifactManifest =
                read_artifact_json(&package.directory, &member.artifact).await?;
            for descriptor in manifest.payloads.into_values() {
                insert_descriptor(&mut closure, descriptor)?;
            }
        }
        for descriptor in closure.values() {
            let file = tokio::fs::File::open(content_path(&package.directory, descriptor)?).await?;
            let response = self
                .client
                .post(self.endpoint(&["publications", &key, "content", &descriptor.digest])?)
                .bearer_auth(&self.token)
                .header("content-type", "application/octet-stream")
                .header("content-length", descriptor.size_bytes)
                .body(reqwest::Body::wrap_stream(ReaderStream::with_capacity(
                    file,
                    64 * 1024,
                )))
                .send()
                .await
                .context("artifact transfer failed; publication was not confirmed")?;
            if response.status() == reqwest::StatusCode::CONFLICT {
                let receipt: PublicationReceipt = read_json(
                    self.client
                        .get(receipt_url.clone())
                        .bearer_auth(&self.token)
                        .send()
                        .await?,
                )
                .await?;
                self.validate_receipt(&receipt, intent)?;
                return Ok(receipt);
            }
            ensure!(
                response.status().is_success(),
                "artifact upload returned HTTP {}",
                response.status().as_u16()
            );
        }
        let receipt: PublicationReceipt = read_json(self.client.post(receipt_url)
            .bearer_auth(&self.token).json(intent).send().await
            .context("publication outcome is unknown; replay the same publication to recover its receipt")?).await?;
        self.validate_receipt(&receipt, intent)?;
        Ok(receipt)
    }
}

pub(super) fn authorization_path(
    workspace: &Path,
    target: &str,
    endpoint: &Url,
    xdg: &Xdg,
) -> Result<std::path::PathBuf> {
    let identity = serde_json::to_vec(&(workspace.canonicalize()?, endpoint.as_str(), target))?;
    Ok(xdg
        .struo_state()
        .join("artifact-authorizations")
        .join(struo_cas::Digest::of_bytes(&identity).to_string()))
}

fn insert_descriptor(
    closure: &mut BTreeMap<String, ArtifactDescriptor>,
    descriptor: ArtifactDescriptor,
) -> Result<()> {
    if let Some(existing) = closure.get(&descriptor.digest) {
        ensure!(
            existing.size_bytes == descriptor.size_bytes,
            "conflicting artifact descriptors"
        );
    } else {
        closure.insert(descriptor.digest.clone(), descriptor);
    }
    Ok(())
}

async fn read_json<T: serde::de::DeserializeOwned>(mut response: Response) -> Result<T> {
    ensure!(
        response.status().is_success(),
        "artifact service returned HTTP {}",
        response.status().as_u16()
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 1024 * 1024,
            "publication metadata exceeds one mebibyte"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}
