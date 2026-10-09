use super::*;
use bytestream::byte_stream_server::{ByteStream, ByteStreamServer};
use reapi::action_cache_server::{ActionCache, ActionCacheServer};
use reapi::content_addressable_storage_server::{
    ContentAddressableStorage, ContentAddressableStorageServer,
};
use std::collections::VecDeque;
use std::pin::Pin;
use tonic::Response;

type RpcResult<T> = std::result::Result<Response<T>, Status>;
type ReadStream = Pin<
    Box<dyn futures::Stream<Item = std::result::Result<bytestream::ReadResponse, Status>> + Send>,
>;

#[derive(Default)]
struct Observations {
    heads: Vec<reapi::FindMissingBlobsRequest>,
    blobs: Vec<reapi::BatchUpdateBlobsRequest>,
    actions: Vec<reapi::UpdateActionResultRequest>,
    writes: Vec<Vec<bytestream::WriteRequest>>,
    failures: HashMap<&'static str, VecDeque<Status>>,
    batch_statuses: VecDeque<Code>,
    action_delay: Option<Duration>,
    write_message_delay: Option<Duration>,
    write_stops_reading: bool,
}

#[derive(Clone, Default)]
struct UploadServer(Arc<Mutex<Observations>>);

fn authenticated<T>(request: &Request<T>) {
    assert_eq!(
        request.metadata().get("authorization").unwrap(),
        "Bearer test-token"
    );
}

fn fail(state: &mut Observations, method: &str) -> std::result::Result<(), Status> {
    match state.failures.get_mut(method).and_then(VecDeque::pop_front) {
        Some(status) => Err(status),
        None => Ok(()),
    }
}

#[tonic::async_trait]
impl ContentAddressableStorage for UploadServer {
    type GetTreeStream = Pin<
        Box<dyn futures::Stream<Item = std::result::Result<reapi::GetTreeResponse, Status>> + Send>,
    >;
    type GetChunkMappingStream = Pin<
        Box<
            dyn futures::Stream<Item = std::result::Result<reapi::GetChunkMappingResponse, Status>>
                + Send,
        >,
    >;

    async fn find_missing_blobs(
        &self,
        request: Request<reapi::FindMissingBlobsRequest>,
    ) -> RpcResult<reapi::FindMissingBlobsResponse> {
        authenticated(&request);
        let mut state = self.0.lock().await;
        state.heads.push(request.get_ref().clone());
        fail(&mut state, "head")?;
        Ok(Response::new(reapi::FindMissingBlobsResponse {
            missing_blob_digests: request.into_inner().blob_digests,
        }))
    }

    async fn batch_update_blobs(
        &self,
        request: Request<reapi::BatchUpdateBlobsRequest>,
    ) -> RpcResult<reapi::BatchUpdateBlobsResponse> {
        authenticated(&request);
        let mut state = self.0.lock().await;
        state.blobs.push(request.get_ref().clone());
        fail(&mut state, "batch")?;
        let code = state.batch_statuses.pop_front().unwrap_or(Code::Ok);
        Ok(Response::new(reapi::BatchUpdateBlobsResponse {
            responses: request
                .into_inner()
                .requests
                .into_iter()
                .map(|blob| reapi::batch_update_blobs_response::Response {
                    digest: blob.digest,
                    status: Some(bazel_remote_apis::google::rpc::Status {
                        code: code as i32,
                        message: "injected blob status".into(),
                        details: vec![],
                    }),
                })
                .collect(),
        }))
    }

    async fn batch_read_blobs(
        &self,
        _: Request<reapi::BatchReadBlobsRequest>,
    ) -> RpcResult<reapi::BatchReadBlobsResponse> {
        Err(Status::unimplemented("upload-only server"))
    }
    async fn get_tree(&self, _: Request<reapi::GetTreeRequest>) -> RpcResult<Self::GetTreeStream> {
        Err(Status::unimplemented("upload-only server"))
    }
    async fn split_blob(
        &self,
        _: Request<reapi::SplitBlobRequest>,
    ) -> RpcResult<reapi::SplitBlobResponse> {
        Err(Status::unimplemented("upload-only server"))
    }
    async fn splice_blob(
        &self,
        _: Request<reapi::SpliceBlobRequest>,
    ) -> RpcResult<reapi::SpliceBlobResponse> {
        Err(Status::unimplemented("upload-only server"))
    }
    async fn get_chunk_mapping(
        &self,
        _: Request<reapi::GetChunkMappingRequest>,
    ) -> RpcResult<Self::GetChunkMappingStream> {
        Err(Status::unimplemented("upload-only server"))
    }
    async fn register_chunk_mapping(
        &self,
        _: Request<tonic::Streaming<reapi::RegisterChunkMappingRequest>>,
    ) -> RpcResult<reapi::RegisterChunkMappingResponse> {
        Err(Status::unimplemented("upload-only server"))
    }
}

