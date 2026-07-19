use anyhow::{ensure, Context, Result};
use once_cas::Digest;
use serde::{Deserialize, Serialize};

pub const TEST_MANIFEST_SCHEMA: &str = "once.test_manifest.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestUnit {
    pub id: String,
    pub name: String,
    pub suite: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestSharding {
    pub supported: bool,
    pub granularity: String,
}

impl Default for TestSharding {
    fn default() -> Self {
        Self {
            supported: false,
            granularity: "target".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestManifest {
    pub schema: String,
    pub id: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
    pub source: String,
    pub listing_supported: bool,
    pub case_filtering: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery_fingerprint: Option<String>,
    #[serde(default)]
    pub sharding: TestSharding,
    pub units: Vec<TestUnit>,
}

impl TestManifest {
    pub fn new(
        target: impl Into<String>,
        runner: Option<String>,
        source: impl Into<String>,
        listing_supported: bool,
        case_filtering: impl Into<String>,
        sharding: TestSharding,
        mut units: Vec<TestUnit>,
    ) -> Result<Self> {
        let target = target.into();
        ensure!(!target.is_empty(), "test manifest target cannot be empty");
        for unit in &units {
            ensure!(!unit.id.is_empty(), "test unit id cannot be empty");
        }
        units.sort_by(|left, right| left.id.cmp(&right.id));
        units.dedup_by(|left, right| left.id == right.id);
        let source = source.into();
        let case_filtering = case_filtering.into();
        let mut manifest = Self {
            schema: TEST_MANIFEST_SCHEMA.to_string(),
            id: String::new(),
            target,
            runner,
            source,
            listing_supported,
            case_filtering,
            discovery_fingerprint: None,
            sharding,
            units,
        };
        manifest.refresh_id()?;
        Ok(manifest)
    }

    pub fn with_discovery_fingerprint(mut self, fingerprint: Option<String>) -> Result<Self> {
        self.discovery_fingerprint = fingerprint;
        self.refresh_id()?;
        Ok(self)
    }

    fn refresh_id(&mut self) -> Result<()> {
        self.id = stable_id(
            "test-manifest",
            &(
                &self.target,
                &self.runner,
                &self.source,
                self.listing_supported,
                &self.case_filtering,
                &self.discovery_fingerprint,
                &self.sharding,
                &self.units,
            ),
        )?;
        Ok(())
    }
}

fn stable_id<T: Serialize>(domain: &str, value: &T) -> Result<String> {
    let mut material = domain.as_bytes().to_vec();
    material.push(0);
    material.extend(serde_json::to_vec(value).context("serializing test manifest identity")?);
    Ok(Digest::of_bytes(&material).to_string())
}

#[cfg(test)]
mod tests;
