//! Fault injection across the remote read paths: only confirmed absence is a
//! miss, and nothing unverified reaches the local store.

use super::fault_server::{connected_cache, FaultServer};
use super::*;
use tokio::sync::Semaphore;

struct Published {
    action: Digest,
    output: Digest,
    output_bytes: Vec<u8>,
    result: ActionResult,
}

/// Publishes an action result with one output from a separate local store,
/// so the reader can only learn about it through the remote tier.
async fn publish(temp: &tempfile::TempDir, url: &str) -> Published {
    let writer = connected_cache(temp, "writer", url).await;
    let output_bytes = b"compiled output".to_vec();
    let output = writer.local.put_blob(&output_bytes).await.unwrap();
    let stdout = writer.local.put_blob(b"build log").await.unwrap();
    let result = ActionResult {
        exit_code: 0,
        stdout: Some(stdout),
        stderr: None,
        outputs: BTreeMap::from([("out/lib.a".to_string(), output)]),
    };
    let action = Digest::of_bytes(b"compile lib");
    writer
        .put_action_result_remote(&action, &result)
        .await
        .unwrap();
    Published {
        action,
        output,
        output_bytes,
        result,
    }
}

async fn setup() -> (
    tempfile::TempDir,
    FaultServer,
    String,
    Published,
    TuistCache,
) {
    let temp = tempfile::TempDir::new().unwrap();
    let server = FaultServer::default();
    let url = server.serve().await;
    let published = publish(&temp, &url).await;
    let reader = connected_cache(&temp, "reader", &url).await;
    (temp, server, url, published, reader)
}

#[tokio::test]
async fn only_a_not_found_action_cache_answer_is_a_miss() {
    let (_temp, _server, _url, published, reader) = setup().await;
    assert_eq!(
        reader
            .get_action_result(&Digest::of_bytes(b"never ran"))
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        reader.get_action_result(&published.action).await.unwrap(),
        Some(published.result)
    );
}

#[tokio::test]
async fn action_cache_outages_and_refusals_are_errors_not_misses() {
    for (status, kind, calls) in [
        (Status::unavailable("down"), ErrorKind::Unavailable, 3),
        (
            Status::internal("h2 protocol error"),
            ErrorKind::Unavailable,
            3,
        ),
        (Status::unauthenticated("expired"), ErrorKind::Refused, 1),
        (
            Status::permission_denied("no access"),
            ErrorKind::Refused,
            1,
        ),
        (
            Status::invalid_argument("bad digest"),
            ErrorKind::Protocol,
            1,
        ),
    ] {
        let (_temp, server, _url, published, reader) = setup().await;
        server.fail("get_action", vec![status.clone(); 5]).await;
        let error = reader
            .get_action_result(&published.action)
            .await
            .expect_err("a failure must not be reported as a hit or a miss");
        assert_eq!(error.kind(), kind, "{status:?}: {error}");
        assert!(!error.is_not_found());
        assert_eq!(server.calls("get_action").await, calls, "{status:?}");
        assert!(reader
            .local
            .get_action_result(&published.action)
            .await
            .unwrap()
            .is_none());
    }
}

#[tokio::test]
async fn a_transient_action_cache_failure_is_retried_to_a_hit() {
    let (_temp, server, _url, published, reader) = setup().await;
    server
        .fail("get_action", vec![Status::unavailable("connection reset")])
        .await;
    assert_eq!(
        reader.get_action_result(&published.action).await.unwrap(),
        Some(published.result)
    );
    assert_eq!(server.calls("get_action").await, 2);
}

