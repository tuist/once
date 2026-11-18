use super::*;

#[tokio::test]
async fn repair_deadline_keeps_the_admission_and_action_lock_until_writing_stops() {
    let mut workspace = Workspace::new();
    workspace.resources = Arc::new(ResourcePool::new(struo_core::ResourceLimits::new(
        1,
        1 << 30,
    )));
    let producers = Producers::with_limits(RepairLimits {
        deadline: Some(Duration::from_mins(1)),
        ..Default::default()
    });
    let mut declared = action(
        serde_json::json!({ "operation": {"kind": "write_file", "path": "out.fifo", "bytes": []}, "outputs": ["out.fifo"], "cacheable": true }),
    );
    let Some(struo_frontend::analysis::DeclaredActionOperation::WriteFile { bytes, .. }) =
        &mut declared.operation
    else {
        panic!("wrong operation")
    };
    *bytes = vec![b'x'; 8 * 1024 * 1024];
    let analysis = AnalysisResult {
        actions: vec![declared],
        provider: serde_json::json!({}),
        declared_outputs: Vec::new(),
        observations: AnalysisObservations::default(),
    };
    let first = workspace
        .build_with(analysis, &producers, None)
        .await
        .unwrap();
    let digest = first.result.outputs["out.fifo"];
    std::fs::remove_file(workspace.path().join("out.fifo")).unwrap();
    std::fs::remove_file(workspace.blob_path(digest)).unwrap();
    let path = workspace.path().join("out.fifo");
    assert!(std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .unwrap()
        .success());
    let mut reader = Reader::open(&path);
    let repair = Repair {
        workspace: workspace.path(),
        cache: &workspace.cache,
        source_digest_cache: Some(&workspace.digests),
        resources: &workspace.resources,
        producers: &producers,
    };
    let result = single_output("out.fifo", digest);
    let mut restore = Box::pin(repair.restore(&result));
    tokio::select! {
        result = &mut restore => panic!("repair finished before the blocked write: {result:?}"),
        () = reader.started() => {},
    }
    tokio::time::pause();
    tokio::time::advance(Duration::from_mins(1)).await;
    let error = restore.await.unwrap_err();
    tokio::time::resume();
    assert!(
        format!("{error:#}").contains("did not finish within"),
        "unexpected repair failure: {error:#}"
    );
    let entry = std::fs::read_dir(workspace.path().join(".struo/locks/actions"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(entry.path())
        .unwrap();
    let locked = matches!(lock.try_lock(), Err(std::fs::TryLockError::WouldBlock));
    let held = tokio::time::timeout(
        Duration::from_millis(100),
        workspace.resources.acquire(ResourceRequest::default()),
    )
    .await
    .is_err();
    reader.finish().await;
    assert!(
        held,
        "repair deadline released admission during physical work"
    );
    assert!(
        locked,
        "repair deadline released the action lock during physical work"
    );
    let _permit = tokio::time::timeout(
        Duration::from_secs(5),
        workspace.resources.acquire(ResourceRequest::default()),
    )
    .await
    .unwrap();
    wait_for_lock(&lock).await;
}
