use super::super::*;

#[tokio::test]
async fn a_batched_miss_does_not_mark_its_cached_sibling_materialized() {
    let workspace = tempfile::tempdir().unwrap();
    let cache_root = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(cache_root.path()));
    let actions = |bytes: &[u8], destination: &str| {
        vec![
            write("cached", b"cached bytes", false),
            write("miss", bytes, false),
            copy("cached", destination, false),
        ]
    };
    run(workspace.path(), &cache, actions(b"first", "first-copy")).await;
    for path in ["cached", "miss", "first-copy"] {
        std::fs::remove_file(workspace.path().join(path)).unwrap();
    }
    let outcome = run(workspace.path(), &cache, actions(b"second", "second-copy")).await;
    assert_eq!(outcome.result.exit_code, 0);
    assert_eq!(
        std::fs::read(workspace.path().join("second-copy")).unwrap(),
        b"cached bytes"
    );
    assert_eq!(
        std::fs::read(workspace.path().join("miss")).unwrap(),
        b"second"
    );
}

#[tokio::test]
async fn assembled_directory_inputs_include_deferred_children_and_keep_their_key() {
    let workspace = tempfile::tempdir().unwrap();
    let cache_root = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(cache_root.path()));
    run(
        workspace.path(),
        &cache,
        vec![write("stage/a", b"alpha", false)],
    )
    .await;
    std::fs::remove_dir_all(workspace.path().join("stage")).unwrap();
    let actions = || {
        let mut tree = copy("stage", "published", false);
        if let Some(DeclaredActionOperation::CopyPath { mode, .. }) = &mut tree.operation {
            *mode = DeclaredCopyPathMode::Tree;
        }
        vec![
            write("stage/a", b"alpha", false),
            write("stage/nested/b", b"beta", false),
            write("published/MANIFEST", b"intermediate", false),
            tree,
        ]
    };
    let digests = SourceDigestCache::open(workspace.path());
    let first = run_with_digests(workspace.path(), &cache, actions(), Some(&digests)).await;
    for directory in ["stage", "published"] {
        std::fs::remove_dir_all(workspace.path().join(directory)).unwrap();
    }
    digests.with_known_changes(
        super::super::super::source_digest_cache::KnownChanges::Since {
            sources: BTreeSet::new(),
            outputs: BTreeSet::new(),
        },
    );
    let restored = run_with_digests(workspace.path(), &cache, actions(), Some(&digests)).await;
    materialize_prior_cached_results(
        workspace.path(),
        &cache,
        &restored.cached_results,
        Some(&digests),
    )
    .await
    .unwrap();
    assert_eq!(first.action_digest, restored.action_digest);
    assert_eq!(restored.cache_state, EvidenceCacheState::Hit);
    assert_eq!(
        std::fs::read(workspace.path().join("published/a")).unwrap(),
        b"alpha"
    );
    assert_eq!(
        std::fs::read(workspace.path().join("published/nested/b")).unwrap(),
        b"beta"
    );
}

fn write(path: &str, bytes: &[u8], ordered: bool) -> DeclaredAction {
    serde_json::from_value(serde_json::json!({
        "outputs": [path], "depends_on_prior_actions": ordered, "identifier": path,
        "operation": {"kind": "write_file", "path": path, "bytes": bytes}
    }))
    .unwrap()
}

fn copy(source: &str, destination: &str, ordered: bool) -> DeclaredAction {
    serde_json::from_value(serde_json::json!({
        "inputs": [source], "outputs": [destination], "depends_on_prior_actions": ordered,
        "identifier": destination,
        "operation": {"kind": "copy_path", "sources": [source], "destination": destination, "mode": "file"}
    })).unwrap()
}

async fn run(
    workspace: &Path,
    cache: &CacheProvider,
    actions: Vec<DeclaredAction>,
) -> BuildOutcome {
    run_with_digests(workspace, cache, actions, None).await
}

async fn run_with_digests(
    workspace: &Path,
    cache: &CacheProvider,
    actions: Vec<DeclaredAction>,
    digests: Option<&SourceDigestCache>,
) -> BuildOutcome {
    let target: GraphTarget = serde_json::from_value(serde_json::json!({
        "label": {"package": "", "name": "pending", "id": "pending"}, "kind": "fixture", "capabilities": []
    })).unwrap();
    let analysis = AnalysisResult {
        actions,
        provider: serde_json::json!({}),
        declared_outputs: Vec::new(),
        observations: once_frontend::analysis::AnalysisObservations::default(),
    };
    let resources = Arc::new(ResourcePool::new(once_core::ResourceLimits::new(
        4,
        1024 * 1024 * 1024,
    )));
    let workers = crate::bus_events::WorkerSlots::new(4);
    run_declared_actions(
        None,
        workspace,
        cache,
        Digest::of_bytes(b"pending-tests"),
        &target,
        "build",
        analysis,
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        digests,
        SandboxMode::Off,
        &resources,
        None,
        None,
        &workers,
    )
    .await
    .unwrap()
}

