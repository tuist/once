use super::*;
use bytestream::byte_stream_server::{ByteStream, ByteStreamServer};
use futures::StreamExt;
use std::pin::Pin;
use tonic::Response;

#[derive(Clone)]
struct BlobServer {
    chunks: Vec<std::result::Result<bytestream::ReadResponse, Status>>,
    failures: Vec<Status>,
    reads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    stalls: bool,
}

#[tonic::async_trait]
impl ByteStream for BlobServer {
    type ReadStream = Pin<
        Box<
            dyn futures::Stream<Item = std::result::Result<bytestream::ReadResponse, Status>>
                + Send,
        >,
    >;

    async fn read(
        &self,
        request: Request<bytestream::ReadRequest>,
    ) -> std::result::Result<Response<Self::ReadStream>, Status> {
        assert_eq!(
            request.metadata().get("authorization").unwrap(),
            "Bearer test-token"
        );
        let read = self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(failure) = self.failures.get(read) {
            return Ok(Response::new(Box::pin(stream::iter(vec![Err(
                failure.clone()
            )]))));
        }
        let chunks = stream::iter(self.chunks.clone());
        if self.stalls {
            return Ok(Response::new(Box::pin(chunks.chain(stream::pending()))));
        }
        Ok(Response::new(Box::pin(chunks)))
    }

    async fn write(
        &self,
        _: Request<tonic::Streaming<bytestream::WriteRequest>>,
    ) -> std::result::Result<Response<bytestream::WriteResponse>, Status> {
        Err(Status::unimplemented("read-only test server"))
    }

    async fn query_write_status(
        &self,
        _: Request<bytestream::QueryWriteStatusRequest>,
    ) -> std::result::Result<Response<bytestream::QueryWriteStatusResponse>, Status> {
        Err(Status::unimplemented("read-only test server"))
    }
}

async fn cache_for_stream(
    temp: &tempfile::TempDir,
    digest: Digest,
    chunks: Vec<std::result::Result<bytestream::ReadResponse, Status>>,
) -> (TuistCache, tokio::task::JoinHandle<()>) {
    let (cache, server, _) =
        cache_for_failing_stream(temp, digest, chunks, Vec::new(), false).await;
    (cache, server)
}

