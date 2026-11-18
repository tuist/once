use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};

use prost::bytes::Bytes;
use tonic::codegen::http;
use tonic::codegen::Service;

use super::*;
use crate::tuist::cache::fault_server::connected_cache;

#[test]
fn repeated_messages_are_bounded_before_protobuf_allocation() {
    let wire = [0x0a_u8, 0].repeat(262_144);
    assert_eq!(
        decode(&mut wire.as_slice(), 1).unwrap_err().code(),
        Code::InvalidArgument
    );
    let response = |details| reapi::batch_read_blobs_response::Response {
        status: Some(bazel_remote_apis::google::rpc::Status {
            details: vec![bazel_remote_apis::google::protobuf::Any::default(); details],
            ..Default::default()
        }),
        ..Default::default()
    };
    let valid = reapi::BatchReadBlobsResponse {
        responses: vec![response(MAX_STATUS_DETAILS)],
    };
    let bytes = valid.encode_to_vec();
    assert_eq!(decode(&mut bytes.as_slice(), 1).unwrap(), valid);
    let invalid = reapi::BatchReadBlobsResponse {
        responses: vec![response(65), response(64)],
    };
    assert_eq!(
        decode(&mut invalid.encode_to_vec().as_slice(), 2)
            .unwrap_err()
            .code(),
        Code::InvalidArgument
    );
}

#[test]
fn bounded_decoding_preserves_compressed_data_digests_and_retry_details() {
    let status =
        crate::tuist::retry_hint::hinted(Code::ResourceExhausted, Duration::from_millis(50));
    let response = reapi::BatchReadBlobsResponse {
        responses: vec![reapi::batch_read_blobs_response::Response {
            digest: Some(sha256_digest(b"verified bytes").unwrap()),
            data: zstd::encode_all(&b"verified bytes"[..], 1).unwrap(),
            compressor: reapi::compressor::Value::Zstd as i32,
            status: Some(bazel_remote_apis::google::rpc::Status::decode(status.details()).unwrap()),
        }],
    };
    assert_eq!(
        decode(&mut response.encode_to_vec().as_slice(), 1).unwrap(),
        response
    );
    for bytes in [
        &[0x0a, 0x80][..],
        &[0x0a, 0x03, 0x1a, 0x02][..],
        &[0x08, 0x00][..],
    ] {
        assert!(decode(&mut &*bytes, 1).is_err());
    }
}

#[derive(Clone)]
struct StalledResponse {
    bytes: Bytes,
    sent: Arc<std::sync::atomic::AtomicBool>,
}

impl tonic::server::NamedService for StalledResponse {
    const NAME: &'static str = SERVICE;
}

impl Service<http::Request<tonic::body::Body>> for StalledResponse {
    type Response = http::Response<tonic::body::Body>;
    type Error = Infallible;
    type Future = std::future::Ready<std::result::Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: http::Request<tonic::body::Body>) -> Self::Future {
        std::future::ready(Ok(http::Response::builder()
            .header("content-type", "application/grpc")
            .body(tonic::body::Body::new(StalledBody {
                bytes: Some(self.bytes.clone()),
                sent: Arc::clone(&self.sent),
            }))
            .unwrap()))
    }
}

struct StalledBody {
    bytes: Option<Bytes>,
    sent: Arc<std::sync::atomic::AtomicBool>,
}

impl http_body::Body for StalledBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<http_body::Frame<Bytes>, Infallible>>> {
        if let Some(bytes) = self.bytes.take() {
            self.sent.store(true, Ordering::Release);
            Poll::Ready(Some(Ok(http_body::Frame::data(bytes))))
        } else {
            Poll::Pending
        }
    }
}

#[tokio::test]
async fn stalled_message_bodies_and_trailers_release_resources_without_cancelling_the_reader() {
    for frame in [vec![0, 0, 0, 0, 5, 0x0a], vec![0, 0, 0, 0, 2, 0x0a, 0]] {
        let temp = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let incoming = stream::unfold(listener, |listener| async {
            Some((listener.accept().await.map(|(socket, _)| socket), listener))
        });
        let sent = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let service = StalledResponse {
            bytes: frame.into(),
            sent: Arc::clone(&sent),
        };
        let server = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(service)
                .serve_with_incoming(incoming)
                .await
                .unwrap();
        });
        let mut cache = connected_cache(&temp, "reader", &format!("http://{address}")).await;
        cache.stream_stall_timeout = Duration::from_millis(100);
        cache.capabilities_cache = Arc::default();
        cache.capabilities_cache.set(RemoteCapabilities {
            batch_read_limit: Some(0),
            ..Default::default()
        });
        let digest = Digest::of_bytes(b"blob");
        cache
            .known_remote_blobs
            .lock()
            .await
            .insert(digest, sha256_digest(b"blob").unwrap());
        let owner = Arc::new(());
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            cache.ensure_blob_local_guarded(&digest, Arc::clone(&owner)),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err().kind(), ErrorKind::Unavailable);
        assert!(sent.load(Ordering::Acquire));
        tokio::time::timeout(Duration::from_secs(3), async {
            while Arc::strong_count(&owner) != 1
                || cache.budget.available()
                    != (cache.budget.memory_bytes(), cache.budget.staging_bytes())
            {
                tokio::task::yield_now().await;
            }
            let _flight = cache.download_flights.acquire(digest).await;
        })
        .await
        .unwrap();
        server.abort();
    }
}