fn prefix() -> Vec<DeclaredAction> {
    vec![
        write("prior.txt", b"prior", true),
        write("needed.txt", b"needed", true),
    ]
}

async fn prime(workspace: &Path, cache: &CacheProvider) {
    run(workspace, cache, prefix()).await;
    std::fs::remove_file(workspace.join("prior.txt")).unwrap();
    std::fs::remove_file(workspace.join("needed.txt")).unwrap();
}

#[tokio::test]
async fn independent_misses_and_bypasses_keep_unconsumed_cached_outputs() {
    for cacheable in [true, false] {
        let workspace = tempfile::tempdir().unwrap();
        let cache_root = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(cache_root.path()));
        prime(workspace.path(), &cache).await;
        let mut actions = prefix();
        let mut last = copy("needed.txt", "copy.txt", false);
        last.cacheable = cacheable;
        actions.push(last);
        let outcome = run(workspace.path(), &cache, actions).await;
        assert_eq!(
            std::fs::read(workspace.path().join("copy.txt")).unwrap(),
            b"needed"
        );
        if super::super::super::materialization::disjoint_paths(
            workspace.path(),
            &BTreeSet::from(["needed.txt".into()]),
            &BTreeSet::from(["copy.txt".into()]),
        ) {
            assert!(outcome.available_inputs["needed.txt"].materialized);
        }
        if super::super::super::materialization::disjoint_paths(
            workspace.path(),
            &BTreeSet::from(["prior.txt".into()]),
            &BTreeSet::from(["copy.txt".into()]),
        ) {
            assert!(!workspace.path().join("prior.txt").exists());
            assert_eq!(
                outcome
                    .cached_results
                    .iter()
                    .flat_map(|result| result.outputs.keys())
                    .collect::<Vec<_>>(),
                vec!["prior.txt"]
            );
        }
        materialize_prior_cached_results(workspace.path(), &cache, &outcome.cached_results, None)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(workspace.path().join("prior.txt")).unwrap(),
            b"prior"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn ordered_action_after_an_independent_miss_sees_implicit_cached_inputs() {
    let workspace = tempfile::tempdir().unwrap();
    let cache_root = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(cache_root.path()));
    prime(workspace.path(), &cache).await;
    let mut actions = prefix();
    actions.push(copy("needed.txt", "copy.txt", false));
    actions.push(serde_json::from_value(serde_json::json!({
        "argv": ["/bin/cp", "prior.txt", "final.txt"], "outputs": ["final.txt"], "identifier": "implicit"
    })).unwrap());
    let outcome = run(workspace.path(), &cache, actions).await;
    assert_eq!(
        std::fs::read(workspace.path().join("final.txt")).unwrap(),
        b"prior"
    );
    assert!(outcome.cached_results.is_empty());
}

#[tokio::test]
async fn parallel_independent_misses_preserve_pending_prior_outputs() {
    let workspace = tempfile::tempdir().unwrap();
    let cache_root = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(cache_root.path()));
    prime(workspace.path(), &cache).await;
    let mut actions = prefix();
    actions.extend([
        copy("needed.txt", "a.txt", false),
        copy("needed.txt", "b.txt", false),
    ]);
    let outcome = run(workspace.path(), &cache, actions).await;
    for path in ["a.txt", "b.txt"] {
        assert_eq!(
            std::fs::read(workspace.path().join(path)).unwrap(),
            b"needed"
        );
    }
    materialize_prior_cached_results(workspace.path(), &cache, &outcome.cached_results, None)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(workspace.path().join("prior.txt")).unwrap(),
        b"prior"
    );
}

#[tokio::test]
async fn child_mutation_restores_its_cached_tree_before_execution() {
    let workspace = tempfile::tempdir().unwrap();
    let cache_root = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(cache_root.path()));
    std::fs::create_dir(workspace.path().join("seed")).unwrap();
    std::fs::write(workspace.path().join("seed/item.txt"), b"old").unwrap();
    std::fs::write(workspace.path().join("seed/keep.txt"), b"keep").unwrap();
    let tree: DeclaredAction = serde_json::from_value(serde_json::json!({
        "inputs": ["seed"], "outputs": ["tree"], "identifier": "tree",
        "operation": {"kind": "copy_path", "sources": ["seed"], "destination": "tree", "mode": "tree"}
    })).unwrap();
    run(workspace.path(), &cache, vec![tree.clone()]).await;
    std::fs::remove_dir_all(workspace.path().join("tree")).unwrap();
    let outcome = run(
        workspace.path(),
        &cache,
        vec![tree, write("tree/item.txt", b"fresh", false)],
    )
    .await;
    materialize_prior_cached_results(workspace.path(), &cache, &outcome.cached_results, None)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(workspace.path().join("tree/item.txt")).unwrap(),
        b"fresh"
    );
    assert_eq!(
        std::fs::read(workspace.path().join("tree/keep.txt")).unwrap(),
        b"keep"
    );
    assert!(outcome.available_inputs["tree"].materialized);
}

