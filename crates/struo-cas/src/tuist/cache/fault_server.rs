//! An in-memory REAPI cache that can inject faults, for tests.

use std::collections::VecDeque;
use std::pin::Pin;

use bytestream::byte_stream_server::{ByteStream, ByteStreamServer};
use futures::StreamExt;
use reapi::action_cache_server::{ActionCache, ActionCacheServer};
use reapi::content_addressable_storage_server::{
    ContentAddressableStorage, ContentAddressableStorageServer,
};
use tonic::Response;

use super::*;

type RpcResult<T> = std::result::Result<Response<T>, Status>;
type Stream<T> = Pin<Box<dyn futures::Stream<Item = std::result::Result<T, Status>> + Send>>;

#[derive(Default)]
pub(super) struct State {
    pub(super) actions: HashMap<String, reapi::ActionResult>,
    pub(super) blobs: HashMap<String, Vec<u8>>,
    /// Injected failures per method, consumed one per call.
    pub(super) failures: HashMap<&'static str, VecDeque<Status>>,
    pub(super) calls: HashMap<&'static str, usize>,
    /// Bytes appended to every byte stream read.
    pub(super) read_suffix: Vec<u8>,
    /// Replaces the body of every byte stream read.
    pub(super) read_replacement: Option<Vec<u8>>,
    /// Byte stream reads send their first chunk and then never finish.
    pub(super) read_stalls: bool,
    pub(super) batch_requests: Vec<Vec<reapi::Digest>>,
    pub(super) batch_overrides: HashMap<String, reapi::batch_read_blobs_response::Response>,
    pub(super) batch_stalls: bool,
    pub(super) batch_duplicate_response: bool,
}

#[derive(Clone, Default)]
pub(super) struct FaultServer(pub(super) Arc<Mutex<State>>);

impl FaultServer {
    async fn enter(&self, method: &'static str) -> std::result::Result<(), Status> {
        let mut state = self.0.lock().await;
        *state.calls.entry(method).or_default() += 1;
        match state.failures.get_mut(method).and_then(VecDeque::pop_front) {
            Some(status) => Err(status),
            None => Ok(()),
        }
    }

    pub(super) async fn fail(&self, method: &'static str, statuses: Vec<Status>) {
        self.0
            .lock()
            .await
            .failures
            .insert(method, VecDeque::from(statuses));
    }

    pub(super) async fn calls(&self, method: &'static str) -> usize {
        self.0.lock().await.calls.get(method).copied().unwrap_or(0)
    }

    pub(super) async fn evict_all_blobs(&self) {
        self.0.lock().await.blobs.clear();
    }

    pub(super) async fn serve(&self) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        self.serve_on(listener);
        format!("http://{address}")
    }

    pub(super) fn serve_on(&self, listener: tokio::net::TcpListener) {
        let incoming = stream::unfold(listener, |listener| async {
            Some((listener.accept().await.map(|(socket, _)| socket), listener))
        });
        let service = self.clone();
        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(ContentAddressableStorageServer::new(service.clone()))
                .add_service(ActionCacheServer::new(service.clone()))
                .add_service(ByteStreamServer::new(service))
                .serve_with_incoming(incoming)
                .await
                .unwrap();
        });
    }
}

fn resource_hash(resource: &str) -> String {
    let parts: Vec<&str> = resource.split('/').collect();
    let at = parts
        .iter()
        .position(|part| *part == "blobs")
        .expect("resource names a blob");
    parts[at + 1].to_string()
}

#[tonic::async_trait]
impl ActionCache for FaultServer {
    async fn get_action_result(
        &self,
        request: Request<reapi::GetActionResultRequest>,
    ) -> RpcResult<reapi::ActionResult> {
        self.enter("get_action").await?;
        let key = request.into_inner().action_digest.unwrap().hash;
        match self.0.lock().await.actions.get(&key) {
            Some(result) => Ok(Response::new(result.clone())),
            None => Err(Status::not_found("no action result")),
        }
    }

    async fn update_action_result(
        &self,
        request: Request<reapi::UpdateActionResultRequest>,
    ) -> RpcResult<reapi::ActionResult> {
        self.enter("update_action").await?;
        let request = request.into_inner();
        let result = request.action_result.unwrap();
        self.0
            .lock()
            .await
            .actions
            .insert(request.action_digest.unwrap().hash, result.clone());
        Ok(Response::new(result))
    }
}

#[tonic::async_trait]
impl ContentAddressableStorage for FaultServer {
    type GetTreeStream = Stream<reapi::GetTreeResponse>;
    type GetChunkMappingStream = Stream<reapi::GetChunkMappingResponse>;

    async fn find_missing_blobs(
        &self,
        request: Request<reapi::FindMissingBlobsRequest>,
    ) -> RpcResult<reapi::FindMissingBlobsResponse> {
        self.enter("find_missing").await?;
        let state = self.0.lock().await;
        Ok(Response::new(reapi::FindMissingBlobsResponse {
            missing_blob_digests: request
                .into_inner()
                .blob_digests
                .into_iter()
                .filter(|digest| !state.blobs.contains_key(&digest.hash))
                .collect(),
        }))
    }

    async fn batch_update_blobs(
        &self,
        request: Request<reapi::BatchUpdateBlobsRequest>,
    ) -> RpcResult<reapi::BatchUpdateBlobsResponse> {
        self.enter("batch_update").await?;
        let mut state = self.0.lock().await;
        let mut responses = Vec::new();
        for blob in request.into_inner().requests {
            let digest = blob.digest.unwrap();
            state.blobs.insert(digest.hash.clone(), blob.data);
            responses.push(reapi::batch_update_blobs_response::Response {
                digest: Some(digest),
                status: Some(RpcOk::ok()),
            });
        }
        Ok(Response::new(reapi::BatchUpdateBlobsResponse { responses }))
    }

