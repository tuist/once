//! Remote execution through a Tuist project's REAPI execution service.
//!
//! The Tuist server names the execution endpoint, the CAS its workers read,
//! the worker image, and the absolute root actions run in. Inputs are uploaded
//! to that CAS before `Execute`, and outputs are read back from it.
//!
//! REAPI allows an action to run more than once, and Tuist re-executes when it
//! loses an operation, so only duplicate-safe actions may use this path.

use std::time::Duration;

use bazel_remote_apis::build::bazel::remote::execution::v2::execution_client::ExecutionClient;
use bazel_remote_apis::google::longrunning::{operation, Operation};
use futures::StreamExt;
use prost::Message as _;
use tonic::Status;

use super::*;

const FIND_MISSING_CHUNK: usize = 4096;
/// Submissions of one action, counting resubmissions after a lost operation.
const EXECUTE_SUBMISSIONS: u32 = 3;
/// Reattachments to one operation after its stream broke.
const EXECUTE_REATTACHMENTS: u32 = 5;
/// Queueing and transfer time allowed beyond the action's own timeout.
const EXECUTE_SLACK: Duration = Duration::from_mins(5);
/// Workers apply this timeout when an action declares none.
pub const DEFAULT_EXECUTION_TIMEOUT: Duration = Duration::from_mins(10);
const PRECONDITION_FAILURE_TYPE: &str = "type.googleapis.com/google.rpc.PreconditionFailure";

/// A connection to the project's remote execution service and the CAS its
/// workers read.
#[derive(Debug, Clone)]
pub struct TuistExecution {
    cas: TuistCache,
    executor: Channel,
    image: String,
    root: String,
}

