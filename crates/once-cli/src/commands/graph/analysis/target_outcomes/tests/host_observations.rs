use super::*;
use crate::commands::graph::analysis::actions::Producers;
use once_frontend::analysis::{AnalysisEngine, CommandPolicy, UnchangedWorkspace};
use sha2::{Digest as _, Sha256};

#[test]
fn persisted_host_content_observations_invalidate_without_workspace_changes() {
    for (external, tree) in [(false, false), (true, false), (false, true), (true, true)] {
        let workspace = TempDir::new().unwrap();
        let host = TempDir::new().unwrap();
        let source = if external {
            host.path()
        } else {
            workspace.path()
        }
        .join("sources/source.rs");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "before").unwrap();
        let target = target("", "consumer");
        let observed_path = if tree {
            source.parent().unwrap()
        } else {
            &source
        };
        let mut observations = AnalysisObservations::default();
        observations.push_for_test(if tree {
            Observation::TreeDigest {
                path: observed_path.to_string_lossy().into_owned(),
                sha256: once_host_tree::host_tree_sha256_hex(observed_path).unwrap(),
            }
        } else {
            Observation::FileDigest {
                path: observed_path.to_string_lossy().into_owned(),
                sha256: hex::encode(Sha256::digest(b"before")),
            }
        });
        let reopened = round_trip(&workspace, |outcomes| {
            outcomes.record(
                &target,
                "key".to_string(),
                &observations,
                &inputs(&[]),
                &outcome(&[]),
            );
        });
        let reuse = || {
            let analyzer = AnalysisEngine::new().unwrap();
            let outcome = reopened.reuse_with_inputs(
                &target,
                "key",
                &changed(&[], &[]),
                &BTreeMap::new(),
                &Producers::default(),
                |recorded| {
                    assert_eq!(recorded, &observations);
                    analyzer.observations_hold(
                        workspace.path(),
                        recorded,
                        CommandPolicy::Rerun,
                        &UnchangedWorkspace::Except(BTreeSet::new()),
                    )
                },
            );
            assert!(analyzer.observed_host_paths().contains(observed_path));
            outcome
        };
        assert!(reuse().is_some());
        std::fs::write(&source, "after").unwrap();
        assert!(reuse().is_none());
        std::fs::remove_file(&source).unwrap();
        assert!(reuse().is_none());
    }
}

#[test]
fn incomplete_observations_are_not_saved_as_reusable_outcomes() {
    let workspace = TempDir::new().unwrap();
    let target = target("", "consumer");
    let mut observations = AnalysisObservations::default();
    observations.mark_incomplete_for_test();
    let reopened = round_trip(&workspace, |outcomes| {
        outcomes.record(
            &target,
            "key".to_string(),
            &observations,
            &inputs(&[]),
            &outcome(&[]),
        );
    });
    assert!(reopened.reuse(&target, "key", &changed(&[], &[])).is_none());
}
