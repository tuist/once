use super::*;

#[cfg(unix)]
#[tokio::test]
async fn independent_miss_resolves_aliases_only_after_resource_admission() {
    let workspace = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let cache_root = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(struo_cas::Cas::open(cache_root.path()));
    let resources = Arc::new(ResourcePool::new(struo_core::ResourceLimits::new(
        1,
        1024 * 1024 * 1024,
    )));
    std::fs::create_dir(source.path().join("seed")).unwrap();
    std::fs::write(source.path().join("seed/keep.txt"), b"keep").unwrap();
    let capture = Action::CopyPath {
        sources: vec![WorkspacePath::try_from("seed").unwrap()],
        destination: WorkspacePath::try_from("tree").unwrap(),
        mode: CopyPathMode::Tree,
        input_digest: None,
    };
    let prior = {
        let _permit = resources.acquire(capture.resource_request().clone()).await;
        struo_core::run_uncached(&capture, source.path(), &cache, false)
            .await
            .unwrap()
    };
    let available = prior
        .outputs
        .iter()
        .map(|(path, digest)| {
            (
                path.clone(),
                AvailableInput {
                    blob_digest: *digest,
                    same_target: true,
                    materialized: false,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let declared: DeclaredAction = serde_json::from_value(serde_json::json!({
        "outputs": ["alias/item.txt"], "depends_on_prior_actions": false,
        "operation": {"kind": "write_file", "path": "alias/item.txt", "bytes": b"fresh"}
    }))
    .unwrap();
    let prepared = prepare_declared_action_with_inputs(
        workspace.path(),
        &declared,
        Digest::of_bytes(b"admission"),
        &[],
        &available,
        None,
        SandboxMode::Off,
    )
    .unwrap();
    let held = resources.acquire(ResourceRequest::default()).await;
    let root = workspace.path().to_path_buf();
    let pool = Arc::clone(&resources);
    let (started, waiting) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let context = DeclaredActionContext {
            workspace: &root,
            cache: &cache,
            source_digest_cache: None,
            available_inputs: &available,
            prior_cached_results: PendingResults {
                restored_at_end: &[prior],
                intermediate: &[],
            },
            target_id: "admission",
            capability: "build",
            index: 1,
            identifier: "write",
            argv: &[],
            arg_files: &[],
            input_fingerprint: prepared.input_fingerprint,
            record_success_evidence: false,
            resources: &pool,
            output_observer: None,
        };
        started.send(()).unwrap();
        execute_declared_cache_miss(&context, &prepared.action, &declared)
            .await
            .unwrap()
    });
    waiting.await.unwrap();
    std::fs::create_dir(workspace.path().join("tree")).unwrap();
    std::fs::remove_dir(workspace.path().join("alias")).unwrap();
    std::os::unix::fs::symlink("tree", workspace.path().join("alias")).unwrap();
    drop(held);
    let (outcome, restored) = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.result.exit_code, 0);
    assert!(restored.paths.contains("tree"));
    assert_eq!(
        std::fs::read(workspace.path().join("tree/keep.txt")).unwrap(),
        b"keep"
    );
    assert_eq!(
        std::fs::read(workspace.path().join("tree/item.txt")).unwrap(),
        b"fresh"
    );
}
