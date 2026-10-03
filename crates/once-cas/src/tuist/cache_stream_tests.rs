use super::*;
use bytestream::byte_stream_server::{ByteStream, ByteStreamServer};
use std::pin::Pin;
use tonic::Response;

#[derive(Clone)]
struct BlobServer {
    chunks: Vec<std::result::Result<bytestream::ReadResponse, Status>>,
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
        Ok(Response::new(Box::pin(stream::iter(self.chunks.clone()))))
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let incoming = stream::unfold(listener, |listener| async {
        Some((listener.accept().await.map(|(socket, _)| socket), listener))
    });
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(ByteStreamServer::new(BlobServer { chunks }))
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
    cache
        .grpc_channel_cache
        .set(Ok(Endpoint::from_shared(format!("http://{address}"))
            .unwrap()
            .connect()
            .await
            .unwrap()))
        .unwrap();
    cache.auth_token_cache.set(Ok("test-token".into())).unwrap();
    cache
        .known_remote_blobs
        .lock()
        .await
        .insert(digest, sha256_digest(b"test mapping").unwrap());
    (cache, server)
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
