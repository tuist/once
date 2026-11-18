//! Lazily initialized remote state that recovers from transient failures.
//!
//! A plain `OnceCell<Result<..>>` keeps the first failure for the life of the
//! process, so one dropped connection or a discovery outage would disable the
//! remote tier for a long-running session. Here only success is permanent.
//! A refusal or misconfiguration is kept because retrying cannot fix it. Any
//! other failure is replayed to callers until a cooldown passes, then exactly
//! one probe runs.
//!
//! The probe is owned by its own task, not by the caller that started it.
//! Cancelling that caller does not cancel the probe or hand the slot to the
//! next caller: every waiter, including ones that arrive later, receives the
//! probe's single outcome. The probe has a deadline, so a remote that never
//! answers cannot hold the slot forever.

use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;

use crate::{Error, ErrorKind, Result};

const FIRST_COOLDOWN: Duration = Duration::from_secs(1);
const MAX_COOLDOWN: Duration = Duration::from_secs(30);
/// Longest a single probe may run. Endpoint discovery, endpoint probing, and
/// connecting each have shorter timeouts of their own.
pub(super) const PROBE_DEADLINE: Duration = Duration::from_mins(1);

#[derive(Debug)]
pub(super) struct Recovering<T> {
    ready: OnceLock<T>,
    phase: Mutex<Phase>,
}

#[derive(Debug)]
enum Phase {
    Idle,
    Probing {
        done: watch::Receiver<bool>,
    },
    Failed {
        error: Error,
        failures: u32,
        retry_at: Instant,
    },
}

/// What a caller does once it has inspected the phase and released its lock.
enum Next {
    Wait(watch::Receiver<bool>),
    Fail(Error),
}

impl<T> Default for Recovering<T> {
    fn default() -> Self {
        Self {
            ready: OnceLock::new(),
            phase: Mutex::new(Phase::Idle),
        }
    }
}

impl<T: Send + Sync + 'static> Recovering<T> {
    /// The initialized value, initializing it with `init` when no probe is
    /// running and no cooldown applies.
    ///
    /// The future `init` returns runs on its own task, so it must own what it
    /// uses.
    pub(super) async fn get_or_try_init<F, Fut>(self: &Arc<Self>, init: F) -> Result<&T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        let mut init = Some(init);
        loop {
            if let Some(value) = self.ready.get() {
                return Ok(value);
            }
            let next = {
                let mut phase = self.phase.lock().expect("phase lock is not poisoned");
                if let Some(value) = self.ready.get() {
                    return Ok(value);
                }
                match &*phase {
                    Phase::Probing { done } => Next::Wait(done.clone()),
                    Phase::Failed {
                        error, retry_at, ..
                    } if is_permanent(error.kind()) || Instant::now() < *retry_at => {
                        Next::Fail(replay(error))
                    }
                    Phase::Idle | Phase::Failed { .. } => match init.take() {
                        Some(init) => Next::Wait(self.start_probe(&mut phase, init())),
                        // The probe this caller started has finished and its
                        // cooldown already passed; report how it ended.
                        None => Next::Fail(match &*phase {
                            Phase::Failed { error, .. } => replay(error),
                            _ => ended_without_outcome(),
                        }),
                    },
                }
            };
            match next {
                Next::Fail(error) => return Err(error),
                // A closed channel means the probe task ended; the phase
                // records how, so the loop reads it either way.
                Next::Wait(mut done) => {
                    let _ = done.wait_for(|done| *done).await;
                }
            }
        }
    }

    fn start_probe<Fut>(self: &Arc<Self>, phase: &mut Phase, probe: Fut) -> watch::Receiver<bool>
    where
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        let failures = match phase {
            Phase::Failed { failures, .. } => *failures,
            _ => 0,
        };
        let (sender, receiver) = watch::channel(false);
        *phase = Phase::Probing {
            done: receiver.clone(),
        };
        let mut owner = ProbeOwner {
            cell: Arc::clone(self),
            failures,
            sender: Some(sender),
        };
        tokio::spawn(async move {
            let outcome = match tokio::time::timeout(PROBE_DEADLINE, probe).await {
                Ok(outcome) => outcome,
                Err(_) => Err(Error::remote(
                    super::PROVIDER_NAME,
                    "initialize remote cache",
                    ErrorKind::Unavailable,
                    format!("initialization did not finish within {PROBE_DEADLINE:?}"),
                )),
            };
            owner.finish(outcome);
        });
        receiver
    }

    fn record(&self, outcome: Result<T>, previous_failures: u32) {
        let mut phase = self.phase.lock().expect("phase lock is not poisoned");
        match outcome {
            Ok(value) => {
                let _ = self.ready.set(value);
                *phase = Phase::Idle;
            }
            Err(error) => {
                let failures = previous_failures.saturating_add(1);
                let cooldown = cooldown(failures);
                tracing::debug!(
                    error = %error,
                    failures,
                    cooldown_ms = u64::try_from(cooldown.as_millis()).unwrap_or(u64::MAX),
                    "remote cache initialization failed"
                );
                *phase = Phase::Failed {
                    error,
                    failures,
                    retry_at: Instant::now() + cooldown,
                };
            }
        }
    }

    #[cfg(test)]
    pub(super) fn set(&self, value: T) {
        self.ready.set(value).ok().expect("cell is empty");
    }
}

/// Owns the single probe slot. Finishing records the outcome before waking
/// waiters. Dropping without finishing, because the task panicked or the
/// runtime shut down, records a failure so the slot cannot stay taken.
struct ProbeOwner<T: Send + Sync + 'static> {
    cell: Arc<Recovering<T>>,
    failures: u32,
    sender: Option<watch::Sender<bool>>,
}

impl<T: Send + Sync + 'static> ProbeOwner<T> {
    fn finish(&mut self, outcome: Result<T>) {
        let Some(sender) = self.sender.take() else {
            return;
        };
        self.cell.record(outcome, self.failures);
        let _ = sender.send(true);
    }
}

impl<T: Send + Sync + 'static> Drop for ProbeOwner<T> {
    fn drop(&mut self) {
        self.finish(Err(ended_without_outcome()));
    }
}

fn ended_without_outcome() -> Error {
    Error::remote(
        super::PROVIDER_NAME,
        "initialize remote cache",
        ErrorKind::Other,
        "initialization ended without an outcome",
    )
}

fn is_permanent(kind: ErrorKind) -> bool {
    matches!(kind, ErrorKind::Refused | ErrorKind::Misconfigured)
}

fn cooldown(failures: u32) -> Duration {
    FIRST_COOLDOWN
        .saturating_mul(1_u32 << failures.saturating_sub(1).min(16))
        .min(MAX_COOLDOWN)
}

/// An equivalent error for another caller, keeping its classification.
fn replay(error: &Error) -> Error {
    match error {
        Error::BlobNotFound(digest) => Error::BlobNotFound(*digest),
        Error::InvalidConfig { provider, message } => Error::InvalidConfig {
            provider,
            message: message.clone(),
        },
        Error::Remote {
            provider,
            operation,
            message,
        } => Error::Remote {
            provider,
            operation,
            message: message.clone(),
        },
        Error::RemoteFailure {
            provider,
            operation,
            kind,
            message,
        } => Error::remote(provider, operation, *kind, message.clone()),
        Error::Io { .. } | Error::Corrupt(..) => Error::remote(
            super::PROVIDER_NAME,
            "initialize remote cache",
            error.kind(),
            error.to_string(),
        ),
    }
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
