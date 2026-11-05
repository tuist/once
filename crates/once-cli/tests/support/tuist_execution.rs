//! A fake Tuist project: the remote-execution capabilities endpoint, a REAPI
//! CAS and action cache, and an execution service that runs each action in a
//! fresh copy of its input root on this machine.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use bazel_remote_apis::build::bazel::remote::execution::v2::{
    self as reapi,
    action_cache_server::{ActionCache, ActionCacheServer},
    capabilities_server::{Capabilities, CapabilitiesServer},
    content_addressable_storage_server::{
        ContentAddressableStorage, ContentAddressableStorageServer,
    },
    execution_server::{Execution, ExecutionServer},
};
use bazel_remote_apis::google::bytestream::{
    self,
    byte_stream_server::{ByteStream, ByteStreamServer},
};
use bazel_remote_apis::google::longrunning::{operation, Operation};
use bazel_remote_apis::google::protobuf::Any;
use bazel_remote_apis::google::rpc::Status as RpcStatus;
use futures::StreamExt as _;
use once_cas::ReapiMessage as _;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::Mutex;
use tonic::{Request, Response, Status, Streaming};

type Rpc<T> = Result<Response<T>, Status>;
type Stream<T> = Pin<Box<dyn futures::Stream<Item = Result<T, Status>> + Send>>;

pub const IMAGE: &str = "sha-test";

pub enum Fault {
    /// Run the action, then break its stream and forget the operation, the
    /// way Tuist loses an operation when a scheduler restarts.
    Lose,
    /// Run the action, then break its stream; `WaitExecution` for its
    /// operation returns the finished result.
    Break,
    /// Drop every stored blob, then report the action's inputs missing, the
    /// way an evicted CAS answers.
    Evict,
    /// Report the action as stopped at its timeout, with its logs.
    TimeOut,
}

#[derive(Default)]
pub struct State {
    blobs: HashMap<String, Vec<u8>>,
    actions: HashMap<String, reapi::ActionResult>,
    /// Commands each execution ran, in order.
    pub executed: Vec<Vec<String>>,
    /// Faults the next answered submissions hit, one each, in order.
    pub faults: std::collections::VecDeque<Fault>,
    /// `Execute` calls refused as a full queue before any is accepted.
    pub refuse_queue: usize,
    pub platforms: Vec<Vec<(String, String)>>,
    /// Hashes of every uploaded blob, in order, including repeats.
    pub uploads: Vec<String>,
    /// Actions submitted and not yet answered, and the most seen at once.
    in_flight: usize,
    pub max_in_flight: usize,
    /// Submissions refused because the client exceeded `max_concurrency`.
    pub over_limit: usize,
    operations: HashMap<String, reapi::ExecuteResponse>,
}

#[derive(Clone)]
pub struct Service {
    pub state: Arc<Mutex<State>>,
    root: PathBuf,
    /// Every action shares one execution root, so they run one at a time.
    serial: Arc<Mutex<()>>,
    /// Image of a `linux/amd64` container that runs each action the way a
    /// Tuist worker does; actions run on this machine when absent.
    worker: Option<String>,
    /// Advertised account concurrency. More actions in flight than this are
    /// refused, stricter than Tuist's queue so a client that ignores it fails.
    max_concurrency: usize,
}

pub struct Server {
    pub service: Service,
    pub url: String,
    _root: tempfile::TempDir,
}

/// Tuist workers materialize every action at this root.
const WORKER_ROOT: &str = "/workspace/action";

pub async fn start() -> Server {
    start_with(None, 4).await
}

