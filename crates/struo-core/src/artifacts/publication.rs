use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::ArtifactDescriptor;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationIntent {
    pub schema: String,
    pub root: ArtifactDescriptor,
    pub target: String,
    pub track: Option<String>,
    pub revision: Option<String>,
}

impl PublicationIntent {
    pub fn key(&self) -> Result<String> {
        let mut value = serde_json::to_value(self)?;
        value.sort_all_objects();
        let bytes = serde_json::to_vec(&value)?;
        Ok(format!("sha256:{}", hex::encode(Sha256::digest(bytes))))
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationReceipt {
    pub schema: String,
    pub key: String,
    pub root: ArtifactDescriptor,
    pub destination: String,
    pub url: String,
}

impl PublicationReceipt {
    pub fn validate(&self, intent: &PublicationIntent, destination: &str) -> Result<()> {
        // The Tuist server still writes the legacy `once` receipt schema.
        ensure!(
            self.schema == "once.publication_receipt.v1"
                && self.key == intent.key()?
                && self.root == intent.root
                && self.destination == destination,
            "publication was not confirmed by a matching receipt"
        );
        Ok(())
    }
}