#[tokio::test]
async fn an_entry_whose_blobs_are_confirmed_evicted_is_a_miss() {
    let (_temp, server, _url, published, reader) = setup().await;
    server.evict_all_blobs().await;
    assert_eq!(
        reader.get_action_result(&published.action).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn availability_belongs_to_the_lookup_not_the_persisted_action_record() {
    let (_temp, server, _url, published, reader) = setup().await;
    let provider = crate::CacheProvider::Tuist(reader);
    let remote = provider
        .lookup_action_result(&published.action)
        .await
        .unwrap()
        .unwrap();
    assert!(remote.blobs_available);
    assert_eq!(remote.result, published.result);
    server.evict_all_blobs().await;
    let local = provider
        .lookup_action_result(&published.action)
        .await
        .unwrap()
        .unwrap();
    assert!(!local.blobs_available);
    assert_eq!(local.result, published.result);
    assert!(!provider.has_blob(&published.output).await.unwrap());
}

#[tokio::test]
async fn an_entry_whose_blob_check_cannot_complete_is_an_error() {
    let (_temp, server, _url, published, reader) = setup().await;
    server
        .fail("find_missing", vec![Status::unavailable("down"); 5])
        .await;
    let error = reader
        .get_action_result(&published.action)
        .await
        .unwrap_err();
    assert!(error.is_unavailable(), "{error}");
}

#[tokio::test]
async fn a_malformed_remote_entry_is_corrupt_not_a_miss() {
    let (_temp, server, _url, published, reader) = setup().await;
    {
        let mut state = server.0.lock().await;
        for result in state.actions.values_mut() {
            if let Some(metadata) = result.execution_metadata.as_mut() {
                for value in &mut metadata.auxiliary_metadata {
                    value.value = b"{not json".to_vec();
                }
            }
        }
    }
    let error = reader
        .get_action_result(&published.action)
        .await
        .unwrap_err();
    assert!(error.is_corrupt(), "{error}");
}

#[tokio::test]
async fn has_blob_is_false_only_for_confirmed_absence() {
    let (temp, server, url, published, reader) = setup().await;
    assert!(reader.has_blob(&published.output).await.unwrap());
    assert!(!reader
        .has_blob(&Digest::of_bytes(b"never stored"))
        .await
        .unwrap());

    let fresh = connected_cache(&temp, "fresh", &url).await;
    server
        .fail("get_action", vec![Status::unavailable("down"); 5])
        .await;
    let error = fresh.has_blob(&published.output).await.unwrap_err();
    assert!(error.is_unavailable(), "{error}");

    let fresh = connected_cache(&temp, "fresh-refused", &url).await;
    server
        .fail("get_action", vec![Status::unauthenticated("expired")])
        .await;
    assert_eq!(
        fresh.has_blob(&published.output).await.unwrap_err().kind(),
        ErrorKind::Refused
    );

    server.evict_all_blobs().await;
    let fresh = connected_cache(&temp, "fresh-evicted", &url).await;
    assert!(!fresh.has_blob(&published.output).await.unwrap());
}

#[tokio::test]
async fn evicted_blobs_are_blob_not_found_so_callers_can_repair() {
    let (temp, server, _url, published, reader) = setup().await;
    let result = reader
        .get_action_result(&published.action)
        .await
        .unwrap()
        .unwrap();
    // Evicted after the action cache hit, before the consumer restores.
    server.evict_all_blobs().await;
    let destination = temp.path().join("restored");
    let error = reader
        .copy_blob_to_file(&result.outputs["out/lib.a"], &destination)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::BlobNotFound(digest) if digest == published.output));
    assert!(!destination.exists());
    let error = reader.get_blob(&published.output).await.unwrap_err();
    assert!(matches!(error, Error::BlobNotFound(_)), "{error}");
}

#[tokio::test]
async fn unreachable_blob_reads_are_not_reported_as_missing() {
    let (temp, server, _url, published, reader) = setup().await;
    server
        .fail("read", vec![Status::unavailable("down"); 5])
        .await;
    let error = reader
        .copy_blob_to_file(&published.output, &temp.path().join("restored"))
        .await
        .unwrap_err();
    assert!(error.is_unavailable(), "{error}");
    assert_eq!(server.calls("read").await, 3);

    server
        .fail("batch_read", vec![Status::unavailable("down"); 5])
        .await;
    let error = reader.get_blob(&published.output).await.unwrap_err();
    assert!(error.is_unavailable(), "{error}");
    assert!(!reader.local.has_blob(&published.output).await.unwrap());
}

#[tokio::test]
async fn tampered_and_oversized_downloads_are_never_committed() {
    for (replacement, suffix) in [
        (Some(b"tampered output!".to_vec()), Vec::new()),
        (None, b"trailing bytes".to_vec()),
    ] {
        let (temp, server, _url, published, reader) = setup().await;
        {
            let mut state = server.0.lock().await;
            state.read_replacement = replacement.clone();
            state.read_suffix = suffix;
        }
        let destination = temp.path().join("restored");
        let error = reader
            .copy_blob_to_file(&published.output, &destination)
            .await
            .unwrap_err();
        assert!(error.is_corrupt(), "{error}");
        assert_eq!(server.calls("read").await, 1, "corruption is not retried");
        assert!(!reader.local.has_blob(&published.output).await.unwrap());
        assert!(!destination.exists());
        let scratch = reader.local.scratch_dir();
        if scratch.exists() {
            assert_eq!(std::fs::read_dir(scratch).unwrap().count(), 0);
        }
        if replacement.is_some() {
            let error = reader.get_blob(&published.output).await.unwrap_err();
            assert!(error.is_corrupt(), "{error}");
        }
    }
}

