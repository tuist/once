use super::*;
use crate::commands::graph::analysis::actions::{legacy_outputs::FingerprintKind, Producers};
use crate::commands::graph::analysis::AvailableInput;

#[test]
fn historical_target_outcome_cannot_bypass_fingerprint_dependency_checks() {
    let workspace = TempDir::new().unwrap();
    let target = target("apps/tool", "tool");
    let output = ".once/out/tool/tool";
    std::fs::create_dir_all(workspace.path().join(".once/out/tool")).unwrap();
    std::fs::write(workspace.path().join(output), b"poisoned manifest bytes").unwrap();
    let reopened = round_trip(&workspace, |outcomes| {
        let mut stored = outcome(&[output]);
        stored.result.outputs.insert(
            output.to_string(),
            Digest::of_bytes(b"poisoned manifest bytes"),
        );
        outcomes.record(
            &target,
            "key".to_string(),
            &AnalysisObservations::default(),
            &inputs(&[]),
            &stored,
        );
    });
    assert!(reopened.reuse(&target, "key", &changed(&[], &[])).is_some());
    let digest = Digest::of_bytes(b"once.directory_output.v1\n");
    let dependency_inputs = BTreeMap::from([(
        "gen".to_string(),
        AvailableInput {
            blob_digest: digest,
            materialized: true,
            same_target: false,
        },
    )]);
    let producers = Producers::default();
    producers.record_fingerprint("gen", digest, FingerprintKind::Directory, "produce");
    assert!(reopened
        .reuse_with_inputs(
            &target,
            "key",
            &changed(&[], &[]),
            &dependency_inputs,
            &producers,
            |_| true,
        )
        .is_none());
    assert!(reopened
        .reuse_with_inputs(
            &target,
            "key",
            &changed(&[], &[]),
            &dependency_inputs,
            &Producers::default(),
            |_| true,
        )
        .is_some());
    assert!(reopened
        .reuse_with_inputs(
            &target,
            "key",
            &changed(&[], &[]),
            &BTreeMap::new(),
            &producers,
            |_| true,
        )
        .is_some());
}
