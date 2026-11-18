use super::fault_server::{connected_cache, FaultServer};
use crate::CacheProvider;

#[tokio::test]
async fn guarded_staging_refetches_local_corruption_once() {
    let temp = tempfile::tempdir().unwrap();
    let server = FaultServer::default();
    let url = server.serve().await;
    let (digest, bytes) = super::fault_tests::publish_large(&temp, &url).await;
    let reader = connected_cache(&temp, "staged-reader", &url).await;
    reader.ensure_blob_local(&digest).await.unwrap();
    tokio::fs::write(reader.local.blob_path(&digest), b"tampered")
        .await
        .unwrap();
    let before = server.calls("read").await;
    let provider = CacheProvider::Tuist(reader);
    let staged = provider
        .stage_blob_guarded(&digest, temp.path(), std::sync::Arc::new(()))
        .await
        .unwrap();
    let mut restored = Vec::new();
    std::io::Read::read_to_end(&mut staged.into_reader().unwrap(), &mut restored).unwrap();
    assert_eq!(restored, bytes);
    assert_eq!(server.calls("read").await, before + 1);
}
