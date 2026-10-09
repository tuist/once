use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{ensure, Context, Result};
use once_core::{TestManifest, TestPlan};
use once_frontend::GraphTarget;

use crate::commands::query;

pub(super) fn validate_plan_targets(
    workspace: &Path,
    graph: &[GraphTarget],
    plan: &TestPlan,
) -> Result<()> {
    plan.validate()?;
    let targets = graph
        .iter()
        .map(|target| (target.label.id.as_str(), target))
        .collect::<BTreeMap<_, _>>();
    let mut scopes = BTreeMap::<&str, BTreeSet<&str>>::new();
    for batch in &plan.batches {
        let target = targets
            .get(batch.target.as_str())
            .with_context(|| format!("no target matches `{}`", batch.target))?;
        ensure!(
            target
                .capabilities
                .iter()
                .any(|capability| capability.name == "test"),
            "target `{}` does not expose the test capability",
            batch.target
        );
        if !batch.test_filters.is_empty() {
            scopes
                .entry(&batch.target)
                .or_default()
                .extend(batch.test_filters.iter().map(String::as_str));
        }
    }
    let complete_scope = !matches!(
        plan.selection.policy.evidence.as_str(),
        "requested_test_unit" | "requested_test_batch"
    );
    for (target, units) in scopes {
        let manifest = query::test_manifest_record_with_graph(workspace, target, graph)?;
        ensure!(query::test_manifest_is_current_with_graph(workspace, target, &manifest, graph),
            "the test manifest for `{target}` is stale; run `once test {target}` to refresh discovery");
        validate_scope(&manifest, target, &units, complete_scope)?;
    }
    Ok(())
}

fn validate_scope(
    manifest: &TestManifest,
    target: &str,
    units: &BTreeSet<&str>,
    complete: bool,
) -> Result<()> {
    ensure!(
        manifest.case_filtering == "runner_args",
        "target `{target}` does not support explicit test-unit filtering"
    );
    let discovered = manifest
        .units
        .iter()
        .map(|unit| unit.id.as_str())
        .collect::<BTreeSet<_>>();
    for unit in units.difference(&discovered) {
        anyhow::bail!("test unit `{unit}` is not present in the current manifest for `{target}`; run `once test {target}` to refresh discovery");
    }
    if complete {
        ensure!(
            *units == discovered,
            "test batches do not cover the complete manifest for `{target}`"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_core::{TestSharding, TestUnit};

    fn manifest() -> TestManifest {
        TestManifest::new(
            "tests",
            None,
            "normalized_results",
            true,
            "runner_args",
            TestSharding::default(),
            ["a", "b"]
                .into_iter()
                .map(|name| TestUnit {
                    id: name.to_string(),
                    name: name.to_string(),
                    suite: "tests".to_string(),
                    file: None,
                })
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn complete_scope_must_cover_the_entire_inventory() {
        let manifest = manifest();
        assert!(
            validate_scope(&manifest, "tests", &BTreeSet::from(["a"]), true)
                .unwrap_err()
                .to_string()
                .contains("complete manifest")
        );
        validate_scope(&manifest, "tests", &BTreeSet::from(["a", "b"]), true).unwrap();
    }

    #[test]
    fn deliberate_partial_scope_still_rejects_unknown_units() {
        validate_scope(&manifest(), "tests", &BTreeSet::from(["a"]), false).unwrap();
        assert!(validate_scope(&manifest(), "tests", &BTreeSet::from(["missing"]), false).is_err());
    }
}