pub async fn start_with(worker: Option<String>, max_concurrency: usize) -> Server {
    let root = tempfile::tempdir().unwrap();
    let service = Service {
        state: Arc::default(),
        root: root.path().join("action"),
        serial: Arc::default(),
        worker,
        max_concurrency,
    };
    let execution_root = if service.worker.is_some() {
        PathBuf::from(WORKER_ROOT)
    } else {
        service.root.clone()
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let grpc = format!("http://{}", listener.local_addr().unwrap());
    let incoming = futures::stream::unfold(listener, |listener| async {
        Some((listener.accept().await.map(|(socket, _)| socket), listener))
    });
    let grpc_service = service.clone();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(ContentAddressableStorageServer::new(grpc_service.clone()))
            .add_service(ActionCacheServer::new(grpc_service.clone()))
            .add_service(CapabilitiesServer::new(grpc_service.clone()))
            .add_service(ByteStreamServer::new(grpc_service.clone()))
            .add_service(ExecutionServer::new(grpc_service))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    let http = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", http.local_addr().unwrap());
    let body = serde_json::json!({
        "image": IMAGE,
        "endpoint": grpc,
        "cas_endpoint": grpc,
        "max_concurrency": max_concurrency,
        "leases": false,
        "execution_root": execution_root,
    })
    .to_string();
    let endpoints = serde_json::json!({ "endpoints": [grpc] }).to_string();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = http.accept().await.unwrap();
            let (body, endpoints) = (body.clone(), endpoints.clone());
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0; 4096];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let read = socket.read(&mut buffer).await.unwrap();
                    if read == 0 {
                        return;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                let request = String::from_utf8_lossy(&request);
                let authorized = request
                    .lines()
                    .any(|line| line.eq_ignore_ascii_case("authorization: Bearer test-token"));
                let capabilities =
                    request.starts_with("GET /api/remote-execution/acme/app/capabilities ");
                let discovery =
                    request.starts_with("GET /api/cache/endpoints?account_handle=acme ");
                let (status, body) = match (capabilities, discovery, authorized) {
                    (_, _, false) => ("403 Forbidden", "{}".to_string()),
                    (true, _, true) => ("200 OK", body),
                    (_, true, true) => ("200 OK", endpoints),
                    _ => ("404 Not Found", "{}".to_string()),
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    Server {
        service,
        url,
        _root: root,
    }
}

fn digest(bytes: &[u8]) -> reapi::Digest {
    reapi::Digest {
        hash: hex::encode(Sha256::digest(bytes)),
        size_bytes: i64::try_from(bytes.len()).unwrap(),
    }
}

impl Service {
    async fn blob(&self, digest: &reapi::Digest) -> Result<Vec<u8>, Status> {
        if digest.size_bytes == 0 {
            return Ok(Vec::new());
        }
        self.state
            .lock()
            .await
            .blobs
            .get(&digest.hash)
            .cloned()
            .ok_or_else(|| Status::failed_precondition("missing input"))
    }

    async fn put(&self, bytes: Vec<u8>) -> reapi::Digest {
        let digest = digest(&bytes);
        self.state
            .lock()
            .await
            .blobs
            .insert(digest.hash.clone(), bytes);
        digest
    }

    async fn materialize(&self, digest: &reapi::Digest, path: &Path) -> Result<(), Status> {
        std::fs::create_dir_all(path).unwrap();
        let directory = reapi::Directory::decode(self.blob(digest).await?.as_slice()).unwrap();
        for file in &directory.files {
            let target = path.join(&file.name);
            std::fs::write(&target, self.blob(file.digest.as_ref().unwrap()).await?).unwrap();
            if file.is_executable {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        for link in &directory.symlinks {
            std::os::unix::fs::symlink(&link.target, path.join(&link.name)).unwrap();
        }
        for child in &directory.directories {
            Box::pin(self.materialize(child.digest.as_ref().unwrap(), &path.join(&child.name)))
                .await?;
        }
        Ok(())
    }

    async fn directory(&self, path: &Path, tree: &mut Vec<reapi::Directory>) -> reapi::Digest {
        let mut directory = reapi::Directory::default();
        let mut entries = std::fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let name = entry.file_name().into_string().unwrap();
            if entry.file_type().unwrap().is_dir() {
                let digest = Box::pin(self.directory(&entry.path(), tree)).await;
                directory.directories.push(reapi::DirectoryNode {
                    name,
                    digest: Some(digest),
                });
            } else {
                let digest = self.put(std::fs::read(entry.path()).unwrap()).await;
                directory.files.push(reapi::FileNode {
                    name,
                    digest: Some(digest),
                    ..Default::default()
                });
            }
        }
        let bytes = directory.encode_to_vec();
        tree.push(directory);
        digest(&bytes)
    }

    fn spawn(
        &self,
        command: &reapi::Command,
        working: &Path,
    ) -> Result<std::process::Output, Status> {
        Ok(match &self.worker {
            None => std::process::Command::new(worker_program(command, working)?)
                .args(&command.arguments[1..])
                .current_dir(working)
                .env_clear()
                .envs(
                    command
                        .environment_variables
                        .iter()
                        .map(|variable| (&variable.name, &variable.value)),
                )
                .output()
                .unwrap(),
            Some(image) => {
                let mut docker = std::process::Command::new("docker");
                docker.args([
                    "run",
                    "--rm",
                    "--platform",
                    "linux/amd64",
                    "--network",
                    "none",
                ]);
                docker.args(["--user", "1000:1000", "--entrypoint", ""]);
                docker
                    .arg("-v")
                    .arg(format!("{}:{WORKER_ROOT}", self.root.display()));
                docker
                    .arg("-w")
                    .arg(Path::new(WORKER_ROOT).join(&command.working_directory));
                for variable in &command.environment_variables {
                    docker
                        .arg("--env")
                        .arg(format!("{}={}", variable.name, variable.value));
                }
                docker.arg(image).args(&command.arguments);
                docker.output().unwrap()
            }
        })
    }

    async fn run(&self, action_digest: &reapi::Digest) -> Result<reapi::ExecuteResponse, Status> {
        let action = reapi::Action::decode(self.blob(action_digest).await?.as_slice()).unwrap();
        let command = reapi::Command::decode(
            self.blob(action.command_digest.as_ref().unwrap())
                .await?
                .as_slice(),
        )
        .unwrap();
        let platform = action
            .platform
            .unwrap_or_default()
            .properties
            .into_iter()
            .map(|property| (property.name, property.value))
            .collect::<Vec<_>>();
        self.state.lock().await.platforms.push(platform.clone());
        if !platform.contains(&("tuist-image".into(), IMAGE.into())) {
            return Err(Status::failed_precondition(
                "unsupported execution platform",
            ));
        }
        let _ = std::fs::remove_dir_all(&self.root);
        self.materialize(action.input_root_digest.as_ref().unwrap(), &self.root)
            .await?;
        if self.worker.is_some() {
            // The worker's unprivileged user owns the action root.
            std::process::Command::new("chmod")
                .args(["-R", "a+rwX"])
                .arg(&self.root)
                .status()
                .unwrap();
        }
        let working = self.root.join(&command.working_directory);
        for output in &command.output_paths {
            std::fs::create_dir_all(working.join(output).parent().unwrap()).unwrap();
        }
        let output = self.spawn(&command, &working)?;
        self.state
            .lock()
            .await
            .executed
            .push(command.arguments.clone());
        let mut result = reapi::ActionResult {
            exit_code: output.status.code().unwrap_or(-1),
            stdout_digest: Some(self.put(output.stdout).await),
            stderr_digest: Some(self.put(output.stderr).await),
            ..Default::default()
        };
        for path in &command.output_paths {
            let produced = working.join(path);
            if produced.is_dir() {
                let mut directories = Vec::new();
                self.directory(&produced, &mut directories).await;
                let root = directories.pop();
                let tree = reapi::Tree {
                    root,
                    children: directories,
                };
                result.output_directories.push(reapi::OutputDirectory {
                    path: path.clone(),
                    tree_digest: Some(self.put(tree.encode_to_vec()).await),
                    ..Default::default()
                });
            } else if produced.is_file() {
                result.output_files.push(reapi::OutputFile {
                    path: path.clone(),
                    digest: Some(self.put(std::fs::read(&produced).unwrap()).await),
                    ..Default::default()
                });
            }
        }
        Ok(reapi::ExecuteResponse {
            result: Some(result),
            status: Some(RpcStatus::default()),
            ..Default::default()
        })
    }
}

fn done(response: &reapi::ExecuteResponse) -> Operation {
    Operation {
        name: "operations/finished".into(),
        metadata: None,
        done: true,
        result: Some(operation::Result::Response(Any {
            type_url: "type.googleapis.com/build.bazel.remote.execution.v2.ExecuteResponse".into(),
            value: response.encode_to_vec(),
        })),
    }
}

#[tonic::async_trait]
impl Execution for Service {
    type ExecuteStream = Stream<Operation>;
    type WaitExecutionStream = Stream<Operation>;

    async fn execute(&self, request: Request<reapi::ExecuteRequest>) -> Rpc<Self::ExecuteStream> {
        let request = request.into_inner();
        assert_eq!(request.instance_name, "app");
        let digest = request.action_digest.unwrap();
        {
            let mut state = self.state.lock().await;
            if state.refuse_queue > 0 {
                state.refuse_queue -= 1;
                return Err(Status::resource_exhausted("execution queue is full"));
            }
            if state.in_flight >= self.max_concurrency {
                state.over_limit += 1;
                return Err(Status::resource_exhausted("execution queue is full"));
            }
            state.in_flight += 1;
            state.max_in_flight = state.max_in_flight.max(state.in_flight);
        }
        let response = {
            let _serial = self.serial.lock().await;
            self.answer(&digest).await
        };
        self.state.lock().await.in_flight -= 1;
        Ok(Response::new(futures::stream::iter(response).boxed()))
    }

    async fn wait_execution(
        &self,
        request: Request<reapi::WaitExecutionRequest>,
    ) -> Rpc<Self::WaitExecutionStream> {
        let name = request.into_inner().name;
        let response = self
            .state
            .lock()
            .await
            .operations
            .remove(&name)
            .ok_or_else(|| Status::not_found("operation not found"))?;
        Ok(Response::new(
            futures::stream::iter([Ok(done(&response))]).boxed(),
        ))
    }
}

/// `PreconditionFailure { violations: [{ type: "MISSING" }] }`, encoded.
const MISSING_PRECONDITION: &[u8] = b"\x0a\x09\x0a\x07MISSING";

/// Resolve the program the way the worker does: a bare name only through the
/// action's own `PATH`.
fn worker_program(command: &reapi::Command, working: &Path) -> Result<PathBuf, Status> {
    let program = &command.arguments[0];
    if program.contains('/') {
        return Ok(working.join(program));
    }
    let path = command
        .environment_variables
        .iter()
        .find(|variable| variable.name == "PATH")
        .map_or("", |variable| variable.value.as_str());
    std::env::split_paths(path)
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| Status::invalid_argument("executable is absent from action PATH"))
}

impl Service {
    async fn answer(&self, digest: &reapi::Digest) -> Vec<Result<Operation, Status>> {
        let fault = {
            let mut state = self.state.lock().await;
            let fault = state.faults.pop_front();
            if matches!(fault, Some(Fault::Evict)) {
                state.blobs.clear();
            }
            fault
        };
        if matches!(fault, Some(Fault::Evict)) {
            return vec![Ok(done(&reapi::ExecuteResponse {
                status: Some(RpcStatus {
                    code: tonic::Code::FailedPrecondition as i32,
                    details: vec![Any {
                        type_url: "type.googleapis.com/google.rpc.PreconditionFailure".into(),
                        value: MISSING_PRECONDITION.to_vec(),
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            }))];
        }
        let response = self.run(digest).await;
        match response {
            _ if matches!(fault, Some(Fault::Lose)) => vec![
                Ok(Operation {
                    name: "operations/lost".into(),
                    ..Default::default()
                }),
                Err(Status::unavailable("scheduler restarted")),
            ],
            Ok(response) if matches!(fault, Some(Fault::Break)) => {
                self.state
                    .lock()
                    .await
                    .operations
                    .insert("operations/broken".into(), response);
                vec![
                    Ok(Operation {
                        name: "operations/broken".into(),
                        ..Default::default()
                    }),
                    Err(Status::unavailable("stream reset")),
                ]
            }
            Ok(mut response) if matches!(fault, Some(Fault::TimeOut)) => {
                response.status = Some(RpcStatus {
                    code: tonic::Code::DeadlineExceeded as i32,
                    ..Default::default()
                });
                vec![Ok(done(&response))]
            }
            Ok(response) => vec![Ok(done(&response))],
            Err(status) => vec![Ok(done(&reapi::ExecuteResponse {
                status: Some(RpcStatus {
                    code: status.code() as i32,
                    ..Default::default()
                }),
                ..Default::default()
            }))],
        }
    }
}

#[tonic::async_trait]
impl ContentAddressableStorage for Service {
    type GetTreeStream = Stream<reapi::GetTreeResponse>;
    type GetChunkMappingStream = Stream<reapi::GetChunkMappingResponse>;

    async fn find_missing_blobs(
        &self,
        request: Request<reapi::FindMissingBlobsRequest>,
    ) -> Rpc<reapi::FindMissingBlobsResponse> {
        let state = self.state.lock().await;
        Ok(Response::new(reapi::FindMissingBlobsResponse {
            missing_blob_digests: request
                .into_inner()
                .blob_digests
                .into_iter()
                .filter(|digest| digest.size_bytes > 0 && !state.blobs.contains_key(&digest.hash))
                .collect(),
        }))
    }

    async fn batch_update_blobs(
        &self,
        request: Request<reapi::BatchUpdateBlobsRequest>,
    ) -> Rpc<reapi::BatchUpdateBlobsResponse> {
        let mut responses = Vec::new();
        for item in request.into_inner().requests {
            let digest = self.put(item.data).await;
            self.state.lock().await.uploads.push(digest.hash.clone());
            assert_eq!(
                Some(&digest),
                item.digest.as_ref(),
                "uploaded bytes match their digest"
            );
            responses.push(reapi::batch_update_blobs_response::Response {
                digest: Some(digest),
                status: Some(RpcStatus::default()),
            });
        }
        Ok(Response::new(reapi::BatchUpdateBlobsResponse { responses }))
    }

    async fn batch_read_blobs(
        &self,
        request: Request<reapi::BatchReadBlobsRequest>,
    ) -> Rpc<reapi::BatchReadBlobsResponse> {
        let state = self.state.lock().await;
        let responses = request
            .into_inner()
            .digests
            .into_iter()
            .map(|digest| {
                let found = state.blobs.get(&digest.hash).cloned();
                reapi::batch_read_blobs_response::Response {
                    status: Some(RpcStatus {
                        code: if found.is_some() || digest.size_bytes == 0 {
                            0
                        } else {
                            5
                        },
                        ..Default::default()
                    }),
                    data: found.unwrap_or_default(),
                    digest: Some(digest),
                    compressor: 0,
                }
            })
            .collect();
        Ok(Response::new(reapi::BatchReadBlobsResponse { responses }))
    }

    async fn get_tree(&self, _: Request<reapi::GetTreeRequest>) -> Rpc<Self::GetTreeStream> {
        Err(Status::unimplemented("get tree"))
    }

    async fn split_blob(
        &self,
        _: Request<reapi::SplitBlobRequest>,
    ) -> Rpc<reapi::SplitBlobResponse> {
        Err(Status::unimplemented("split blob"))
    }

    async fn splice_blob(
        &self,
        _: Request<reapi::SpliceBlobRequest>,
    ) -> Rpc<reapi::SpliceBlobResponse> {
        Err(Status::unimplemented("splice blob"))
    }

    async fn get_chunk_mapping(
        &self,
        _: Request<reapi::GetChunkMappingRequest>,
    ) -> Rpc<Self::GetChunkMappingStream> {
        Err(Status::unimplemented("chunk mapping"))
    }

    async fn register_chunk_mapping(
        &self,
        _: Request<Streaming<reapi::RegisterChunkMappingRequest>>,
    ) -> Rpc<reapi::RegisterChunkMappingResponse> {
        Err(Status::unimplemented("chunk mapping"))
    }
}

#[tonic::async_trait]
impl ActionCache for Service {
    async fn get_action_result(
        &self,
        request: Request<reapi::GetActionResultRequest>,
    ) -> Rpc<reapi::ActionResult> {
        let hash = request.into_inner().action_digest.unwrap().hash;
        self.state
            .lock()
            .await
            .actions
            .get(&hash)
            .cloned()
            .map(Response::new)
            .ok_or_else(|| Status::not_found("no action result"))
    }

    async fn update_action_result(
        &self,
        request: Request<reapi::UpdateActionResultRequest>,
    ) -> Rpc<reapi::ActionResult> {
        let request = request.into_inner();
        let result = request.action_result.unwrap_or_default();
        self.state
            .lock()
            .await
            .actions
            .insert(request.action_digest.unwrap().hash, result.clone());
        Ok(Response::new(result))
    }
}

#[tonic::async_trait]
impl Capabilities for Service {
    async fn get_capabilities(
        &self,
        _: Request<reapi::GetCapabilitiesRequest>,
    ) -> Rpc<reapi::ServerCapabilities> {
        Ok(Response::new(reapi::ServerCapabilities {
            cache_capabilities: Some(reapi::CacheCapabilities {
                digest_functions: vec![reapi::digest_function::Value::Sha256 as i32],
                max_batch_total_size_bytes: 4 * 1024 * 1024,
                ..Default::default()
            }),
            ..Default::default()
        }))
    }
}

fn resource_hash(resource: &str) -> String {
    let parts = resource.split('/').collect::<Vec<_>>();
    parts[parts.len() - 2].to_string()
}

#[tonic::async_trait]
impl ByteStream for Service {
    type ReadStream = Stream<bytestream::ReadResponse>;

    async fn read(&self, request: Request<bytestream::ReadRequest>) -> Rpc<Self::ReadStream> {
        let hash = resource_hash(&request.into_inner().resource_name);
        let data = self
            .state
            .lock()
            .await
            .blobs
            .get(&hash)
            .cloned()
            .ok_or_else(|| Status::not_found("blob"))?;
        Ok(Response::new(
            futures::stream::iter([Ok(bytestream::ReadResponse { data })]).boxed(),
        ))
    }

    async fn write(
        &self,
        request: Request<Streaming<bytestream::WriteRequest>>,
    ) -> Rpc<bytestream::WriteResponse> {
        let mut stream = request.into_inner();
        let mut data = Vec::new();
        let mut hash = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if !chunk.resource_name.is_empty() {
                hash = resource_hash(&chunk.resource_name);
            }
            data.extend(chunk.data);
        }
        let committed = i64::try_from(data.len()).unwrap();
        self.state.lock().await.uploads.push(hash.clone());
        assert_eq!(
            self.put(data).await.hash,
            hash,
            "streamed bytes match their digest"
        );
        Ok(Response::new(bytestream::WriteResponse {
            committed_size: committed,
        }))
    }

    async fn query_write_status(
        &self,
        _: Request<bytestream::QueryWriteStatusRequest>,
    ) -> Rpc<bytestream::QueryWriteStatusResponse> {
        Err(Status::unimplemented("query write status"))
    }
}