#[tonic::async_trait]
impl ActionCache for UploadServer {
    async fn get_action_result(
        &self,
        _: Request<reapi::GetActionResultRequest>,
    ) -> RpcResult<reapi::ActionResult> {
        Err(Status::unimplemented("upload-only server"))
    }

    async fn update_action_result(
        &self,
        request: Request<reapi::UpdateActionResultRequest>,
    ) -> RpcResult<reapi::ActionResult> {
        authenticated(&request);
        let mut state = self.0.lock().await;
        state.actions.push(request.get_ref().clone());
        fail(&mut state, "action")?;
        if let Some(delay) = state.action_delay {
            drop(state);
            tokio::time::sleep(delay).await;
        }
        Ok(Response::new(request.into_inner().action_result.unwrap()))
    }
}

#[tonic::async_trait]
impl ByteStream for UploadServer {
    type ReadStream = ReadStream;
    async fn read(&self, _: Request<bytestream::ReadRequest>) -> RpcResult<Self::ReadStream> {
        Err(Status::unimplemented("upload-only server"))
    }
    async fn query_write_status(
        &self,
        _: Request<bytestream::QueryWriteStatusRequest>,
    ) -> RpcResult<bytestream::QueryWriteStatusResponse> {
        Err(Status::unimplemented("upload-only server"))
    }

    async fn write(
        &self,
        request: Request<tonic::Streaming<bytestream::WriteRequest>>,
    ) -> RpcResult<bytestream::WriteResponse> {
        authenticated(&request);
        let mut stream = request.into_inner();
        let first = stream.message().await?.expect("first upload chunk");
        let mut chunks = vec![first];
        let message_delay = {
            let mut state = self.0.lock().await;
            if let Err(status) = fail(&mut state, "write") {
                state.writes.push(chunks);
                return Err(status);
            }
            if state.write_stops_reading {
                state.writes.push(chunks);
                drop(state);
                return std::future::pending().await;
            }
            state.write_message_delay
        };
        while let Some(chunk) = stream.message().await? {
            chunks.push(chunk);
            if let Some(delay) = message_delay {
                tokio::time::sleep(delay).await;
            }
        }
        let size = chunks
            .iter()
            .map(|chunk| i64::try_from(chunk.data.len()).unwrap())
            .sum();
        self.0.lock().await.writes.push(chunks);
        Ok(Response::new(bytestream::WriteResponse {
            committed_size: size,
        }))
    }
}

async fn fixture(
    temp: &tempfile::TempDir,
) -> (TuistCache, UploadServer, tokio::task::JoinHandle<()>) {
    fixture_with_timeout(temp, None).await
}

async fn fixture_with_timeout(
    temp: &tempfile::TempDir,
    timeout: Option<Duration>,
) -> (TuistCache, UploadServer, tokio::task::JoinHandle<()>) {
    let service = UploadServer::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let incoming = stream::unfold(listener, |listener| async {
        Some((listener.accept().await.map(|(socket, _)| socket), listener))
    });
    let server_service = service.clone();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(ContentAddressableStorageServer::new(server_service.clone()))
            .add_service(ActionCacheServer::new(server_service.clone()))
            .add_service(ByteStreamServer::new(server_service))
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
    let endpoint = Endpoint::from_shared(format!("http://{address}")).unwrap();
    let mut requests = endpoint.clone();
    if let Some(timeout) = timeout {
        requests = requests.timeout(timeout);
    }
    cache
        .grpc_channel_cache
        .set(Ok(GrpcChannels {
            requests: requests.connect().await.unwrap(),
            uploads: endpoint.connect().await.unwrap(),
        }))
        .unwrap();
    cache.auth_token_cache.set(Ok("test-token".into())).unwrap();
    *cache.capabilities_cache.lock().await = Some(RemoteCapabilities::default());
    (cache, service, server)
}

#[tokio::test]
async fn blob_publication_recovers_from_transient_head_upload_and_mapping_failures() {
    let temp = tempfile::TempDir::new().unwrap();
    let (cache, service, server) = fixture(&temp).await;
    {
        let mut state = service.0.lock().await;
        state.failures.insert(
            "head",
            VecDeque::from([Status::unavailable("connection closed")]),
        );
        state.failures.insert(
            "batch",
            VecDeque::from([Status::internal("h2 protocol error: http2 error")]),
        );
        state.failures.insert(
            "action",
            VecDeque::from([Status::cancelled("operation was canceled")]),
        );
    }
    let bytes = b"artifact to publish";
    let digest = cache.local.put_blob(bytes).await.unwrap();
    cache.put_blob_remote(&digest, bytes).await.unwrap();
    let state = service.0.lock().await;
    assert_eq!(state.heads.len(), 2);
    assert_eq!(state.heads[0], state.heads[1]);
    assert_eq!(state.blobs.len(), 2);
    assert_eq!(state.blobs[0], state.blobs[1]);
    assert_eq!(state.actions.len(), 2);
    assert_eq!(state.actions[0], state.actions[1]);
    assert_eq!(state.blobs[1].requests[0].data, bytes);
    server.abort();
}

