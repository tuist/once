use super::*;

#[tokio::test]
async fn admitted_uncached_capture_preserves_legacy_artifact_digests() {
    let workspace = Workspace::new();
    std::fs::write(workspace.path().join("file"), b"contents").unwrap();
    std::fs::create_dir(workspace.path().join("tree")).unwrap();
    std::fs::write(workspace.path().join("tree/child"), b"child").unwrap();
    std::os::unix::fs::symlink("file", workspace.path().join("link")).unwrap();
    std::os::unix::fs::symlink("missing", workspace.path().join("dangling")).unwrap();
    let outputs = ["file", "tree", "link", "dangling"]
        .map(|path| struo_core::WorkspacePath::try_from(path).unwrap());
    let capture = crate::commands::graph::analysis::actions::capture_uncached_outputs_admitted;
    let legacy = capture(&outputs, workspace.path(), &workspace.cache, None, None)
        .await
        .unwrap();
    let resources = struo_core::ExecutorResources::new(Arc::clone(&workspace.resources));
    let lease = resources
        .acquire(struo_core::Executor::Local, &ResourceRequest::default())
        .await;
    let admitted = capture(
        &outputs,
        workspace.path(),
        &workspace.cache,
        Some(&lease),
        None,
    )
    .await
    .unwrap();
    assert_eq!(legacy, admitted);
    assert_legacy_digests(&admitted);
}

fn assert_legacy_digests(outputs: &BTreeMap<String, Digest>) {
    assert_eq!(outputs["file"], Digest::of_bytes(b"contents"));
    assert_eq!(
        outputs["link"],
        Digest::of_bytes(b"struo.symlink_output.v1\nfile\n")
    );
    assert_eq!(
        outputs["dangling"],
        Digest::of_bytes(b"struo.symlink_output.v1\nmissing\n")
    );
    let manifest = format!(
        "struo.directory_output.v1\nfile\tchild\t{}\n",
        Digest::of_bytes(b"child")
    );
    assert_eq!(outputs["tree"], Digest::of_bytes(manifest.as_bytes()));
}

#[tokio::test]
async fn direct_spawn_graph_capture_preserves_pinned_legacy_outputs_and_dangling_links() {
    let workspace = Workspace::new();
    let producers = Producers::default();
    let analysis = AnalysisResult {
        actions: vec![action(serde_json::json!({
            "argv": ["/bin/sh", "-c", "printf contents > file; mkdir tree; printf child > tree/child; ln -s file link; ln -s missing dangling"],
            "outputs": ["file", "tree", "link", "dangling"], "cacheable": false,
        }))],
        provider: serde_json::json!({}),
        declared_outputs: Vec::new(),
        observations: AnalysisObservations::default(),
    };
    let outcome = workspace
        .build_with(analysis, &producers, None)
        .await
        .unwrap();
    assert_legacy_digests(&outcome.result.outputs);
}

#[tokio::test]
async fn cancelled_capture_step_retains_owned_admission_until_its_worker_returns() {
    let workspace = Workspace::new();
    let resources = struo_core::ExecutorResources::new(Arc::new(ResourcePool::new(
        struo_core::ResourceLimits::new(1, 1 << 30),
    )));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let lease = resources
        .acquire(struo_core::Executor::Local, &ResourceRequest::default())
        .await;
    let work = move || {
        let _ = started_tx.send(());
        release_rx.recv().unwrap();
        std::fs::write(workspace.path().join("captured"), b"stopped").unwrap();
    };
    let capture_step = crate::commands::graph::analysis::actions::capture_step;
    let mut capture = Box::pin(capture_step(Some(&lease), work));
    tokio::select! {
        result = &mut capture => panic!("capture worker finished before release: {result:?}"),
        _ = started_rx => {},
    }
    drop(capture);
    drop(lease);
    let held = tokio::time::timeout(
        Duration::from_millis(100),
        resources.acquire(struo_core::Executor::Local, &ResourceRequest::default()),
    )
    .await
    .is_err();
    release_tx.send(()).unwrap();
    assert!(held, "capture observation released physical-work admission");
    tokio::time::timeout(
        Duration::from_secs(5),
        resources.acquire(struo_core::Executor::Local, &ResourceRequest::default()),
    )
    .await
    .unwrap();
}
