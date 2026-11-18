mod fingerprint;

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use struo_core::{TestBatch, TestManifest, TestPlan};
use struo_frontend::GraphTarget;

pub(crate) use fingerprint::discovery_fingerprint;

pub(crate) const SCHEMA: &str = "struo.portable_test_plan.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PortableTestPlan {
    pub schema: String,
    pub id: String,
    pub plan: TestPlan,
    pub bindings: Vec<Binding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Binding {
    pub target: String,
    pub fingerprint: String,
    pub manifest: Option<TestManifest>,
}

impl PortableTestPlan {
    pub fn export(workspace: &Path, graph: &[GraphTarget], plan: TestPlan) -> Result<Self> {
        plan.validate()?;
        let mut bindings = Vec::new();
        for selected in &plan.selection.tests {
            let fingerprint = discovery_fingerprint(workspace, graph, &selected.id)?;
            let stored = super::super::stored_test_manifest_record(workspace, &selected.id)?;
            let manifest = stored
                .filter(|manifest| manifest.discovery_fingerprint.as_deref() == Some(&fingerprint))
                .map(|manifest| portable_manifest(&manifest))
                .transpose()?;
            bindings.push(Binding {
                target: selected.id.clone(),
                fingerprint,
                manifest: manifest.filter(shardable),
            });
        }
        let mut portable = Self {
            schema: SCHEMA.to_string(),
            id: String::new(),
            plan,
            bindings,
        };
        portable.id = portable.identity()?;
        portable.validate(workspace, graph)?;
        Ok(portable)
    }

    fn identity(&self) -> Result<String> {
        let mut digest = struo_core::InputDigestBuilder::new(SCHEMA.as_bytes());
        digest.push_bytes(&serde_json::to_vec(&(&self.plan, &self.bindings))?);
        Ok(digest.finish().to_string())
    }

    pub fn validate(&self, workspace: &Path, graph: &[GraphTarget]) -> Result<()> {
        ensure!(
            self.schema == SCHEMA,
            "unsupported portable test plan schema"
        );
        ensure!(
            self.id == self.identity()?,
            "portable test plan identity mismatch"
        );
        self.plan.validate()?;
        ensure!(
            self.bindings.len() == self.plan.selection.tests.len(),
            "portable plan bindings do not match selected targets"
        );
        for (binding, selected) in self.bindings.iter().zip(&self.plan.selection.tests) {
            ensure!(
                binding.target == selected.id,
                "portable plan bindings are not canonical"
            );
            ensure!(
                binding.fingerprint == discovery_fingerprint(workspace, graph, &binding.target)?,
                "portable test plan is stale for `{}`; export a new plan after complete discovery",
                binding.target
            );
            let expected = match &binding.manifest {
                Some(manifest) => {
                    ensure!(
                        manifest.target == binding.target && shardable(manifest),
                        "invalid portable discovery manifest"
                    );
                    ensure!(
                        *manifest == portable_manifest(manifest)?,
                        "portable discovery manifest is not canonical"
                    );
                    super::batches_for_manifest(selected, manifest.clone())?
                }
                None => vec![TestBatch::new(&binding.target, vec![])?],
            };
            let expected = TestPlan::new(self.plan.selection.clone(), expected)?.batches;
            let actual = self
                .plan
                .batches
                .iter()
                .filter(|batch| batch.target == binding.target)
                .cloned()
                .collect::<Vec<_>>();
            ensure!(actual == expected, "portable test scopes do not cover complete discovery or split a fixture boundary for `{}`", binding.target);
        }
        Ok(())
    }

    pub fn select(&self, ids: &[String]) -> Result<TestPlan> {
        if ids.is_empty() {
            return Ok(self.plan.clone());
        }
        let mut selected = BTreeSet::new();
        for id in ids {
            ensure!(
                selected.insert(id.as_str()),
                "duplicate selected test batch `{id}`"
            );
            ensure!(
                self.plan.batches.iter().any(|batch| batch.id == *id),
                "unknown portable test batch `{id}`"
            );
        }
        let batches = self
            .plan
            .batches
            .iter()
            .filter(|batch| selected.contains(batch.id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        let targets = batches
            .iter()
            .map(|batch| &batch.target)
            .collect::<BTreeSet<_>>();
        let mut selection = self.plan.selection.clone();
        selection.tests.retain(|test| targets.contains(&test.id));
        selection.policy.evidence = "portable_test_batches".to_string();
        let plan = TestPlan::new(selection, batches)?;
        plan.validate()?;
        Ok(plan)
    }

    pub fn read(path: &Path) -> Result<Self> {
        serde_json::from_reader(std::fs::File::open(path).context("opening portable test plan")?)
            .context("reading portable test plan")
    }
}

pub(super) fn shardable(manifest: &TestManifest) -> bool {
    manifest.source == "normalized_results"
        && manifest.listing_supported
        && manifest.case_filtering == "runner_args"
        && manifest.sharding.supported
        && !manifest.units.is_empty()
}

fn portable_manifest(manifest: &TestManifest) -> Result<TestManifest> {
    TestManifest::new(
        &manifest.target,
        manifest.runner.clone(),
        &manifest.source,
        manifest.listing_supported,
        &manifest.case_filtering,
        manifest.sharding.clone(),
        manifest.units.clone(),
    )
}

#[cfg(test)]
mod tests;