#[derive(serde::Deserialize)]
struct Capabilities {
    image: String,
    endpoint: String,
    cas_endpoint: String,
    #[serde(default)]
    execution_root: Option<String>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct PreconditionFailure {
    #[prost(message, repeated, tag = "1")]
    violations: Vec<PreconditionViolation>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct PreconditionViolation {
    #[prost(string, tag = "1")]
    r#type: String,
}

impl TuistCache {
    /// Resolve the project's execution service once per process.
    pub async fn execution(&self) -> Result<TuistExecution> {
        let this = self.clone();
        self.execution_cache
            .get_or_try_init(move || async move { this.connect_execution().await })
            .await
            .cloned()
    }

    async fn connect_execution(&self) -> Result<TuistExecution> {
        let operation = "discover remote execution";
        let Some(project) = self.config.project.as_deref() else {
            return Err(status::remote(
                operation,
                ErrorKind::Misconfigured,
                "remote execution requires a Tuist `project`",
            ));
        };
        let url = self.server_url(&format!(
            "api/remote-execution/{}/{project}/capabilities",
            self.account()
        ))?;
        let token = self.auth_token().await?;
        let response = self
            .authorized_request(Method::GET, url, token)
            .await?
            .send()
            .await
            .map_err(|source| {
                status::remote(
                    operation,
                    ErrorKind::Unavailable,
                    redact::transport(&source),
                )
            })?;
        let code = response.status();
        if !code.is_success() {
            let hint = match code.as_u16() {
                403 => "; the account needs remote execution enabled and the credential needs read and write access to the project",
                503 => "; the account has no remote execution capacity configured",
                _ => "",
            };
            return Err(status::remote(
                operation,
                status::http_kind(code),
                format!("{}{hint}", redact::http_status(code)),
            ));
        }
        let body = response.bytes().await.map_err(|source| {
            status::remote(
                operation,
                ErrorKind::Unavailable,
                redact::transport(&source),
            )
        })?;
        let capabilities: Capabilities = serde_json::from_slice(&body).map_err(|source| {
            status::remote(
                operation,
                ErrorKind::Protocol,
                format!(
                    "malformed capabilities at line {} column {}",
                    source.line(),
                    source.column()
                ),
            )
        })?;
        let root = capabilities
            .execution_root
            .filter(|root| root.starts_with('/') && !root.split('/').any(|part| part == ".."))
            .ok_or_else(|| {
                status::remote(
                    operation,
                    ErrorKind::Protocol,
                    "the Tuist server did not advertise an absolute execution root; update the server to one that supports Once remote execution",
                )
            })?;
        let executor = grpc_endpoint(&capabilities.endpoint)
            .map_err(|message| status::remote(operation, ErrorKind::Misconfigured, message))?
            .http2_keep_alive_interval(Duration::from_secs(30))
            .keep_alive_while_idle(true)
            .connect_lazy();
        tracing::debug!(
            executor = %redact::endpoint(&capabilities.endpoint),
            cas = %redact::endpoint(&capabilities.cas_endpoint),
            image = %capabilities.image,
            "resolved Tuist remote execution"
        );
        Ok(TuistExecution {
            cas: self.pinned_to(&capabilities.cas_endpoint),
            executor,
            image: capabilities.image,
            root,
        })
    }

    /// A cache that talks to `endpoint`, the CAS execution workers read. An
    /// explicit endpoint override wins: it names the same CAS from inside a
    /// network that cannot reach the public one.
    fn pinned_to(&self, advertised: &str) -> Self {
        let mut cache = self.clone();
        if let Some(endpoint) = &cache.endpoint_override {
            if endpoint != advertised {
                tracing::warn!(
                    cache_override = %redact::endpoint(endpoint),
                    advertised = %redact::endpoint(advertised),
                    "TUIST_CACHE_ENDPOINT replaces the CAS the execution service advertises; inputs uploaded there must be readable by its workers"
                );
            }
            return cache;
        }
        cache.endpoint_override = Some(advertised.to_string());
        cache.grpc_channel_cache = Arc::default();
        cache.capabilities_cache = Arc::default();
        cache.known_remote_blobs = Arc::default();
        cache.batch_downloads = Arc::default();
        cache.execution_cache = Arc::default();
        cache
    }
}

impl TuistExecution {
    /// Worker image tag the service runs actions in.
    pub fn image(&self) -> &str {
        &self.image
    }

    /// Absolute directory each action's input root is materialized at.
    pub fn execution_root(&self) -> &str {
        &self.root
    }

    /// The subset of `digests` the execution CAS does not hold.
    pub async fn find_missing(&self, digests: Vec<reapi::Digest>) -> Result<Vec<reapi::Digest>> {
        let operation = "find missing execution inputs";
        let channel = self.cas.grpc_channel().await?;
        let client = ContentAddressableStorageClient::new(channel)
            .max_decoding_message_size(METADATA_RESPONSE_LIMIT);
        let mut missing = Vec::new();
        for chunk in digests.chunks(FIND_MISSING_CHUNK) {
            let request = reapi::FindMissingBlobsRequest {
                instance_name: self.cas.instance_name(),
                blob_digests: chunk.to_vec(),
                digest_function: SHA256_DIGEST_FUNCTION,
            };
            let response = retry::grpc(operation, || {
                let mut client = client.clone();
                let request = request.clone();
                async move {
                    let _memory = self.cas.reserve_metadata().await?;
                    Ok(client
                        .find_missing_blobs(
                            self.cas.authorized_grpc_request(request, operation).await?,
                        )
                        .await?)
                }
            })
            .await?;
            missing.extend(response.into_inner().missing_blob_digests);
        }
        Ok(missing)
    }

    /// Upload a blob held in memory, such as an encoded directory or command.
    pub async fn upload_blob(&self, digest: &reapi::Digest, bytes: &[u8]) -> Result<()> {
        validate_sha256_digest(digest, bytes, "upload execution input")?;
        self.cas
            .upload_reapi_blob(digest, bytes, "upload execution input")
            .await
    }

    /// Stream a file whose content `digest` describes.
    pub async fn upload_file(&self, digest: &reapi::Digest, path: &Path) -> Result<()> {
        let operation = "upload execution input";
        let _memory = self
            .cas
            .budget
            .reserve_memory(file_upload_memory(digest.size_bytes))
            .await?;
        let _permit = self
            .cas
            .transfer_limit
            .acquire()
            .await
            .expect("transfer semaphore remains open");
        self.cas
            .write_reapi_blob_file_stream(digest, path, operation)
            .await
    }

    /// Copy a verified output blob into the local store and return its local
    /// digest.
    pub async fn mirror_blob(&self, digest: &reapi::Digest) -> Result<Digest> {
        self.cas
            .mirror_reapi_blob(digest, "read execution output")
            .await
    }

    /// Run `action`, which must already be in the execution CAS with every
    /// blob it references, and return its result.
    ///
    /// A broken stream reattaches to the operation; a lost operation is
    /// submitted again. A failure the service reports for the action itself is
    /// classified: missing inputs are [`ErrorKind::NotFound`], a rejected
    /// platform or request is [`ErrorKind::Misconfigured`], and an exhausted
    /// queue or worker outage is [`ErrorKind::Unavailable`].
    pub async fn execute(
        &self,
        action: &reapi::Digest,
        timeout: Option<Duration>,
    ) -> Result<reapi::ActionResult> {
        let deadline =
            Instant::now() + timeout.unwrap_or(DEFAULT_EXECUTION_TIMEOUT) + EXECUTE_SLACK;
        let client = ExecutionClient::new(self.executor.clone())
            .max_decoding_message_size(METADATA_RESPONSE_LIMIT);
        let mut operation_name = None::<String>;
        let (mut submissions, mut reattachments) = (0, 0);
        loop {
            if operation_name.is_none() {
                submissions += 1;
            } else {
                reattachments += 1;
            }
            let attempt = async {
                let mut client = client.clone();
                let stream = match &operation_name {
                    None => {
                        let request = reapi::ExecuteRequest {
                            instance_name: self.cas.instance_name(),
                            skip_cache_lookup: false,
                            action_digest: Some(action.clone()),
                            execution_policy: None,
                            results_cache_policy: None,
                            digest_function: SHA256_DIGEST_FUNCTION,
                            inline_stdout: false,
                            inline_stderr: false,
                            inline_output_files: Vec::new(),
                        };
                        client
                            .execute(self.cas.authorized_grpc_request(request, "execute").await?)
                            .await
                    }
                    Some(name) => {
                        let request = reapi::WaitExecutionRequest { name: name.clone() };
                        client
                            .wait_execution(
                                self.cas.authorized_grpc_request(request, "execute").await?,
                            )
                            .await
                    }
                };
                follow(stream, &mut operation_name).await
            };
            let outcome = tokio::time::timeout_at(deadline, attempt)
                .await
                .map_err(|_| {
                    status::remote(
                        "execute",
                        ErrorKind::Unavailable,
                        "remote execution did not finish before its deadline",
                    )
                })??;
            let status = match outcome {
                Ok(response) => return response_result(response),
                Err(status) => status,
            };
            let lost = status.code() == Code::NotFound && operation_name.is_some();
            let broken = matches!(
                status.code(),
                Code::Unavailable
                    | Code::Aborted
                    | Code::Cancelled
                    | Code::Unknown
                    | Code::Internal
            );
            if lost || (broken && operation_name.is_none()) {
                operation_name = None;
                if submissions < EXECUTE_SUBMISSIONS {
                    tracing::debug!(code = ?status.code(), "submitting the remote action again");
                    continue;
                }
            } else if broken && reattachments < EXECUTE_REATTACHMENTS {
                tracing::debug!(code = ?status.code(), "reattaching to the remote operation");
                continue;
            }
            return Err(status::grpc_error("execute", &status));
        }
    }
}

/// Read operation updates until one is done, remembering the operation's
/// name so a broken stream can reattach to it.
/// The outer error is a malformed answer, which retrying cannot fix; the
/// inner one is a transport or service status the caller classifies.
async fn follow(
    stream: std::result::Result<tonic::Response<tonic::Streaming<Operation>>, Status>,
    name: &mut Option<String>,
) -> Result<std::result::Result<reapi::ExecuteResponse, Status>> {
    let protocol = |message: &str| status::remote("execute", ErrorKind::Protocol, message);
    let mut stream = match stream {
        Ok(stream) => stream.into_inner(),
        Err(status) => return Ok(Err(status)),
    };
    while let Some(update) = stream.next().await {
        let update = match update {
            Ok(update) => update,
            Err(status) => return Ok(Err(status)),
        };
        if !update.name.is_empty() {
            *name = Some(update.name.clone());
        }
        if !update.done {
            continue;
        }
        return match update.result {
            Some(operation::Result::Response(any)) => {
                reapi::ExecuteResponse::decode(any.value.as_slice())
                    .map(Ok)
                    .map_err(|_| protocol("malformed execute response"))
            }
            Some(operation::Result::Error(error)) => Ok(Err(Status::new(
                Code::from_i32(error.code),
                "operation failed",
            ))),
            None => Err(protocol("operation finished without a result")),
        };
    }
    Ok(Err(Status::unavailable(
        "execution stream ended before the operation finished",
    )))
}

fn response_result(response: reapi::ExecuteResponse) -> Result<reapi::ActionResult> {
    if let Some(error) = response
        .status
        .as_ref()
        .filter(|status| status.code != REAPI_STATUS_OK)
    {
        return Err(action_failure(error));
    }
    response.result.ok_or_else(|| {
        status::remote(
            "execute",
            ErrorKind::Protocol,
            "the execute response carried no action result",
        )
    })
}

/// Describe a failure the service reports for the action. The server's text
/// is not quoted; see `redact`.
fn action_failure(error: &bazel_remote_apis::google::rpc::Status) -> Error {
    let code = Code::from_i32(error.code);
    let missing = error.details.iter().any(|detail| {
        detail.type_url == PRECONDITION_FAILURE_TYPE
            && PreconditionFailure::decode(detail.value.as_slice()).is_ok_and(|failure| {
                failure
                    .violations
                    .iter()
                    .any(|violation| violation.r#type == "MISSING")
            })
    });
    let (kind, message) = match code {
        Code::FailedPrecondition if missing => (
            ErrorKind::NotFound,
            "the execution CAS is missing an input of the action",
        ),
        Code::FailedPrecondition => (
            ErrorKind::Misconfigured,
            "the worker rejected the execution platform; the configured image must match the image the project's execution service runs",
        ),
        Code::InvalidArgument => (
            ErrorKind::Misconfigured,
            "the worker rejected the action, for example its timeout, paths, environment, or an executable missing from the image",
        ),
        Code::DeadlineExceeded => (ErrorKind::Other, "the action exceeded its timeout"),
        Code::ResourceExhausted => (
            ErrorKind::Unavailable,
            "the project's execution queue or worker resources are exhausted",
        ),
        _ => (status::grpc_kind(code), "the remote action failed"),
    };
    status::remote(
        "execute",
        kind,
        format!("REAPI status {}: {message}", error.code),
    )
}

#[cfg(test)]
mod tests {
    use bazel_remote_apis::google::{protobuf::Any, rpc::Status as RpcStatus};

    use super::*;

    fn failure(code: Code, missing: bool) -> Error {
        let violations = vec![PreconditionViolation {
            r#type: if missing { "MISSING" } else { "OTHER" }.into(),
        }];
        action_failure(&RpcStatus {
            code: code as i32,
            message: "token=secret".into(),
            details: vec![Any {
                type_url: PRECONDITION_FAILURE_TYPE.into(),
                value: PreconditionFailure { violations }.encode_to_vec(),
            }],
        })
    }

    #[test]
    fn only_missing_precondition_violations_are_absence() {
        assert_eq!(
            failure(Code::FailedPrecondition, true).kind(),
            ErrorKind::NotFound
        );
        assert_eq!(
            failure(Code::FailedPrecondition, false).kind(),
            ErrorKind::Misconfigured
        );
        assert_eq!(
            failure(Code::ResourceExhausted, false).kind(),
            ErrorKind::Unavailable
        );
        assert!(!failure(Code::InvalidArgument, false)
            .to_string()
            .contains("secret"));
    }
}