#[tokio::test]
async fn a_corrupt_local_copy_is_replaced_from_the_remote_tier() {
    let (temp, _server, _url, published, reader) = setup().await;
    reader.ensure_blob_local(&published.output).await.unwrap();
    let stored = reader.local.blob_path(&published.output);
    std::fs::write(&stored, b"bit rot").unwrap();

    let destination = temp.path().join("restored");
    reader
        .copy_blob_to_file(&published.output, &destination)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), published.output_bytes);
    assert_eq!(
        reader.local.get_blob(&published.output).await.unwrap(),
        published.output_bytes
    );

    std::fs::write(&stored, b"bit rot again").unwrap();
    assert_eq!(
        reader.get_blob(&published.output).await.unwrap(),
        published.output_bytes
    );
}

#[tokio::test]
async fn an_oversized_streamed_read_is_cut_off_at_its_declared_size() {
    let (_temp, server, _url, published, reader) = setup().await;
    server.0.lock().await.read_suffix = vec![0; 4096];
    let remote = reader.remote_blob_digest(&published.output).await.unwrap();
    let error = retry::grpc("get blob", || reader.stage_remote_blob(&remote, "get blob"))
        .await
        .unwrap_err();
    assert!(error.is_corrupt(), "{error}");
}

/// A blob above the in-memory limit, published by a writer and read back by
/// a fresh reader.
pub(super) async fn publish_large(temp: &tempfile::TempDir, url: &str) -> (Digest, Vec<u8>) {
    let writer = connected_cache(temp, "large-writer", url).await;
    let bytes: Vec<u8> = (0..usize::try_from(download::IN_MEMORY_BLOB_LIMIT).unwrap() * 2)
        .map(|index| u8::try_from(index % 251).unwrap())
        .collect();
    let digest = writer.local.put_blob(&bytes).await.unwrap();
    writer.put_blob_remote(&digest, &bytes).await.unwrap();
    (digest, bytes)
}

#[tokio::test]
async fn large_remote_blobs_stream_to_disk_instead_of_memory() {
    let (temp, server, url, _published, _reader) = setup().await;
    let (digest, bytes) = publish_large(&temp, &url).await;
    let reader = connected_cache(&temp, "large-reader", &url).await;

    assert_eq!(reader.get_blob(&digest).await.unwrap(), bytes);

    assert_eq!(
        server.calls("batch_read").await,
        0,
        "no in-memory batch read"
    );
    assert_eq!(server.calls("read").await, 1);
    assert!(reader.local.has_blob(&digest).await.unwrap());
}

