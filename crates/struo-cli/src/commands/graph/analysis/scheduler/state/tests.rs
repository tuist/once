use std::collections::BTreeSet;

use super::*;
use struo_cas::ActionResult;
use struo_frontend::TargetLabel;

fn target(name: &str, deps: &[&str], roles: &[(&str, Vec<&str>)]) -> Arc<GraphTarget> {
    Arc::new(GraphTarget {
        label: TargetLabel {
            package: String::new(),
            name: name.to_string(),
            id: name.to_string(),
        },
        kind: "test".to_string(),
        deps: deps.iter().map(|dep| (*dep).to_string()).collect(),
        dependency_edges: roles
            .iter()
            .map(|(role, deps)| {
                (
                    (*role).to_string(),
                    deps.iter().map(|dep| (*dep).to_string()).collect(),
                )
            })
            .collect(),
        srcs: Vec::new(),
        visibility: Vec::new(),
        attrs: BTreeMap::new(),
        capabilities: Vec::new(),
        providers: Vec::new(),
        tools: Vec::new(),
        diagnostics: Vec::new(),
    })
}

fn outcome() -> BuildOutcome {
    let digest = Digest::of_bytes(b"output");
    BuildOutcome {
        provider: Arc::new(serde_json::json!({"output": "out"})),
        action_digest: Digest::of_bytes(b"action"),
        input_digest: None,
        input_fingerprint: None,
        available_inputs: BTreeMap::from([(
            "out".to_string(),
            AvailableInput {
                blob_digest: digest,
                same_target: true,
                materialized: false,
            },
        )]),
        outputs: vec!["out".to_string()],
        cache_tag: "hit",
        cache_state: struo_core::EvidenceCacheState::Hit,
        result: ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs: BTreeMap::from([("out".to_string(), digest)]),
        },
        cached_results: Vec::new(),
        intermediate_outputs: BTreeSet::new(),
        per_action_outcomes: Vec::new(),
    }
}

#[test]
fn the_last_reader_moves_an_unretained_outcome() {
    let dep = target("dep", &[], &[]);
    let root = target("root", &["dep"], &[]);
    let targets = HashMap::from([
        ("dep".to_string(), dep),
        ("root".to_string(), Arc::clone(&root)),
    ]);
    let reachable = targets.keys().cloned().collect();
    let retained = HashSet::from(["root".to_string()]);
    let mut state = BuildState::new(&targets, &reachable, &retained).unwrap();
    let outcome = outcome();
    let provider_pointer = Arc::as_ptr(&outcome.provider);
    state.record_completion("dep", outcome).unwrap();

    let inputs = state.dependency_inputs(&root, &reachable).unwrap();

    assert!(!state.outcomes.contains_key("dep"));
    assert_eq!(Arc::as_ptr(&inputs.providers[0]), provider_pointer);
    assert_eq!(Arc::strong_count(&inputs.providers[0]), 1);
    assert!(!inputs.available_inputs["out"].same_target);
    assert!(!inputs.available_inputs["out"].materialized);
    assert_eq!(state.ready.len(), 2);
}

#[test]
fn repeated_roles_keep_an_outcome_alive_until_every_edge_is_read() {
    let dep = target("dep", &[], &[]);
    let root = target("root", &["dep"], &[("tools", vec!["dep", "dep"])]);
    let targets = HashMap::from([
        ("dep".to_string(), dep),
        ("root".to_string(), Arc::clone(&root)),
    ]);
    let reachable = targets.keys().cloned().collect();
    let mut state = BuildState::new(&targets, &reachable, &HashSet::new()).unwrap();
    state.record_completion("dep", outcome()).unwrap();
    assert_eq!(state.remaining_readers["dep"], 3);

    let inputs = state.dependency_inputs(&root, &reachable).unwrap();

    assert_eq!(inputs.providers.len(), 1);
    assert_eq!(inputs.providers_by_role["tools"].len(), 2);
    assert_eq!(inputs.action_digests.len(), 3);
    assert!(!state.outcomes.contains_key("dep"));
    assert!(inputs.providers_by_role["tools"]
        .iter()
        .all(|provider| Arc::ptr_eq(provider, &inputs.providers[0])));
}

#[test]
fn a_retained_outcome_survives_its_last_dependency_reader() {
    let dep = target("dep", &[], &[]);
    let root = target("root", &["dep"], &[]);
    let targets = HashMap::from([
        ("dep".to_string(), dep),
        ("root".to_string(), Arc::clone(&root)),
    ]);
    let reachable = targets.keys().cloned().collect();
    let mut state = BuildState::new(&targets, &reachable, &reachable).unwrap();
    state.record_completion("dep", outcome()).unwrap();

    let inputs = state.dependency_inputs(&root, &reachable).unwrap();

    assert!(state.outcomes.contains_key("dep"));
    assert!(state.outcomes["dep"].available_inputs["out"].same_target);
    assert!(!inputs.available_inputs["out"].same_target);
    assert!(Arc::ptr_eq(
        &state.outcomes["dep"].provider,
        &inputs.providers[0]
    ));
}
