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
pub struct TestManifest {
    pub schema: String,
    pub id: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
    pub source: String,
    pub listing_supported: bool,
    pub case_filtering: String,
    pub units: Vec<TestUnit>,
}

impl TestManifest {
    pub fn new(
        target: impl Into<String>,
        runner: Option<String>,
        source: impl Into<String>,
        listing_supported: bool,
        case_filtering: impl Into<String>,
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
        let id = stable_id(
            "test-manifest",
            &(
                &target,
                &runner,
                &source,
                listing_supported,
                &case_filtering,
                &units,
            ),
        )?;
        Ok(Self {
            schema: TEST_MANIFEST_SCHEMA.to_string(),
            id,
            target,
            runner,
            source,
            listing_supported,
            case_filtering,
            units,
        })
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
