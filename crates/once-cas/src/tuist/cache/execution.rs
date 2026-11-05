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
use tokio::sync::OwnedSemaphorePermit;
use tonic::Status;

use super::*;

const FIND_MISSING_CHUNK: usize = 4096;
/// Submissions of one action, counting resubmissions after a lost operation.
const EXECUTE_SUBMISSIONS: u32 = 3;
/// Reattachments to one operation after its stream broke.
const EXECUTE_REATTACHMENTS: u32 = 5;
/// Time allowed beyond the action's own timeout: Tuist keeps an action
/// queued for up to fifteen minutes, plus transfer time.
const EXECUTE_SLACK: Duration = Duration::from_mins(20);
/// Workers apply this timeout when an action declares none.
pub const DEFAULT_EXECUTION_TIMEOUT: Duration = Duration::from_mins(10);
/// Wait before resubmitting to a full queue, doubling up to the maximum.
const QUEUE_BACKOFF: Duration = Duration::from_millis(500);
const MAX_QUEUE_BACKOFF: Duration = Duration::from_secs(10);
/// The capabilities document is a handful of short fields.
const CAPABILITIES_LIMIT: usize = 64 * 1024;
const PRECONDITION_FAILURE_TYPE: &str = "type.googleapis.com/google.rpc.PreconditionFailure";

/// A connection to the project's remote execution service and the CAS its
/// workers read.
#[derive(Debug, Clone)]
pub struct TuistExecution {
    cas: TuistCache,
    executor: Channel,
    image: String,
    root: String,
    /// Actions in flight, bounded by the account's advertised concurrency so
    /// a wide build waits here instead of overflowing the service's queue.
    slots: Arc<Semaphore>,
}

/// The result of an action the service ran to completion or to its timeout.
#[derive(Debug)]
pub struct Executed {
    /// What the worker reported for the command.
    pub result: reapi::ActionResult,
    /// The worker stopped the command at its timeout; `result` holds what it
    /// captured until then.
    pub timed_out: bool,
}

#[derive(serde::Deserialize)]
struct Capabilities {
    image: String,
    endpoint: String,
    cas_endpoint: String,
    #[serde(default)]
    max_concurrency: usize,
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
        let body = read_capabilities(response, operation).await?;
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
            slots: Arc::new(Semaphore::new(
                capabilities
                    .max_concurrency
                    .clamp(1, Semaphore::MAX_PERMITS),
            )),
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

    /// Wait for one of the account's execution slots. Hold it from upload
    /// through execution.
    pub async fn admit(&self) -> OwnedSemaphorePermit {
        Arc::clone(&self.slots)
            .acquire_owned()
            .await
            .expect("execution slots are never closed")
    }

    /// The subset of `digests` the execution CAS does not hold. A rejected
    /// credential is resolved again once.
    pub async fn find_missing(&self, digests: Vec<reapi::Digest>) -> Result<Vec<reapi::Digest>> {
        self.refreshing(|| self.find_missing_once(&digests)).await
    }

