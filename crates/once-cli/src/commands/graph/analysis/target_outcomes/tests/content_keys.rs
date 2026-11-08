use super::*;
use crate::commands::graph::analysis::actions::{legacy_outputs::FingerprintKind, Producers};
use crate::commands::graph::analysis::AvailableInput;

fn available(bytes: &[u8]) -> AvailableInput {
    AvailableInput {
        blob_digest: Digest::of_bytes(bytes),
        same_target: false,
        materialized: true,
    }
}

#[test]
fn local_outcome_key_tracks_contents_even_when_producer_actions_are_unchanged() {
    let actions = vec![("producer".to_string(), Digest::of_bytes(b"same action"))];
    let mut inputs = BTreeMap::from([("gen".to_string(), available(b"before"))]);
    let before = super::super::key("analysis", &actions, &inputs);
    inputs.get_mut("gen").unwrap().materialized = false;
    inputs.get_mut("gen").unwrap().same_target = true;
    assert_eq!(before, super::super::key("analysis", &actions, &inputs));
    inputs.get_mut("gen").unwrap().blob_digest = Digest::of_bytes(b"after");
    assert_ne!(before, super::super::key("analysis", &actions, &inputs));
    let renamed = BTreeMap::from([("other".to_string(), available(b"before"))]);
    assert_ne!(before, super::super::key("analysis", &actions, &renamed));
}

#[test]
fn local_outcome_key_canonicalizes_dependency_order_and_component_boundaries() {
    let actions = [
        ("one".to_string(), Digest::of_bytes(b"one")),
        ("two".to_string(), Digest::of_bytes(b"two")),
    ];
    assert_eq!(
        super::super::key("analysis", &actions, &BTreeMap::new()),
        super::super::key(
            "analysis",
            &[actions[1].clone(), actions[0].clone()],
            &BTreeMap::new()
        )
    );
    let inputs = BTreeMap::from([
        ("one".to_string(), available(b"one")),
        ("two".to_string(), available(b"two")),
    ]);
    assert_ne!(
        super::super::key("analysis", &actions, &BTreeMap::new()),
        super::super::key("analysis", &[], &inputs)
    );
}

#[test]
fn intermediate_replacement_cannot_hide_changed_contents_from_historical_target_reuse() {
    let workspace = TempDir::new().unwrap();
    let target = target("apps/consumer", "consumer");
    let output = ".once/out/consumer/result";
    std::fs::create_dir_all(workspace.path().join(".once/out/consumer")).unwrap();
    std::fs::write(workspace.path().join(output), b"poisoned downstream result").unwrap();
    let actions = vec![(
        "transform".to_string(),
        Digest::of_bytes(b"unchanged transform action"),
    )];
    let poisoned = BTreeMap::from([(
        "gen".to_string(),
        available(b"old transformed manifest text"),
    )]);
    let old_key = super::super::key("analysis", &actions, &poisoned);
    let reopened = round_trip(&workspace, |outcomes| {
        let mut stored = outcome(&[output]);
        stored.result.outputs.insert(
            output.to_string(),
            Digest::of_bytes(b"poisoned downstream result"),
        );
        outcomes.record(
            &target,
            old_key.clone(),
            &AnalysisObservations::default(),
            &inputs(&[]),
            &stored,
        );
    });
    let producers = Producers::default();
    producers.record_fingerprint(
        "gen",
        Digest::of_bytes(b"original fingerprint"),
        FingerprintKind::Directory,
        "produce",
    );
    assert!(reopened
        .reuse_with_inputs(
            &target,
            &old_key,
            &changed(&[], &[]),
            &poisoned,
            &producers,
            |_| true
        )
        .is_some());
    let corrected = BTreeMap::from([(
        "gen".to_string(),
        available(b"correct transformed tree contents"),
    )]);
    assert!(!producers.has_fingerprint("gen", corrected["gen"].blob_digest));
    let new_key = super::super::key("analysis", &actions, &corrected);
    assert_ne!(old_key, new_key);
    assert!(reopened
        .reuse_with_inputs(
            &target,
            &new_key,
            &changed(&[], &[]),
            &corrected,
            &producers,
            |_| true,
        )
        .is_none());
}
