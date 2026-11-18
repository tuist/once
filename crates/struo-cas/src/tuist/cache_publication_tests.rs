use super::*;

#[tokio::test]
async fn file_and_memory_publications_share_one_upload_and_mapping() {
    let temp = tempfile::TempDir::new().unwrap();
    let (cache, service, server) = fixture(&temp).await;
    let bytes = vec![42; 256 * 1024];
    let digest = cache.local.put_blob(&bytes).await.unwrap();
    let recorder = Arc::new(crate::transfer::Recorder::default());
    let (file, memory) = Box::pin(crate::transfer::observe(recorder.clone(), async {
        tokio::try_join!(
            cache.upload_local_blob(Some(&digest), "put action result"),
            cache.put_blob_remote(&digest, &bytes),
        )
        .unwrap()
    }))
    .await;
    assert_eq!(file.unwrap(), memory);
    assert_eq!(memory, sha256_digest(&bytes).unwrap());
    let observed = service.0.lock().await;
    assert_eq!(observed.heads.len(), 1);
    assert_eq!(observed.writes.len(), 1);
    assert!(observed.blobs.is_empty());
    assert_eq!(observed.actions.len(), 1);
    assert_eq!(recorder.transfers().len(), 1);
    server.abort();
}

#[tokio::test]
async fn a_failed_mapping_releases_waiters_without_remembering_success() {
    let temp = tempfile::TempDir::new().unwrap();
    let (cache, service, server) = fixture(&temp).await;
    let bytes = vec![42; 256 * 1024];
    let digest = cache.local.put_blob(&bytes).await.unwrap();
    service.0.lock().await.failures.insert(
        "action",
        VecDeque::from([Status::permission_denied("no mapping")]),
    );
    let (first, second) = tokio::join!(
        cache.upload_local_blob(Some(&digest), "put action result"),
        cache.upload_local_blob(Some(&digest), "put action result"),
    );
    assert!(first.is_err());
    assert_eq!(second.unwrap().unwrap(), sha256_digest(&bytes).unwrap());
    assert_eq!(
        cache.put_blob_remote(&digest, &bytes).await.unwrap(),
        sha256_digest(&bytes).unwrap()
    );
    let observed = service.0.lock().await;
    assert_eq!(observed.heads.len(), 2);
    assert_eq!(observed.writes.len(), 2);
    assert_eq!(observed.actions.len(), 2);
    server.abort();
}

#[tokio::test]
async fn cancelling_a_publisher_releases_its_digest_for_a_fresh_attempt() {
    let temp = tempfile::TempDir::new().unwrap();
    let (cache, service, server) = fixture(&temp).await;
    let bytes = vec![42; 256 * 1024];
    let digest = cache.local.put_blob(&bytes).await.unwrap();
    service.0.lock().await.action_delay = Some(Duration::from_secs(30));
    let producer = cache.clone();
    let first = tokio::spawn(async move {
        producer
            .upload_local_blob(Some(&digest), "put action result")
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !service.0.lock().await.actions.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    service.0.lock().await.action_delay = None;
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        cache.upload_local_blob(Some(&digest), "put action result"),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.unwrap(), sha256_digest(&bytes).unwrap());
    server.abort();
}