#[tokio::test]
async fn a_declared_size_never_drives_allocation_or_acceptance() {
    let (temp, server, url, _published, _reader) = setup().await;
    let (digest, _) = publish_large(&temp, &url).await;
    let remote = {
        let writer = connected_cache(&temp, "probe", &url).await;
        writer.remote_blob_digest(&digest).await.unwrap()
    };
    // A non-native action result whose output claims to be a terabyte.
    let huge = reapi::Digest {
        hash: remote.hash.clone(),
        size_bytes: 1 << 40,
    };
    let action = Digest::of_bytes(b"foreign action");
    server.0.lock().await.actions.insert(
        action_mapping_digest(&action).unwrap().hash,
        reapi::ActionResult {
            output_files: vec![reapi::OutputFile {
                path: "out/huge".into(),
                digest: Some(huge),
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    // The default staging budget refuses the claim before reading a byte.
    let reader = connected_cache(&temp, "foreign-reader", &url).await;
    let error = reader.get_action_result(&action).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Exhausted, "{error}");
    assert_eq!(server.calls("read").await, 0);

    // A budget that admits it still rejects the short body by its size.
    let mut generous = connected_cache(&temp, "generous-reader", &url).await;
    generous.set_transfer_budget(crate::TransferBudget::new(64 << 20, 1 << 42));
    let error = generous.get_action_result(&action).await.unwrap_err();
    assert!(error.is_corrupt(), "{error}");
    assert_eq!(server.calls("batch_read").await, 0);
    for cache in [&reader, &generous] {
        let scratch = cache.local.scratch_dir();
        if scratch.exists() {
            assert_eq!(std::fs::read_dir(scratch).unwrap().count(), 0);
        }
    }
}

#[tokio::test]
async fn large_outputs_of_foreign_action_results_are_mirrored_by_streaming() {
    let (temp, server, url, _published, _reader) = setup().await;
    let (digest, bytes) = publish_large(&temp, &url).await;
    let remote = connected_cache(&temp, "probe", &url)
        .await
        .remote_blob_digest(&digest)
        .await
        .unwrap();
    let action = Digest::of_bytes(b"foreign action");
    server.0.lock().await.actions.insert(
        action_mapping_digest(&action).unwrap().hash,
        reapi::ActionResult {
            output_files: vec![reapi::OutputFile {
                path: "out/large".into(),
                digest: Some(remote),
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    let reader = connected_cache(&temp, "foreign-reader", &url).await;

    let result = reader.get_action_result(&action).await.unwrap().unwrap();

    assert_eq!(result.outputs["out/large"], digest);
    assert_eq!(reader.local.get_blob(&digest).await.unwrap(), bytes);
    assert_eq!(server.calls("batch_read").await, 0);
}

#[tokio::test]
async fn a_failed_connection_recovers_without_restarting_the_process() {
    let temp = tempfile::TempDir::new().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);

    let mut cache = connected_cache(&temp, "seed", &FaultServer::default().serve().await).await;
    cache.grpc_channel_cache = Arc::default();
    cache.endpoint_override = Some(format!("http://{address}"));

    let error = cache
        .has_blob(&Digest::of_bytes(b"probe"))
        .await
        .unwrap_err();
    assert!(error.is_unavailable(), "{error}");
    let replayed = cache
        .has_blob(&Digest::of_bytes(b"probe"))
        .await
        .unwrap_err();
    assert!(replayed.is_unavailable(), "{replayed}");

    let server = FaultServer::default();
    server.serve_on(tokio::net::TcpListener::bind(address).await.unwrap());
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(!cache.has_blob(&Digest::of_bytes(b"probe")).await.unwrap());
    assert_eq!(server.calls("get_action").await, 1);
}

/// Endpoint discovery that answers its first request with an outage and
/// holds the second until `release` grants a permit.
async fn gated_discovery(
    endpoint: String,
) -> (String, Arc<std::sync::atomic::AtomicUsize>, Arc<Semaphore>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let release = Arc::new(Semaphore::new(0));
    let (counter, gate) = (Arc::clone(&requests), Arc::clone(&release));
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let gate = Arc::clone(&gate);
            let endpoint = endpoint.clone();
            tokio::spawn(async move {
                let mut buffer = [0_u8; 4096];
                let _ = socket.read(&mut buffer).await.unwrap();
                let response = if request == 0 {
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_string()
                } else {
                    gate.acquire().await.unwrap().forget();
                    let body = format!(r#"{{"endpoints":["{endpoint}"]}}"#);
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                };
                socket.write_all(response.as_bytes()).await.unwrap();
                let _ = socket.shutdown().await;
            });
        }
    });
    (format!("http://{address}"), requests, release)
}

#[tokio::test]
async fn cancelling_the_caller_of_a_pending_half_open_probe_still_serves_every_waiter() {
    let temp = tempfile::TempDir::new().unwrap();
    let server = FaultServer::default();
    let data_plane = server.serve().await;
    let (discovery, requests, release) = gated_discovery(data_plane.clone()).await;
    let mut cache = connected_cache(&temp, "half-open", &data_plane).await;
    cache.grpc_channel_cache = Arc::default();
    cache.endpoint_override = None;
    cache.config.url = discovery;
    let probe = Digest::of_bytes(b"probe");

    let outage = cache.has_blob(&probe).await.unwrap_err();
    assert!(outage.is_unavailable(), "{outage}");
    tokio::time::sleep(Duration::from_millis(1100)).await;

    let initiator = {
        let cache = cache.clone();
        tokio::spawn(async move { cache.has_blob(&probe).await })
    };
    while requests.load(std::sync::atomic::Ordering::SeqCst) < 2 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let waiters: Vec<_> = (0..4)
        .map(|_| {
            let cache = cache.clone();
            tokio::spawn(async move { cache.has_blob(&probe).await })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(20)).await;
    initiator.abort();
    assert!(initiator.await.unwrap_err().is_cancelled());

    release.add_permits(1);
    for waiter in waiters {
        assert!(!waiter.await.unwrap().unwrap(), "probe is confirmed absent");
    }
    assert!(!cache.has_blob(&probe).await.unwrap());
    assert_eq!(
        requests.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the outage and one half-open probe, nothing more"
    );
}