async fn cache_for_failing_stream(
    temp: &tempfile::TempDir,
    digest: Digest,
    chunks: Vec<std::result::Result<bytestream::ReadResponse, Status>>,
    failures: Vec<Status>,
    stalls: bool,
) -> (
    TuistCache,
    tokio::task::JoinHandle<()>,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let service = BlobServer {
        chunks,
        failures,
        reads: reads.clone(),
        stalls,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let incoming = stream::unfold(listener, |listener| async {
        Some((listener.accept().await.map(|(socket, _)| socket), listener))
    });
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(ByteStreamServer::new(service))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    let cache = TuistCache::new(
        Cas::open(temp.path().join("cas")),
        temp.path().join("auth"),
        TuistCacheConfig {
            url: "https://tuist.dev".into(),
            account: Some("tuist".into()),
            project: Some("once".into()),
            oauth_client_id: None,
            provider_name: "tuist".into(),
        },
    )
    .unwrap();
    let channel = Endpoint::from_shared(format!("http://{address}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    cache
        .grpc_channel_cache
        .set(Ok(GrpcChannels {
            requests: channel.clone(),
            uploads: channel,
        }))
        .unwrap();
    cache.auth_token_cache.set(Ok("test-token".into())).unwrap();
    cache
        .known_remote_blobs
        .lock()
        .await
        .insert(digest, sha256_digest(b"test mapping").unwrap());
    (cache, server, reads)
}

#[tokio::test]
async fn remote_stream_closes_before_local_mirror_waits_for_end() {
    for bytes in [b"small blob".to_vec(), vec![42; 256 * 1024]] {
        let temp = tempfile::TempDir::new().unwrap();
        let digest = Digest::of_bytes(&bytes);
        let chunks = bytes
            .chunks(32 * 1024)
            .map(|chunk| {
                Ok(bytestream::ReadResponse {
                    data: chunk.to_vec(),
                })
            })
            .collect();
        let (cache, server) = cache_for_stream(&temp, digest, chunks).await;
        assert!(!cache.local.has_blob(&digest).await.unwrap());
        let result =
            tokio::time::timeout(Duration::from_secs(2), cache.ensure_blob_local(&digest)).await;
        server.abort();
        result
            .expect("remote restoration must finish after end of stream")
            .unwrap();
        assert_eq!(cache.local.get_blob(&digest).await.unwrap(), bytes);
    }
}

#[tokio::test]
async fn remote_stream_errors_and_digest_mismatches_do_not_restore_requested_blob() {
    for chunks in [
        vec![
            Ok(bytestream::ReadResponse {
                data: vec![42; 100_000],
            }),
            Err(Status::unavailable("interrupted")),
        ],
        vec![Ok(bytestream::ReadResponse {
            data: b"wrong blob".to_vec(),
        })],
        Vec::new(),
    ] {
        let temp = tempfile::TempDir::new().unwrap();
        let digest = Digest::of_bytes(b"expected blob");
        let (cache, server) = cache_for_stream(&temp, digest, chunks).await;
        let result =
            tokio::time::timeout(Duration::from_secs(2), cache.ensure_blob_local(&digest)).await;
        server.abort();
        assert!(result.expect("failed restoration must terminate").is_err());
        assert!(!cache.local.has_blob(&digest).await.unwrap());
        if cache.local.scratch_dir().exists() {
            let mut scratch = tokio::fs::read_dir(cache.local.scratch_dir())
                .await
                .unwrap();
            assert!(scratch.next_entry().await.unwrap().is_none());
        }
    }
}

#[tokio::test]
async fn transient_stream_failures_are_retried_on_a_fresh_read() {
    let bytes = vec![7; 100_000];
    let temp = tempfile::TempDir::new().unwrap();
    let digest = Digest::of_bytes(&bytes);
    let chunks = vec![Ok(bytestream::ReadResponse {
        data: bytes.clone(),
    })];
    let (cache, server, reads) = cache_for_failing_stream(
        &temp,
        digest,
        chunks,
        vec![
            Status::internal("h2 protocol error: http2 error"),
            Status::cancelled("operation was canceled"),
        ],
        false,
    )
    .await;
    let result =
        tokio::time::timeout(Duration::from_secs(5), cache.ensure_blob_local(&digest)).await;
    server.abort();
    result.expect("retried restoration must finish").unwrap();
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 3);
    assert_eq!(cache.local.get_blob(&digest).await.unwrap(), bytes);
}

#[tokio::test]
async fn non_transient_stream_failures_are_not_retried() {
    let bytes = b"blob".to_vec();
    let temp = tempfile::TempDir::new().unwrap();
    let digest = Digest::of_bytes(&bytes);
    let chunks = vec![Ok(bytestream::ReadResponse { data: bytes })];
    let (cache, server, reads) = cache_for_failing_stream(
        &temp,
        digest,
        chunks,
        vec![Status::permission_denied("no access")],
        false,
    )
    .await;
    let result =
        tokio::time::timeout(Duration::from_secs(5), cache.ensure_blob_local(&digest)).await;
    server.abort();
    assert!(result.expect("failed restoration must terminate").is_err());
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(!cache.local.has_blob(&digest).await.unwrap());
}

#[tokio::test]
async fn stalled_streams_fail_at_the_stall_timeout_without_retrying() {
    let bytes = vec![9; 300_000];
    let temp = tempfile::TempDir::new().unwrap();
    let digest = Digest::of_bytes(&bytes);
    let chunks = vec![Ok(bytestream::ReadResponse {
        data: bytes[..100_000].to_vec(),
    })];
    let (mut cache, server, reads) =
        cache_for_failing_stream(&temp, digest, chunks, Vec::new(), true).await;
    cache.stream_stall_timeout = Duration::from_millis(200);

    let restore = tokio::time::timeout(Duration::from_secs(5), cache.ensure_blob_local(&digest))
        .await
        .expect("a stalled restoration must fail at its stall timeout")
        .unwrap_err();
    assert!(
        restore.to_string().contains("made no progress"),
        "{restore}"
    );
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(!cache.local.has_blob(&digest).await.unwrap());

    let remote_digest = sha256_digest(&bytes).unwrap();
    let read = tokio::time::timeout(
        Duration::from_secs(5),
        cache.read_reapi_blob_stream(&remote_digest, "get blob"),
    )
    .await
    .expect("a stalled read must fail at its stall timeout")
    .unwrap_err();
    assert!(read.to_string().contains("made no progress"), "{read}");
    server.abort();
}

#[tokio::test]
async fn a_streamed_download_is_reported_once_and_a_local_blob_not_at_all() {
    let temp = tempfile::TempDir::new().unwrap();
    let bytes = b"downloaded output".to_vec();
    let digest = Digest::of_bytes(&bytes);
    let chunks = vec![Ok(bytestream::ReadResponse {
        data: bytes.clone(),
    })];
    let (cache, server) = cache_for_stream(&temp, digest, chunks).await;
    let recorder = Arc::new(crate::transfer::Recorder::default());
    crate::transfer::observe(recorder.clone(), async {
        cache.ensure_blob_local(&digest).await.unwrap();
        cache.ensure_blob_local(&digest).await.unwrap();
    })
    .await;
    server.abort();

    let transfers = recorder.transfers();
    assert_eq!(transfers.len(), 1, "{transfers:?}");
    assert_eq!(transfers[0].direction, TransferDirection::Download);
    assert_eq!(transfers[0].digest, digest);
    // The size is the one the remote digest declares; the fixture maps every
    // blob to the same placeholder remote digest.
    assert_eq!(
        transfers[0].size_bytes,
        sha256_digest(b"test mapping").unwrap().size_bytes as u64
    );
}
