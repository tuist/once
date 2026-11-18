use super::*;
use bytestream::byte_stream_server::{ByteStream, ByteStreamServer};
use futures::StreamExt;
use std::pin::Pin;
use tonic::Response;

#[path = "cache_download_benchmarks.rs"]
mod benchmarks;
#[path = "cache_compressed_tests.rs"]
mod compressed;
#[path = "cache_download_tests.rs"]
mod downloads;
#[path = "cache_paced_benchmarks.rs"]
mod paced;

#[derive(Default)]
struct StreamOptions {
    failures: Vec<Status>,
    stalls: bool,
    compressed: Option<Vec<std::result::Result<bytestream::ReadResponse, Status>>>,
    chunk_delay: Duration,
}

#[derive(Clone)]
struct BlobServer {
    chunks: Vec<std::result::Result<bytestream::ReadResponse, Status>>,
    failures: Vec<Status>,
    reads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    stalls: bool,
    compressed: Option<Vec<std::result::Result<bytestream::ReadResponse, Status>>>,
    chunk_delay: Duration,
    wire_bytes: Arc<std::sync::atomic::AtomicU64>,
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
        let compressed = request
            .get_ref()
            .resource_name
            .contains("compressed-blobs/zstd/");
        let chunks = if compressed {
            self.compressed
                .clone()
                .ok_or_else(|| Status::unimplemented("compression unavailable"))?
        } else {
            self.chunks.clone()
        };
        let delay = self.chunk_delay;
        let wire_bytes = Arc::clone(&self.wire_bytes);
        let chunks = stream::iter(chunks).then(move |chunk| {
            let wire_bytes = Arc::clone(&wire_bytes);
            async move {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                if let Ok(response) = &chunk {
                    wire_bytes.fetch_add(
                        u64::try_from(response.data.len()).unwrap(),
                        std::sync::atomic::Ordering::SeqCst,
                    );
                }
                chunk
            }
        });
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
    let (cache, server, reads, _) = cache_for_configured_stream(
        temp,
        digest,
        chunks,
        StreamOptions {
            failures,
            stalls,
            ..StreamOptions::default()
        },
    )
    .await;
    (cache, server, reads)
}

async fn cache_for_configured_stream(
    temp: &tempfile::TempDir,
    digest: Digest,
    chunks: Vec<std::result::Result<bytestream::ReadResponse, Status>>,
    options: StreamOptions,
) -> (
    TuistCache,
    tokio::task::JoinHandle<()>,
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<std::sync::atomic::AtomicU64>,
) {
    let mut remote_hasher = Sha256::new();
    for chunk in chunks.iter().filter_map(|chunk| chunk.as_ref().ok()) {
        remote_hasher.update(&chunk.data);
    }
    let size_bytes = chunks
        .iter()
        .filter_map(|chunk| chunk.as_ref().ok())
        .map(|chunk| i64::try_from(chunk.data.len()).unwrap())
        .sum();
    let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let wire_bytes = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let capabilities = RemoteCapabilities {
        zstd_streams: options.compressed.is_some(),
        ..RemoteCapabilities::default()
    };
    let service = BlobServer {
        chunks,
        failures: options.failures,
        reads: reads.clone(),
        stalls: options.stalls,
        compressed: options.compressed,
        chunk_delay: options.chunk_delay,
        wire_bytes: Arc::clone(&wire_bytes),
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
            project: Some("struo".into()),
            oauth_client_id: None,
            provider_name: "tuist".into(),
        },
    )
    .unwrap();
    let channel = grpc_endpoint(&format!("http://{address}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    cache.grpc_channel_cache.set(GrpcChannels {
        requests: channel.clone(),
        uploads: channel,
    });
    cache.auth_token_cache.set("test-token".into());
    cache.known_remote_blobs.lock().await.insert(
        digest,
        reapi::Digest {
            size_bytes,
            hash: hex::encode(remote_hasher.finalize()),
        },
    );
    cache.capabilities_cache.set(capabilities);
    (cache, server, reads, wire_bytes)
}

#[tokio::test]
#[ignore = "manual transfer performance measurement"]
async fn benchmark_large_stream_downloads() {
    let mut state = 0x1234_5678_u64;
    let bytes: Vec<u8> = (0..32 * 1024 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect();
    let digest = Digest::of_bytes(&bytes);
    let mut samples = Vec::new();
    for _ in 0..7 {
        let temp = tempfile::TempDir::new().unwrap();
        let chunks = bytes
            .chunks(256 * 1024)
            .map(|chunk| {
                Ok(bytestream::ReadResponse {
                    data: chunk.to_vec(),
                })
            })
            .collect();
        let (cache, server) = cache_for_stream(&temp, digest, chunks).await;
        let started = Instant::now();
        cache.ensure_blob_local(&digest).await.unwrap();
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        server.abort();
        assert_eq!(cache.local.get_blob(&digest).await.unwrap(), bytes);
    }
    samples.sort_by(f64::total_cmp);
    println!("METRIC stream_download_ms={}", samples[3]);
    println!("METRIC download_mib_per_s={}", 32_000.0 / samples[3]);
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
        cache.stage_remote_blob(&remote_digest, "get blob"),
    )
    .await
    .expect("a stalled read must fail at its stall timeout")
    .unwrap_err();
    let read = match read {
        retry::Failure::Rpc(error) => status::grpc_error("get blob", &error),
        retry::Failure::Local(error) => error,
    };
    assert!(read.to_string().contains("made no progress"), "{read}");
    server.abort();
}

#[tokio::test]
async fn a_streamed_download_is_reported_struo_and_a_local_blob_not_at_all() {
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
    assert_eq!(transfers[0].size_bytes, u64::try_from(bytes.len()).unwrap());
}