#[tokio::test]
async fn batch_upload_retries_individual_transient_status_but_not_permission_denied() {
    for (code, succeeds, calls) in [
        (Code::Unavailable, true, 2),
        (Code::PermissionDenied, false, 1),
    ] {
        let temp = tempfile::TempDir::new().unwrap();
        let (cache, service, server) = fixture(&temp).await;
        service.0.lock().await.batch_statuses.push_back(code);
        let bytes = b"small blob";
        let result = cache
            .upload_reapi_blob(&sha256_digest(bytes).unwrap(), bytes, "put blob")
            .await;
        assert_eq!(result.is_ok(), succeeds);
        if let Err(error) = result {
            assert!(error
                .to_string()
                .contains("REAPI status 7: injected blob status"));
        }
        assert_eq!(service.0.lock().await.blobs.len(), calls);
        server.abort();
    }
}

#[tokio::test]
async fn streamed_upload_reopens_file_and_restarts_memory_body_with_fresh_resource() {
    for (from_file, compressed) in [(false, false), (false, true), (true, false), (true, true)] {
        let temp = tempfile::TempDir::new().unwrap();
        let (cache, service, server) = fixture(&temp).await;
        cache
            .capabilities_cache
            .lock()
            .await
            .as_mut()
            .unwrap()
            .zstd_streams = compressed;
        service.0.lock().await.failures.insert(
            "write",
            VecDeque::from([Status::internal("h2 protocol error: http2 error")]),
        );
        let bytes: Vec<u8> = (0..BATCH_BLOB_LIMIT + 1024)
            .map(|index| u8::try_from(index % 251).unwrap())
            .collect();
        let digest = sha256_digest(&bytes).unwrap();
        if from_file {
            let path = temp.path().join("blob");
            tokio::fs::write(&path, &bytes).await.unwrap();
            cache
                .write_reapi_blob_file_stream(&digest, &path, "put blob")
                .await
                .unwrap();
        } else {
            cache
                .upload_reapi_blob(&digest, &bytes, "put blob")
                .await
                .unwrap();
        }
        let state = service.0.lock().await;
        assert_eq!(state.writes.len(), 2);
        let first = &state.writes[0][0];
        let retried = &state.writes[1];
        assert_eq!(first.write_offset, 0);
        assert_eq!(retried[0].write_offset, 0);
        assert_ne!(first.resource_name, retried[0].resource_name);
        assert_eq!(first.data, retried[0].data);
        assert!(retried.last().unwrap().finish_write);
        let mut offset = 0;
        let mut uploaded = Vec::new();
        for chunk in retried {
            assert_eq!(chunk.write_offset, offset);
            offset += i64::try_from(chunk.data.len()).unwrap();
            uploaded.extend_from_slice(&chunk.data);
        }
        let compressor = if compressed {
            reapi::compressor::Value::Zstd
        } else {
            reapi::compressor::Value::Identity
        };
        assert_eq!(
            decode_reapi_blob(uploaded, compressor as i32, &digest, "put blob").unwrap(),
            bytes
        );
        server.abort();
    }
}

#[tokio::test]
async fn request_timeouts_are_not_retried() {
    let temp = tempfile::TempDir::new().unwrap();
    let (cache, service, server) =
        fixture_with_timeout(&temp, Some(Duration::from_millis(100))).await;
    service.0.lock().await.action_delay = Some(Duration::from_secs(5));
    let digest = sha256_digest(b"action").unwrap();
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        cache.update_reapi_action_result(&digest, reapi::ActionResult::default(), "put action"),
    )
    .await
    .expect("a timed out request must not be retried until the test deadline")
    .unwrap_err();
    assert!(error.to_string().contains("Timeout expired"), "{error}");
    assert_eq!(service.0.lock().await.actions.len(), 1);
    server.abort();
}

fn streamed_blob(chunks: usize) -> (Vec<u8>, reapi::Digest) {
    let bytes: Vec<u8> = (0..BYTE_STREAM_CHUNK_SIZE * chunks)
        .map(|index| u8::try_from(index % 251).unwrap())
        .collect();
    let digest = sha256_digest(&bytes).unwrap();
    (bytes, digest)
}

