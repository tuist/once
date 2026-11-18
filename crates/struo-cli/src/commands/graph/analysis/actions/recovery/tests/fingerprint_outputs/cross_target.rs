use super::*;

#[tokio::test]
async fn dependency_fingerprint_cannot_be_restored_as_a_file_in_another_target() {
    let workspace = Workspace::new();
    let producers = Producers::default();
    let mut source = graph(
        &format!("mkdir {PRODUCED}; printf original > {PRODUCED}/child"),
        false,
        &format!("cat {PRODUCED}/child > .struo/out/consumed.txt"),
    );
    source.actions.pop();
    let produced = workspace
        .build_with(source, &producers, None)
        .await
        .unwrap();
    assert_eq!(produced.cache_state, EvidenceCacheState::Bypass);
    let mut dependency_inputs = produced.available_inputs;
    for input in dependency_inputs.values_mut() {
        input.same_target = false;
    }
    let mut destination = graph(
        "unused",
        true,
        &format!("cat {PRODUCED}/child > .struo/out/consumed.txt"),
    );
    destination.actions.remove(0);
    let mut consumer_target = target();
    consumer_target.label.name = "consumer".to_string();
    consumer_target.label.id = "tools/consumer".to_string();
    let workers = crate::bus_events::WorkerSlots::new(4);
    let error = super::super::super::super::run_declared_actions_with(
        None,
        workspace.path(),
        &workspace.cache,
        Digest::of_bytes(b"modules"),
        &consumer_target,
        "build",
        destination,
        &[],
        &dependency_inputs,
        &BTreeMap::new(),
        Some(&workspace.digests),
        SandboxMode::Off,
        &workspace.resources,
        None,
        None,
        &workers,
        &producers,
    )
    .await
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("fingerprint-only output"),
        "{error:#}"
    );
    assert!(!workspace.path().join(PRODUCED).exists());
    assert!(!workspace.path().join(".struo/out/consumed.txt").exists());
}
