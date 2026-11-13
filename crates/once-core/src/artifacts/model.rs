use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::WorkspacePath;

pub const ARTIFACT_INFO_SCHEMA: &str = "once.artifact_info.v1";
pub(super) const MAX_METADATA_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactInfo {
    pub schema: String,
    pub artifacts: Vec<ArtifactDeclaration>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub automatic_publication: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactDeclaration {
    pub name: String,
    pub payloads: BTreeMap<String, ArtifactPayload>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub interfaces: BTreeMap<String, ArtifactInterface>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub selectors: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactPayload {
    pub path: String,
    pub media_type: String,
    /// An optional payload whose output is empty is left out of the exported
    /// manifest, together with interface spec `*_payload` entries that name it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactInterface {
    pub payload: String,
    #[serde(default)]
    pub spec: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactDescriptor {
    pub digest: String,
    pub size_bytes: u64,
    pub media_type: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactManifest {
    pub schema: String,
    pub payloads: BTreeMap<String, ArtifactDescriptor>,
    pub interfaces: BTreeMap<String, ArtifactInterface>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactSet {
    pub schema: String,
    pub members: BTreeMap<String, ArtifactMember>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactMember {
    pub artifact: ArtifactDescriptor,
    pub selectors: BTreeMap<String, String>,
}

impl ArtifactInfo {
    pub fn validate(&self, declared_outputs: &[String]) -> Result<()> {
        ensure!(
            self.schema == ARTIFACT_INFO_SCHEMA || self.schema == "once.artifact_info.v2",
            "unsupported artifact provider schema"
        );
        ensure!(
            !self.automatic_publication || self.schema == "once.artifact_info.v2",
            "automatic publication requires a v2 producer declaration"
        );
        ensure!(
            (1..=128).contains(&self.artifacts.len()),
            "artifact provider must export 1 to 128 artifacts"
        );
        ensure!(
            serde_json::to_vec(self)?.len() as u64 <= MAX_METADATA_BYTES,
            "artifact metadata exceeds one mebibyte"
        );
        let mut names = BTreeSet::new();
        for artifact in &self.artifacts {
            validate_name(&artifact.name)?;
            ensure!(
                names.insert(&artifact.name),
                "duplicate artifact name `{}`",
                artifact.name
            );
            ensure!(
                (1..=128).contains(&artifact.payloads.len()),
                "artifact `{}` must have 1 to 128 payloads",
                artifact.name
            );
            for (name, payload) in &artifact.payloads {
                validate_name(name)?;
                WorkspacePath::try_from(payload.path.as_str())?;
                ensure!(
                    declared_outputs
                        .iter()
                        .any(|output| output == &payload.path),
                    "artifact payload `{}` is not a declared output",
                    payload.path
                );
                ensure!(
                    payload.media_type.len() <= 256
                        && payload.media_type.contains('/')
                        && !payload.media_type.chars().any(char::is_whitespace),
                    "invalid artifact payload media type"
                );
            }
            ensure!(
                artifact.interfaces.len() <= 32 && artifact.selectors.len() <= 32,
                "too many artifact interfaces or selectors"
            );
            for (contract, interface) in &artifact.interfaces {
                ensure!(
                    contract.len() <= 256
                        && !contract.chars().any(char::is_whitespace)
                        && contract
                            .rsplit_once(".v")
                            .is_some_and(|(namespace, version)| {
                                namespace.contains('.')
                                    && !version.is_empty()
                                    && version.bytes().all(|byte| byte.is_ascii_digit())
                            }),
                    "interface contract must be a namespaced versioned identifier"
                );
                let payload = artifact.payloads.get(&interface.payload);
                ensure!(
                    payload.is_some(),
                    "interface `{contract}` references unknown payload `{}`",
                    interface.payload
                );
                ensure!(
                    payload.is_some_and(|payload| !payload.optional),
                    "interface `{contract}` must point to a required payload, not optional `{}`",
                    interface.payload
                );
            }
            for (key, value) in &artifact.selectors {
                validate_name(key)?;
                ensure!(
                    !value.is_empty() && value.len() <= 256,
                    "invalid artifact selector value"
                );
            }
        }
        Ok(())
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        bail!("artifact names must contain 1 to 128 letters, digits, dots, underscores or hyphens");
    }
    Ok(())
}