    /// Run `call`, and once more after resolving the credential again when the
    /// CAS rejects it, so a token that expires during a long build recovers.
    async fn refreshing<T, F>(&self, call: impl Fn() -> F) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        match call().await {
            Err(error) if error.kind() == ErrorKind::Refused => {
                self.cas.refresh_auth_token().await?;
                call().await
            }
            outcome => outcome,
        }
    }

    async fn find_missing_once(&self, digests: &[reapi::Digest]) -> Result<Vec<reapi::Digest>> {
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
        self.refreshing(|| {
            self.cas
                .upload_reapi_blob(digest, bytes, "upload execution input")
        })
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
        self.refreshing(|| {
            self.cas
                .write_reapi_blob_file_stream(digest, path, operation)
        })
        .await
    }

    /// Copy a verified output blob into the local store and return its local
    /// digest.
    pub async fn mirror_blob(&self, digest: &reapi::Digest) -> Result<Digest> {
        self.refreshing(|| self.cas.mirror_reapi_blob(digest, "read execution output"))
            .await
    }

    /// Run `action`, which must already be in the execution CAS with every
    /// blob it references, and return its result.
    ///
    /// A full queue is waited out with backoff, a broken stream reattaches to
    /// the operation, and a lost operation or failed worker is submitted
    /// again. A rejected credential is resolved again once. A failure the
    /// service reports for the action itself is classified: missing inputs
    /// are [`ErrorKind::NotFound`], a rejected platform or request is
    /// [`ErrorKind::Misconfigured`], and a worker limit the action exceeded is
    /// [`ErrorKind::Exhausted`].
    pub async fn execute(
        &self,
        action: &reapi::Digest,
        timeout: Option<Duration>,
    ) -> Result<Executed> {
        let deadline =
            Instant::now() + timeout.unwrap_or(DEFAULT_EXECUTION_TIMEOUT) + EXECUTE_SLACK;
        let client = ExecutionClient::new(self.executor.clone())
            .max_decoding_message_size(METADATA_RESPONSE_LIMIT);
        let mut operation_name = None::<String>;
        let (mut submissions, mut reattachments) = (0, 0);
        let mut refreshed = false;
        let mut backoff = QUEUE_BACKOFF;
        loop {
            let submitting = operation_name.is_none();
            if submitting {
                submissions += 1;
            } else {
                reattachments += 1;
            }
            // Undo this attempt's count when it ended in a wait that does not
            // spend a retry.
            let uncount = |submissions: &mut u32, reattachments: &mut u32| {
                if submitting {
                    *submissions -= 1;
                } else {
                    *reattachments -= 1;
                }
            };
            let attempt = self.attempt(&client, action, &mut operation_name);
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
                Ok(response) => {
                    let worker_failed = response
                        .status
                        .as_ref()
                        .is_some_and(|status| Code::from_i32(status.code) == Code::Unavailable);
                    if worker_failed && submissions < EXECUTE_SUBMISSIONS {
                        tracing::debug!(
                            "submitting the remote action again after a worker failure"
                        );
                        operation_name = None;
                        continue;
                    }
                    return response_result(response);
                }
                Err(status) => status,
            };
            if status.code() == Code::ResourceExhausted {
                // A full queue or a gateway with too many open streams; the
                // request was refused before it was accepted or reattached.
                uncount(&mut submissions, &mut reattachments);
                wait_while_busy(&mut backoff, deadline).await?;
                continue;
            }
            if matches!(
                status.code(),
                Code::Unauthenticated | Code::PermissionDenied
            ) && !refreshed
            {
                refreshed = true;
                uncount(&mut submissions, &mut reattachments);
                self.cas.refresh_auth_token().await?;
                continue;
            }
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
                if lost {
                    // Not missing inputs: the service kept losing the
                    // operation, so uploading again would not help.
                    return Err(status::remote(
                        "execute",
                        ErrorKind::Unavailable,
                        "the execution service lost the operation on every submission",
                    ));
                }
            } else if broken && reattachments < EXECUTE_REATTACHMENTS {
                tracing::debug!(code = ?status.code(), "reattaching to the remote operation");
                continue;
            }
            return Err(status::grpc_error("execute", &status));
        }
    }
    /// Submit the action, or reattach to its operation once it has a name.
    async fn attempt(
        &self,
        client: &ExecutionClient<Channel>,
        action: &reapi::Digest,
        operation_name: &mut Option<String>,
    ) -> Result<std::result::Result<reapi::ExecuteResponse, Status>> {
        let mut client = client.clone();
        let stream = match &*operation_name {
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
                    .wait_execution(self.cas.authorized_grpc_request(request, "execute").await?)
                    .await
            }
        };
        follow(stream, operation_name).await
    }
}

async fn read_capabilities(
    mut response: reqwest::Response,
    operation: &'static str,
) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|source| {
        status::remote(
            operation,
            ErrorKind::Unavailable,
            redact::transport(&source),
        )
    })? {
        if body.len() + chunk.len() > CAPABILITIES_LIMIT {
            return Err(status::remote(
                operation,
                ErrorKind::Protocol,
                "the capabilities response exceeds its size limit",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Back off before asking a busy service again, failing once the wait would
/// pass the action's deadline.
async fn wait_while_busy(backoff: &mut Duration, deadline: Instant) -> Result<()> {
    if Instant::now() + *backoff >= deadline {
        return Err(status::remote(
            "execute",
            ErrorKind::Unavailable,
            "the project's execution service stayed busy until the action's deadline",
        ));
    }
    tracing::debug!(?backoff, "execution service is busy; waiting");
    tokio::time::sleep(*backoff).await;
    *backoff = (*backoff * 2).min(MAX_QUEUE_BACKOFF);
    Ok(())
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

fn response_result(response: reapi::ExecuteResponse) -> Result<Executed> {
    let code = response
        .status
        .as_ref()
        .map_or(REAPI_STATUS_OK, |status| status.code);
    let timed_out = Code::from_i32(code) == Code::DeadlineExceeded;
    if code != REAPI_STATUS_OK && !(timed_out && response.result.is_some()) {
        return Err(action_failure(
            response
                .status
                .as_ref()
                .expect("a failing status is present"),
        ));
    }
    let result = response.result.ok_or_else(|| {
        status::remote(
            "execute",
            ErrorKind::Protocol,
            "the execute response carried no action result",
        )
    })?;
    Ok(Executed { result, timed_out })
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
            ErrorKind::Exhausted,
            "the action exceeded a worker limit, such as 64 MiB of logs or the size of its outputs",
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
            ErrorKind::Exhausted
        );
        assert!(!failure(Code::InvalidArgument, false)
            .to_string()
            .contains("secret"));
    }
}
