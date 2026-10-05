//! Cancels a test schedule when the scheduling process receives SIGINT or
//! SIGTERM.
//!
//! Each batch runs in its own `once` process that streams its own run, and a
//! CI runner cancelling a job signals only the process it started. The
//! scheduler therefore forwards the first signal to every batch process, so
//! each reports its run as cancelled, stops starting queued batches, waits a
//! bounded time for the running ones to exit, and then ends the way the signal
//! would have.

use std::collections::BTreeSet;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio::task::{JoinError, JoinHandle};

use crate::termination::{self, SignalWatcher, Termination};

/// How long batch processes get to report their cancelled runs. Covers their
/// own bounded drain while staying inside the grace period CI runners give
/// before a kill.
const BATCH_DEADLINE: Duration = Duration::from_secs(4);

struct Batches {
    running: BTreeSet<u32>,
    cancelled: Option<Termination>,
}

static BATCHES: Mutex<Batches> = Mutex::new(Batches {
    running: BTreeSet::new(),
    cancelled: None,
});

fn batches() -> std::sync::MutexGuard<'static, Batches> {
    BATCHES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Keeps a batch process registered while it runs.
pub(super) struct Tracked(u32);

impl Drop for Tracked {
    fn drop(&mut self) {
        batches().running.remove(&self.0);
    }
}

/// Register a batch process that just started. A process that raced past
/// [`cancelled`] as the schedule was cancelled is signalled at once.
pub(super) fn track(pid: u32) -> Tracked {
    let mut batches = batches();
    if let Some(signal) = batches.cancelled {
        termination::forward(pid, signal);
    }
    batches.running.insert(pid);
    Tracked(pid)
}

/// Whether the schedule was cancelled, so queued batches should not start.
pub(super) fn cancelled() -> bool {
    batches().cancelled.is_some()
}

fn cancel(signal: Termination) {
    let mut batches = batches();
    batches.cancelled = Some(signal);
    for pid in &batches.running {
        termination::forward(*pid, signal);
    }
}

/// Wait for `schedule`, cancelling its batches on the first termination
/// signal. Returns only when the schedule finishes without being signalled.
pub(super) async fn supervise<T>(schedule: JoinHandle<T>) -> Result<T, JoinError> {
    let Some(mut signals) = SignalWatcher::install() else {
        return schedule.await;
    };
    tokio::pin!(schedule);
    let signal = tokio::select! {
        result = &mut schedule => {
            if let Some(signal) = signals.restore() {
                cancel(signal);
                termination::terminate(signal);
            }
            return result;
        }
        signal = signals.recv() => signal,
    };
    cancel(signal);
    tracing::info!(
        signal = signal.name(),
        "test schedule cancelled by signal; waiting for batches to report"
    );
    let _ = tokio::time::timeout(BATCH_DEADLINE, &mut schedule).await;
    termination::terminate(signal)
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;

    use super::{cancel, track, Termination};

    // Both cases share the process-wide batch registry, so they run in order
    // inside one test.
    #[test]
    fn cancelling_signals_running_batches_and_any_started_afterwards() {
        let mut running = Command::new("sleep").arg("30").spawn().unwrap();
        let tracked = track(running.id());
        cancel(Termination::Terminate);
        assert_eq!(running.wait().unwrap().signal(), Some(libc::SIGTERM));
        drop(tracked);

        let mut late = Command::new("sleep").arg("30").spawn().unwrap();
        let tracked = track(late.id());
        assert_eq!(late.wait().unwrap().signal(), Some(libc::SIGTERM));
        drop(tracked);
    }
}
