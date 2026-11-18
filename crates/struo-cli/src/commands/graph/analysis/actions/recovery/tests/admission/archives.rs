use std::fmt::Write as _;

use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

async fn serve(bytes: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        assert!(socket.read(&mut request).await.unwrap() > 0);
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        );
        socket.write_all(header.as_bytes()).await.unwrap();
        let _ = socket.write_all(&bytes).await;
    });
    (format!("http://{address}/archive.zip"), server)
}

fn download(url: &str, sha256: &str) -> AnalysisResult {
    AnalysisResult {
        actions: vec![action(serde_json::json!({
            "operation": {"kind": "download_and_extract", "url": url, "sha256": sha256, "destination": "unpacked"},
            "outputs": ["unpacked"], "cacheable": true,
        }))],
        provider: serde_json::json!({}),
        declared_outputs: Vec::new(),
        observations: AnalysisObservations::default(),
    }
}

#[tokio::test]
async fn graph_archive_download_uses_the_shared_staging_budget() {
    let (url, server) = serve(vec![0; 4096]).await;
    let workspace = Workspace::new();
    let producers = Producers::default();
    producers
        .inner
        .executors
        .set(
            struo_core::ExecutorResources::new(Arc::clone(&workspace.resources))
                .with_remote_io_limits(struo_core::RemoteIoLimits {
                    max_staging_bytes: 128 * 1024,
                    ..Default::default()
                }),
        )
        .unwrap_or_else(|_| panic!("already initialized"));
    let error = tokio::time::timeout(
        Duration::from_secs(10),
        workspace.build_with(download(&url, &"0".repeat(64)), &producers, None),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(error.chain().any(|cause| matches!(cause.downcast_ref::<struo_core::Error>(), Some(struo_core::Error::ResourceRequirementUnsatisfied { resource, .. }) if resource == "staging bytes")), "unexpected error: {error:#}");
    assert!(!workspace.path().join("unpacked").exists());
    server.await.unwrap();
}

#[tokio::test]
async fn graph_zip_metadata_is_bounded_by_the_actions_memory_reservation() {
    let metadata_bytes = 20 * 1024 * 1024;
    let mut archive = vec![0; metadata_bytes];
    archive[..4].copy_from_slice(&0x0201_4b50_u32.to_le_bytes());
    archive.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    archive.extend_from_slice(&0_u16.to_le_bytes());
    archive.extend_from_slice(&0_u16.to_le_bytes());
    archive.extend_from_slice(&1_u16.to_le_bytes());
    archive.extend_from_slice(&1_u16.to_le_bytes());
    archive.extend_from_slice(&u32::try_from(metadata_bytes).unwrap().to_le_bytes());
    archive.extend_from_slice(&0_u32.to_le_bytes());
    archive.extend_from_slice(&0_u16.to_le_bytes());
    let mut sha256 = String::new();
    for byte in Sha256::digest(&archive) {
        write!(sha256, "{byte:02x}").unwrap();
    }
    let (url, server) = serve(archive).await;
    let mut workspace = Workspace::new();
    workspace.resources = Arc::new(ResourcePool::new(struo_core::ResourceLimits::new(
        1,
        64 * 1024 * 1024,
    )));
    let producers = Producers::default();
    let error = tokio::time::timeout(
        Duration::from_secs(20),
        workspace.build_with(download(&url, &sha256), &producers, None),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(error.chain().any(|cause| matches!(cause.downcast_ref::<struo_core::Error>(), Some(struo_core::Error::ExecutorResourceLimit { resource: "archive metadata bytes", limit, .. }) if *limit == 16 * 1024 * 1024)), "metadata did not follow the graph reservation: {error:#}");
    assert!(!workspace.path().join("unpacked").exists());
    server.await.unwrap();
}
