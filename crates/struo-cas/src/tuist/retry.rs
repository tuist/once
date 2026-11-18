use std::future::Future;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::time::Instant;
use tonic::{Code, Status};

use crate::{Error, Result};

const ATTEMPTS: u32 = 3;
/// First backoff ceiling. Each retry doubles it up to `MAX_DELAY`, and the
/// actual sleep is drawn uniformly below the ceiling (full jitter) so clients
/// that failed together do not retry together.
const BASE_DELAY: Duration = Duration::from_millis(250);
const MAX_DELAY: Duration = Duration::from_secs(4);
/// No new attempt starts once this much time has passed since the first, so
/// a slow failing remote cannot hold a caller for attempts times its timeout.
const RETRY_BUDGET: Duration = Duration::from_secs(45);

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

/// Only use for idempotent RPCs: reads, existence checks, and writes keyed by
/// content or by a key whose value the write fully replaces. Streaming callers
/// must recreate the body and upload resource for every attempt, not replay a
/// partially consumed stream.
pub(super) async fn grpc<T, F, Fut>(operation: &'static str, mut call: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = std::result::Result<T, Failure>>,
{
    let started = Instant::now();
    for attempt in 1..=ATTEMPTS {
        // Boxed so callers' futures do not inline the RPC state of every attempt.
        match Box::pin(call()).await {
            Ok(value) => return Ok(value),
            Err(Failure::Rpc(status)) if retryable(&status) && attempt < ATTEMPTS => {
                // A server hint is a floor; jitter on top keeps clients the
                // server throttled together from returning together.
                let hint = super::retry_hint::server_delay(&status);
                let delay = hint.unwrap_or_default().saturating_add(jittered(attempt));
                if started.elapsed() + delay >= RETRY_BUDGET {
                    tracing::warn!(
                        operation,
                        attempt,
                        code = ?status.code(),
                        "Tuist cache RPC retry budget exhausted"
                    );
                    return Err(super::status::grpc_error(operation, &status));
                }
                tracing::warn!(
                    operation,
                    attempt,
                    delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                    server_hint_ms = hint.map(|hint| u64::try_from(hint.as_millis()).unwrap_or(u64::MAX)),
                    error = %super::status::grpc_status_message(&status),
                    "retrying Tuist cache RPC after a transient failure"
                );
                tokio::time::sleep(delay).await;
            }
            Err(Failure::Rpc(status)) => return Err(super::status::grpc_error(operation, &status)),
            Err(Failure::Local(error)) => return Err(error),
        }
    }
    unreachable!("the last attempt returns its result")
}

fn ceiling(attempt: u32) -> Duration {
    BASE_DELAY
        .saturating_mul(1_u32 << attempt.saturating_sub(1).min(16))
        .min(MAX_DELAY)
}

fn jittered(attempt: u32) -> Duration {
    let ceiling = u64::try_from(ceiling(attempt).as_micros()).unwrap_or(u64::MAX);
    Duration::from_micros(random() % ceiling.saturating_add(1))
}

/// A per-call random value. `RandomState` is seeded randomly per process and
/// per instance, which is all jitter needs.
fn random() -> u64 {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    std::collections::hash_map::RandomState::new()
        .hash_one(SEQUENCE.fetch_add(1, Ordering::Relaxed))
}

/// Transient failures are retried. Quota exhaustion is retried only when
/// the server says when to come back. Refusals and malformed requests never
/// are, whatever their details say.
pub(super) fn retryable(status: &Status) -> bool {
    let overloaded = status.code() == Code::ResourceExhausted
        && super::retry_hint::server_delay(status).is_some();
    (transient(status.code()) || overloaded) && !timed_out(status)
}

// Tonic reports GOAWAY(NO_ERROR) and h2 stream resets as Internal, and
// cancels other in-flight RPCs on the retired connection.
fn transient(code: Code) -> bool {
    matches!(
        code,
        Code::Unavailable | Code::Internal | Code::Unknown | Code::Aborted | Code::Cancelled
    )
}

// Tonic also reports its own request timeout as Cancelled. Retrying it would
// multiply the configured timeout instead of recovering a dropped connection.
pub(super) fn timed_out(status: &Status) -> bool {
    let mut source = std::error::Error::source(status);
    while let Some(error) = source {
        if error.is::<tonic::TimeoutExpired>() {
            return true;
        }
        source = error.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::super::retry_hint::hinted as hint;
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn backoff_is_full_jitter_below_a_doubling_capped_ceiling() {
        assert_eq!(ceiling(1), BASE_DELAY);
        assert_eq!(ceiling(2), BASE_DELAY * 2);
        assert_eq!(ceiling(30), MAX_DELAY);
        let samples: Vec<Duration> = (0..64).map(|_| jittered(3)).collect();
        assert!(samples.iter().all(|delay| *delay <= ceiling(3)));
        assert!(
            samples.windows(2).any(|pair| pair[0] != pair[1]),
            "jitter must vary between retries"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn server_retry_hints_delay_the_retry_and_allow_overload_retries() {
        let started = Instant::now();
        let mut outcomes = VecDeque::from([
            Err(Failure::Rpc(hint(
                Code::ResourceExhausted,
                Duration::from_secs(5),
            ))),
            Ok(9),
        ]);
        let value = grpc("get action result", || {
            std::future::ready(outcomes.pop_front().unwrap())
        })
        .await
        .unwrap();
        assert_eq!(value, 9);
        assert!(started.elapsed() >= Duration::from_secs(5));
        assert!(started.elapsed() <= Duration::from_secs(5) + ceiling(1));
    }

    #[tokio::test(start_paused = true)]
    async fn a_hint_beyond_the_budget_ends_retries() {
        let mut calls = 0;
        let error = grpc::<(), _, _>("get action result", || {
            calls += 1;
            async {
                tokio::time::sleep(RETRY_BUDGET.saturating_sub(Duration::from_secs(10))).await;
                Err(Failure::Rpc(hint(
                    Code::Unavailable,
                    Duration::from_secs(20),
                )))
            }
        })
        .await
        .unwrap_err();
        assert_eq!(calls, 1);
        assert!(error.is_unavailable(), "{error}");
    }

    #[tokio::test(start_paused = true)]
    async fn refusals_are_not_retried_even_with_a_hint() {
        for code in [
            Code::Unauthenticated,
            Code::PermissionDenied,
            Code::InvalidArgument,
            Code::FailedPrecondition,
        ] {
            let mut calls = 0;
            assert!(grpc::<(), _, _>("get action result", || {
                calls += 1;
                std::future::ready(Err(Failure::Rpc(hint(code, Duration::from_millis(1)))))
            })
            .await
            .is_err());
            assert_eq!(calls, 1, "{code:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn retries_stop_struo_the_total_budget_is_spent() {
        let started = Instant::now();
        let mut calls = 0;
        let error = grpc::<(), _, _>("head blob", || {
            calls += 1;
            async {
                // Each attempt fails slowly, as a remote that accepts the
                // connection but never answers would.
                tokio::time::sleep(RETRY_BUDGET).await;
                Err(Failure::Rpc(Status::unavailable("slow outage")))
            }
        })
        .await
        .unwrap_err();
        assert_eq!(calls, 1);
        assert!(started.elapsed() < RETRY_BUDGET + MAX_DELAY);
        assert!(error.is_unavailable(), "{error}");
    }

    #[tokio::test]
    async fn retries_transport_failures_before_success() {
        let mut outcomes = VecDeque::from([
            Err(Failure::Rpc(Status::internal(
                "h2 protocol error: http2 error",
            ))),
            Err(Failure::Rpc(Status::cancelled("operation was canceled"))),
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
        assert!(error.is_unavailable());
        assert!(
            !error.to_string().contains("offline"),
            "server text is not repeated"
        );
    }

    #[tokio::test]
    #[ignore = "requires STRUO_LIVE_CACHE_ENDPOINT; sends 1200 read-only, unauthenticated RPCs"]
    async fn live_connection_retirement_recovers() {
        use bazel_remote_apis::build::bazel::remote::execution::v2::{
            capabilities_client::CapabilitiesClient, GetCapabilitiesRequest,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use tonic::transport::{ClientTlsConfig, Endpoint};

        let url =
            std::env::var("STRUO_LIVE_CACHE_ENDPOINT").expect("set STRUO_LIVE_CACHE_ENDPOINT");
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
    async fn request_timeouts_are_not_retried() {
        let mut calls = 0;
        let error = grpc::<(), _, _>("put blob", || {
            calls += 1;
            let mut status = Status::cancelled("Timeout expired");
            status.set_source(std::sync::Arc::new(tonic::TimeoutExpired(())));
            std::future::ready(Err(Failure::Rpc(status)))
        })
        .await
        .unwrap_err();
        assert_eq!(calls, 1);
        assert!(
            error.to_string().contains("the request timed out"),
            "{error}"
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