#[tokio::test]
async fn streamed_upload_outlasts_the_request_timeout_while_it_progresses() {
    for from_file in [false, true] {
        let temp = tempfile::TempDir::new().unwrap();
        let (mut cache, service, server) =
            fixture_with_timeout(&temp, Some(Duration::from_millis(200))).await;
        cache.stream_stall_timeout = Duration::from_secs(5);
        service.0.lock().await.write_message_delay = Some(Duration::from_millis(100));
        let (bytes, digest) = streamed_blob(8);
        if from_file {
            let path = temp.path().join("blob");
            tokio::fs::write(&path, &bytes).await.unwrap();
            cache
                .write_reapi_blob_file_stream(&digest, &path, "put blob")
                .await
                .unwrap();
        } else {
            cache
                .upload_reapi_blob(&digest, &bytes, "put blob")
                .await
                .unwrap();
        }
        let state = service.0.lock().await;
        assert_eq!(state.writes.len(), 1);
        assert_eq!(state.writes[0].len(), 8);
        server.abort();
    }
}

#[tokio::test]
async fn streamed_upload_fails_without_retrying_once_the_remote_stops_reading() {
    for from_file in [false, true] {
        let temp = tempfile::TempDir::new().unwrap();
        let (mut cache, service, server) = fixture(&temp).await;
        cache.stream_stall_timeout = Duration::from_millis(300);
        service.0.lock().await.write_stops_reading = true;
        let (bytes, digest) = streamed_blob(8);
        let path = temp.path().join("blob");
        tokio::fs::write(&path, &bytes).await.unwrap();
        let upload = async {
            if from_file {
                cache
                    .write_reapi_blob_file_stream(&digest, &path, "put blob")
                    .await
            } else {
                cache.upload_reapi_blob(&digest, &bytes, "put blob").await
            }
        };
        let error = tokio::time::timeout(Duration::from_secs(5), upload)
            .await
            .expect("a stalled upload must fail at its stall timeout")
            .unwrap_err();
        assert!(error.to_string().contains("made no progress"), "{error}");
        assert_eq!(service.0.lock().await.writes.len(), 1);
        server.abort();
    }
}

#[test]
fn batch_upload_outcome_classifies_per_blob_statuses() {
    let response = |status: Option<Code>| reapi::BatchUpdateBlobsResponse {
        responses: vec![reapi::batch_update_blobs_response::Response {
            digest: None,
            status: status.map(|code| bazel_remote_apis::google::rpc::Status {
                code: code as i32,
                message: "blob status".into(),
                details: vec![],
            }),
        }],
    };
    assert!(batch_upload_outcome(&response(Some(Code::Ok)), "put blob").is_ok());
    for code in [Code::Unavailable, Code::Internal] {
        assert!(matches!(
            batch_upload_outcome(&response(Some(code)), "put blob"),
            Err(retry::Failure::Rpc(status)) if status.code() == code
        ));
    }
    for (status, message) in [
        (Some(Code::InvalidArgument), "REAPI status 3: blob status"),
        (None, "Kura returned no status for blob upload"),
    ] {
        assert!(matches!(
            batch_upload_outcome(&response(status), "put blob"),
            Err(retry::Failure::Local(error)) if error.to_string().contains(message)
        ));
    }
}

#[tokio::test]
async fn an_upload_is_reported_once_and_a_blob_the_remote_already_holds_not_at_all() {
    let temp = tempfile::TempDir::new().unwrap();
    let (cache, _service, server) = fixture(&temp).await;
    let recorder = Arc::new(crate::transfer::Recorder::default());
    let bytes = b"artifact to publish";
    let digest = cache.local.put_blob(bytes).await.unwrap();
    let output = cache.local.put_blob(b"action output").await.unwrap();
    let result = ActionResult {
        exit_code: 0,
        stdout: None,
        stderr: None,
        outputs: BTreeMap::from([("out.txt".to_string(), output)]),
    };
    crate::transfer::observe(recorder.clone(), async {
        cache.put_blob_remote(&digest, bytes).await.unwrap();
        cache.put_blob_remote(&digest, bytes).await.unwrap();
        cache
            .put_action_result_remote(&Digest::of_bytes(b"action"), &result)
            .await
            .unwrap();
    })
    .await;
    server.abort();

    let transfers = recorder.transfers();
    assert_eq!(
        transfers
            .iter()
            .map(|transfer| (transfer.direction, transfer.digest, transfer.size_bytes))
            .collect::<Vec<_>>(),
        vec![
            (TransferDirection::Upload, digest, bytes.len() as u64),
            (
                TransferDirection::Upload,
                output,
                b"action output".len() as u64
            ),
        ]
    );
}
