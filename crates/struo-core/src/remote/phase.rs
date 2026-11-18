use struo_cas::ErrorKind;

use crate::{Error, Result};
use std::future::Future;
use std::time::{Duration, Instant};
use tracing::Instrument;

pub(super) async fn run<T>(
    executor: &'static str,
    phase: &'static str,
    future: impl Future<Output = Result<T>>,
) -> Result<T> {
    let span = tracing::debug_span!("executor_phase", executor, phase);
    let mut observation = Observation {
        started: Instant::now(),
        span: span.clone(),
        finished: false,
    };
    let result = future.instrument(span).await;
    observation.finished = true;
    observation.span.in_scope(|| {
        tracing::debug!(
            elapsed_ms =
                u64::try_from(observation.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            outcome = result.as_ref().map_or_else(|error| class(error), |_| "ok"),
            "executor phase completed"
        );
    });
    result
}

/// The action's time budget, spent only by the work it bounds.
///
/// Every phase that does the action's work (preparing inputs, creating the
/// machine, transferring, setting up, executing, retrieving outputs) runs
/// inside [`within`](Self::within) and draws on one shared budget. Cleanup and
/// maintenance run outside it under their own bounds, so a slow delete never
/// turns a finished action into a timeout and a spent budget never skips
/// cleanup.
pub(super) struct Deadline {
    budget: Option<Duration>,
    spent: Duration,
}

impl Deadline {
    pub(super) fn new(timeout_ms: Option<u64>) -> Self {
        Self {
            budget: timeout_ms.map(Duration::from_millis),
            spent: Duration::ZERO,
        }
    }

    pub(super) async fn within<T>(&mut self, future: impl Future<Output = Result<T>>) -> Result<T> {
        let Some(budget) = self.budget else {
            return future.await;
        };
        let started = tokio::time::Instant::now();
        let remaining = budget.saturating_sub(self.spent);
        let result = tokio::time::timeout(remaining, future).await;
        self.spent = self.spent.saturating_add(started.elapsed());
        result.map_err(|_| Error::Timeout(budget))?
    }
}

#[cfg(test)]
pub(super) mod testing {
    /// An action budget no harness wait can outlast (they give up after five
    /// seconds) and shorter than every transport bound (the shortest is
    /// reconciliation's twenty seconds), so spending it fires the action
    /// deadline and nothing else.
    pub(in crate::remote) const BUDGET_MS: u64 = 10_000;

    /// Spend the whole budget at once. The clock is paused only while the
    /// timers already due fire; sockets keep real time throughout.
    pub(in crate::remote) async fn spend_budget() {
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_millis(BUDGET_MS)).await;
        tokio::time::resume();
    }
}

struct Observation {
    started: Instant,
    span: tracing::Span,
    finished: bool,
}
impl Drop for Observation {
    fn drop(&mut self) {
        if !self.finished {
            self.span.in_scope(|| {
                tracing::debug!(
                    elapsed_ms =
                        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    outcome = "cancelled",
                    "executor phase observation ended"
                );
            });
        }
    }
}

fn class(error: &Error) -> &'static str {
    match error {
        Error::Timeout(_) => "timeout",
        Error::ExecutorStalled { .. } => "stalled",
        Error::ExecutorResourceLimit { .. } | Error::ResourceRequirementUnsatisfied { .. } => {
            "capacity"
        }
        Error::RemoteProviderStatus {
            status: 401 | 403, ..
        } => "refused",
        Error::RemoteProviderStatus {
            status: 429 | 500..=599,
            ..
        } => "unavailable",
        Error::RemoteProviderHttp { source, .. } if source.is_timeout() => "timeout",
        Error::RemoteProviderHttp { .. } => "transport",
        Error::Cas(error) => match error.kind() {
            ErrorKind::NotFound => "absent",
            ErrorKind::Unavailable => "unavailable",
            ErrorKind::Refused => "refused",
            ErrorKind::Corrupt => "integrity",
            ErrorKind::Exhausted => "capacity",
            ErrorKind::Protocol => "protocol",
            ErrorKind::Misconfigured => "unsupported",
            _ => "failed",
        },
        Error::UnsupportedExecutorRequirement { .. } | Error::RemoteProviderConfig { .. } => {
            "unsupported"
        }
        _ => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn one_budget_bounds_the_whole_phase_sequence() {
        let mut deadline = Deadline::new(Some(25));
        deadline
            .within(async {
                tokio::time::sleep(Duration::from_millis(15)).await;
                Ok(())
            })
            .await
            .unwrap();
        let result = deadline
            .within(async {
                tokio::time::sleep(Duration::from_millis(15)).await;
                Ok(())
            })
            .await;
        assert!(
            matches!(result, Err(Error::Timeout(budget)) if budget == Duration::from_millis(25))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn time_outside_the_budgeted_work_is_not_spent() {
        let mut deadline = Deadline::new(Some(25));
        deadline
            .within(async {
                tokio::time::sleep(Duration::from_millis(15)).await;
                Ok(())
            })
            .await
            .unwrap();
        // Cleanup or maintenance between phases does not draw on the budget.
        tokio::time::sleep(Duration::from_mins(1)).await;
        deadline
            .within(async {
                tokio::time::sleep(Duration::from_millis(5)).await;
                Ok(())
            })
            .await
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn no_budget_never_times_out() {
        let mut deadline = Deadline::new(None);
        deadline
            .within(async {
                tokio::time::sleep(Duration::from_hours(1)).await;
                Ok(())
            })
            .await
            .unwrap();
    }

    #[test]
    fn cache_failures_keep_their_kind() {
        let cas = |kind| {
            Error::Cas(struo_cas::Error::RemoteFailure {
                provider: "test",
                operation: "get_blob",
                kind,
                message: "secret".to_string(),
            })
        };
        for (kind, expected) in [
            (ErrorKind::NotFound, "absent"),
            (ErrorKind::Unavailable, "unavailable"),
            (ErrorKind::Refused, "refused"),
            (ErrorKind::Corrupt, "integrity"),
            (ErrorKind::Exhausted, "capacity"),
            (ErrorKind::Protocol, "protocol"),
            (ErrorKind::Misconfigured, "unsupported"),
            (ErrorKind::Io, "failed"),
            (ErrorKind::Other, "failed"),
        ] {
            assert_eq!(class(&cas(kind)), expected, "{kind:?}");
        }
    }

    #[test]
    fn diagnostics_classify_failures_without_provider_text() {
        assert_eq!(
            class(&Error::RemoteProviderStatus {
                provider: "test".into(),
                status: 503
            }),
            "unavailable"
        );
        assert_eq!(
            class(&Error::RemoteProviderStatus {
                provider: "test".into(),
                status: 403
            }),
            "refused"
        );
        assert_eq!(
            class(&Error::RemoteProviderApi {
                provider: "test".into(),
                message: "secret".into()
            }),
            "failed"
        );
    }
}