    async fn batch_read_blobs(
        &self,
        request: Request<reapi::BatchReadBlobsRequest>,
    ) -> RpcResult<reapi::BatchReadBlobsResponse> {
        self.enter("batch_read").await?;
        let mut state = self.0.lock().await;
        let digests = request.into_inner().digests;
        state.batch_requests.push(digests.clone());
        if state.batch_stalls {
            drop(state);
            return std::future::pending().await;
        }
        let mut responses = digests
            .into_iter()
            .rev()
            .map(|digest| {
                if let Some(response) = state.batch_overrides.get(&digest.hash) {
                    return response.clone();
                }
                let found = state.blobs.get(&digest.hash).cloned();
                reapi::batch_read_blobs_response::Response {
                    digest: Some(digest),
                    status: Some(if found.is_some() {
                        RpcOk::ok()
                    } else {
                        RpcOk::code(Code::NotFound)
                    }),
                    data: found
                        .map(|data| state.read_replacement.clone().unwrap_or(data))
                        .unwrap_or_default(),
                    compressor: reapi::compressor::Value::Identity as i32,
                }
            })
            .collect::<Vec<_>>();
        if state.batch_duplicate_response {
            responses.push(responses[0].clone());
        }
        Ok(Response::new(reapi::BatchReadBlobsResponse { responses }))
    }

    async fn get_tree(&self, _: Request<reapi::GetTreeRequest>) -> RpcResult<Self::GetTreeStream> {
        Err(Status::unimplemented("fault server"))
    }
    async fn split_blob(
        &self,
        _: Request<reapi::SplitBlobRequest>,
    ) -> RpcResult<reapi::SplitBlobResponse> {
        Err(Status::unimplemented("fault server"))
    }
    async fn splice_blob(
        &self,
        _: Request<reapi::SpliceBlobRequest>,
    ) -> RpcResult<reapi::SpliceBlobResponse> {
        Err(Status::unimplemented("fault server"))
    }
    async fn get_chunk_mapping(
        &self,
        _: Request<reapi::GetChunkMappingRequest>,
    ) -> RpcResult<Self::GetChunkMappingStream> {
        Err(Status::unimplemented("fault server"))
    }
    async fn register_chunk_mapping(
        &self,
        _: Request<tonic::Streaming<reapi::RegisterChunkMappingRequest>>,
    ) -> RpcResult<reapi::RegisterChunkMappingResponse> {
        Err(Status::unimplemented("fault server"))
    }
}

#[tonic::async_trait]
impl ByteStream for FaultServer {
    type ReadStream = Stream<bytestream::ReadResponse>;

    async fn read(&self, request: Request<bytestream::ReadRequest>) -> RpcResult<Self::ReadStream> {
        self.enter("read").await?;
        let hash = resource_hash(&request.into_inner().resource_name);
        let state = self.0.lock().await;
        let Some(blob) = state.blobs.get(&hash) else {
            return Err(Status::not_found("blob evicted"));
        };
        let mut data = state
            .read_replacement
            .clone()
            .unwrap_or_else(|| blob.clone());
        data.extend_from_slice(&state.read_suffix);
        let chunks: Vec<_> = data
            .chunks(1024 * 1024)
            .map(|chunk| {
                Ok(bytestream::ReadResponse {
                    data: chunk.to_vec(),
                })
            })
            .collect();
        if state.read_stalls {
            let first = chunks.into_iter().take(1).collect::<Vec<_>>();
            return Ok(Response::new(Box::pin(
                stream::iter(first).chain(stream::pending()),
            )));
        }
        Ok(Response::new(Box::pin(stream::iter(chunks))))
    }

    async fn write(
        &self,
        request: Request<tonic::Streaming<bytestream::WriteRequest>>,
    ) -> RpcResult<bytestream::WriteResponse> {
        self.enter("write").await?;
        let mut chunks = request.into_inner();
        let mut resource = String::new();
        let mut data = Vec::new();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk?;
            if !chunk.resource_name.is_empty() {
                resource = chunk.resource_name;
            }
            data.extend_from_slice(&chunk.data);
        }
        let committed = i64::try_from(data.len()).unwrap();
        self.0
            .lock()
            .await
            .blobs
            .insert(resource_hash(&resource), data);
        Ok(Response::new(bytestream::WriteResponse {
            committed_size: committed,
        }))
    }

    async fn query_write_status(
        &self,
        _: Request<bytestream::QueryWriteStatusRequest>,
    ) -> RpcResult<bytestream::QueryWriteStatusResponse> {
        Err(Status::unimplemented("fault server"))
    }
}

struct RpcOk;

impl RpcOk {
    fn ok() -> bazel_remote_apis::google::rpc::Status {
        Self::code(Code::Ok)
    }

    fn code(code: Code) -> bazel_remote_apis::google::rpc::Status {
        bazel_remote_apis::google::rpc::Status {
            code: code as i32,
            message: String::new(),
            details: vec![],
        }
    }
}

/// A cache with its own local store that talks to `url`.
pub(super) async fn connected_cache(temp: &tempfile::TempDir, name: &str, url: &str) -> TuistCache {
    let cache = TuistCache::new(
        Cas::open(temp.path().join(name)),
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
    let channel = Endpoint::from_shared(url.to_string())
        .unwrap()
        .connect()
        .await
        .unwrap();
    cache.grpc_channel_cache.set(GrpcChannels {
        requests: channel.clone(),
        uploads: channel,
    });
    cache.auth_token_cache.set("test-token".into());
    cache.capabilities_cache.set(RemoteCapabilities::default());
    cache
}