#[tokio::test]
async fn arg_file_mutation_restores_its_cached_tree_before_execution() {
    let workspace = tempfile::tempdir().unwrap();
    let cache_root = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(cache_root.path()));
    std::fs::create_dir(workspace.path().join("seed")).unwrap();
    std::fs::write(workspace.path().join("seed/item.txt"), b"old").unwrap();
    std::fs::write(workspace.path().join("seed/keep.txt"), b"keep").unwrap();
    let tree: DeclaredAction = serde_json::from_value(serde_json::json!({
        "inputs": ["seed"], "outputs": ["tree"], "identifier": "tree",
        "operation": {"kind": "copy_path", "sources": ["seed"], "destination": "tree", "mode": "tree"}
    })).unwrap();
    run(workspace.path(), &cache, vec![tree.clone()]).await;
    std::fs::remove_dir_all(workspace.path().join("tree")).unwrap();
    let mut action = write("marker.txt", b"fresh", false);
    action.arg_files.push(DeclaredArgFile {
        path: "tree/item.txt".into(),
        format: DeclaredArgFileFormat::LineDelimited,
        args: vec!["replacement".into()],
    });
    let outcome = run(workspace.path(), &cache, vec![tree, action]).await;
    materialize_prior_cached_results(workspace.path(), &cache, &outcome.cached_results, None)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(workspace.path().join("tree/item.txt")).unwrap(),
        b"replacement\n"
    );
    assert_eq!(
        std::fs::read(workspace.path().join("tree/keep.txt")).unwrap(),
        b"keep"
    );
}

#[tokio::test]
async fn a_parent_input_replacement_keeps_overwritten_child_overlays_pending() {
    for later_reader in [false, true] {
        let workspace = tempfile::tempdir().unwrap();
        let cache_root = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(cache_root.path()));
        std::fs::create_dir(workspace.path().join("seed")).unwrap();
        std::fs::write(workspace.path().join("seed/item.txt"), b"parent bytes").unwrap();
        let tree: DeclaredAction = serde_json::from_value(serde_json::json!({
            "inputs": ["seed"], "outputs": ["tree"], "identifier": "tree",
            "operation": {"kind": "copy_path", "sources": ["seed"], "destination": "tree", "mode": "tree"}
        })).unwrap();
        let overlay = write("tree/item.txt", b"overlay bytes", true);
        let prefix = vec![tree, overlay];
        run(workspace.path(), &cache, prefix.clone()).await;
        std::fs::remove_dir_all(workspace.path().join("tree")).unwrap();
        let mut actions = prefix;
        actions.push(serde_json::from_value(serde_json::json!({
            "inputs": ["tree"], "outputs": ["copy"], "identifier": "copy",
            "depends_on_prior_actions": false,
            "operation": {"kind": "copy_path", "sources": ["tree"], "destination": "copy", "mode": "tree"}
        })).unwrap());
        if later_reader {
            actions.push(copy("tree/item.txt", "read.txt", true));
        }
        let outcome = run(workspace.path(), &cache, actions).await;
        assert_eq!(
            std::fs::read(workspace.path().join("copy/item.txt")).unwrap(),
            b"parent bytes"
        );
        if later_reader {
            assert_eq!(
                std::fs::read(workspace.path().join("read.txt")).unwrap(),
                b"overlay bytes"
            );
        } else {
            assert!(!outcome.available_inputs["tree/item.txt"].materialized);
            assert!(outcome
                .cached_results
                .iter()
                .any(|result| { result.outputs.contains_key("tree/item.txt") }));
        }
        materialize_prior_cached_results(workspace.path(), &cache, &outcome.cached_results, None)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(workspace.path().join("tree/item.txt")).unwrap(),
            b"overlay bytes"
        );
    }
}

#[tokio::test]
async fn miss_overwriting_cached_output_cannot_be_replaced_by_late_replay() {
    let workspace = tempfile::tempdir().unwrap();
    let cache_root = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(once_cas::Cas::open(cache_root.path()));
    prime(workspace.path(), &cache).await;
    let mut actions = prefix();
    actions.push(write("prior.txt", b"fresh", false));
    let outcome = run(workspace.path(), &cache, actions).await;
    materialize_prior_cached_results(workspace.path(), &cache, &outcome.cached_results, None)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(workspace.path().join("prior.txt")).unwrap(),
        b"fresh"
    );
}
