use std::future::Future;
use std::time::Duration;

use tonic::{Code, Status};

use crate::{Error, Result};

const ATTEMPTS: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_millis(250);

pub(super) enum Failure {
    Rpc(Status),
    Local(Error),
}

impl From<Status> for Failure {
    fn from(status: Status) -> Self {
        Self::Rpc(status)
    }
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self::Local(error)
    }
}

/// Only use for idempotent RPCs. Streaming callers must recreate the body and
/// upload resource for every attempt, not replay a partially consumed stream.
pub(super) async fn grpc<T, F, Fut>(operation: &'static str, mut call: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = std::result::Result<T, Failure>>,
{
    for attempt in 1..=ATTEMPTS {
        // Boxed so callers' futures do not inline the RPC state of every attempt.
        match Box::pin(call()).await {
            Ok(value) => return Ok(value),
            Err(Failure::Rpc(status)) if transient(status.code()) && attempt < ATTEMPTS => {
                tracing::debug!(operation, attempt, code = ?status.code(), error = status.message(), source = ?std::error::Error::source(&status), "retrying Tuist cache RPC");
                tokio::time::sleep(RETRY_DELAY * attempt).await;
            }
            Err(Failure::Rpc(status)) => return Err(super::cache::grpc_error(operation, &status)),
            Err(Failure::Local(error)) => return Err(error),
        }
    }
    unreachable!("the last attempt returns its result")
}

// Tonic reports GOAWAY(NO_ERROR) as Internal and cancels other in-flight RPCs.
pub(super) fn transient(code: Code) -> bool {
    matches!(
        code,
        Code::Unavailable | Code::Internal | Code::Unknown | Code::Aborted | Code::Cancelled
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[tokio::test]
    async fn retries_transport_failures_before_success() {
        let mut outcomes = VecDeque::from([
            Err(Failure::Rpc(Status::internal(
                "h2 protocol error: http2 error",
            ))),
            Err(Failure::Rpc(Status::cancelled("Timeout expired"))),
            Ok(42),
        ]);
        assert_eq!(
            grpc("put blob", || std::future::ready(
                outcomes.pop_front().unwrap()
            ))
            .await
            .unwrap(),
            42
        );
        assert!(outcomes.is_empty());
    }

    #[tokio::test]
    async fn persistent_transport_failures_stop_after_three_attempts() {
        let mut calls = 0;
        let error = grpc::<(), _, _>("head blob", || {
            calls += 1;
            std::future::ready(Err(Failure::Rpc(Status::unavailable("offline"))))
        })
        .await
        .unwrap_err();
        assert_eq!(calls, ATTEMPTS);
        assert!(error.to_string().contains("head blob"));
        assert!(error.to_string().contains("offline"));
    }

    #[tokio::test]
    #[ignore = "requires ONCE_LIVE_CACHE_ENDPOINT; sends 1200 read-only, unauthenticated RPCs"]
    async fn live_connection_retirement_recovers() {
        use bazel_remote_apis::build::bazel::remote::execution::v2::{
            capabilities_client::CapabilitiesClient, GetCapabilitiesRequest,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use tonic::transport::{ClientTlsConfig, Endpoint};

        let url = std::env::var("ONCE_LIVE_CACHE_ENDPOINT").expect("set ONCE_LIVE_CACHE_ENDPOINT");
        let mut endpoint = Endpoint::from_shared(url.clone())
            .unwrap()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10));
        if url.starts_with("https://") {
            endpoint = endpoint
                .tls_config(ClientTlsConfig::new().with_enabled_roots())
                .unwrap();
        }
        let channel = endpoint.connect().await.unwrap();
        let attempts = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let client = CapabilitiesClient::new(channel.clone());
            let attempts = Arc::clone(&attempts);
            tasks.push(tokio::spawn(async move {
                for _ in 0..150 {
                    grpc("discover capabilities", || {
                        let mut client = client.clone();
                        attempts.fetch_add(1, Ordering::Relaxed);
                        async move {
                            let request = GetCapabilitiesRequest {
                                instance_name: String::new(),
                            };
                            match client.get_capabilities(request).await {
                                Ok(_) => Ok(()),
                                // A refusal confirms the RPC arrived. No credentials or writes needed.
                                Err(status)
                                    if matches!(
                                        status.code(),
                                        Code::Unauthenticated | Code::PermissionDenied
                                    ) =>
                                {
                                    Ok(())
                                }
                                Err(status) => Err(status.into()),
                            }
                        }
                    })
                    .await
                    .unwrap();
                }
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        println!(
            "1200 live RPCs completed in {} attempts",
            attempts.load(Ordering::Relaxed)
        );
    }

    #[tokio::test]
    async fn refusals_and_local_errors_are_not_retried() {
        for failure in [
            Failure::Rpc(Status::unauthenticated("expired token")),
            Failure::Rpc(Status::permission_denied("no access")),
            Failure::Rpc(Status::invalid_argument("bad digest")),
            Failure::Rpc(Status::not_found("absent")),
            Failure::Rpc(Status::resource_exhausted("quota exhausted")),
            Failure::Rpc(Status::deadline_exceeded("deadline exceeded")),
            Failure::Local(Error::InvalidConfig {
                provider: "tuist",
                message: "invalid endpoint".into(),
            }),
        ] {
            let mut outcome = Some(failure);
            let mut calls = 0;
            assert!(grpc::<(), _, _>("put blob", || {
                calls += 1;
                std::future::ready(Err(outcome.take().expect("must not retry")))
            })
            .await
            .is_err());
            assert_eq!(calls, 1);
        }
    }
}
