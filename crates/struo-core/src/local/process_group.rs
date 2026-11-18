//! Each local action runs as the leader of its own process group, so stopping
//! the action stops everything it started, not just the direct child.
//!
//! [`GroupChild`] owns the spawned leader and keeps it unreaped until the
//! action is done with its group: leader exit is observed without reaping it,
//! so the zombie leader stays a member of the group and the group id cannot be
//! reused while Struo may still signal it. Only [`GroupChild::reap`] reaps: it
//! waits for the leader to exit with the group still registered, kills what
//! is left of the group, then unregisters it and reaps. Dropping the handle
//! before that
//! (cancellation, a timeout, a capture error) sends `SIGKILL` to the group and
//! unregisters it before the leader can be reaped.
//!
//! Live groups are recorded in the [`registry`], which a termination handler
//! walks through [`signal_action_process_groups`].

mod registry;
mod sys;

#[cfg(all(test, unix))]
mod tests;

use std::process::ExitStatus;

use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

/// A spawned action leader together with its process group.
///
/// On Unix the leader heads a new process group registered for signal
/// forwarding, and every kill reaches the whole group. Elsewhere there are no
/// process groups: the handle only kills the leader, so descendants it started
/// can outlive a timeout or cancellation. That is a known limitation, not a
/// guarantee this type provides.
pub struct GroupChild {
    child: Child,
    #[cfg(unix)]
    group: Option<registry::Registration>,
}

impl GroupChild {
    /// Spawn `command` as the leader of a new process group.
    ///
    /// Termination signals are held on this thread from reserving the
    /// registry slot until the group id is published, so a handler never runs
    /// here while the slot is reserved, and a handler on another thread waits
    /// for the id instead of missing the group.
    ///
    /// # Errors
    ///
    /// Fails when the command cannot be spawned, when the signal table is
    /// full, or when Struo is already exiting after a termination signal.
    pub fn spawn(command: &mut Command) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            let held = sys::TerminationSignalsHeld::new()?;
            let mut group = registry::Registration::reserve()?;
            command.process_group(0);
            held.restore_in_child(command);
            command.kill_on_drop(true);
            let child = command.spawn()?;
            group.assign(child.id())?;
            drop(held);
            Ok(Self {
                child,
                group: Some(group),
            })
        }
        #[cfg(not(unix))]
        {
            command.kill_on_drop(true);
            Ok(Self {
                child: command.spawn()?,
            })
        }
    }

    /// The leader, for tests. Waiting on it directly would reap it while the
    /// group is still registered, so callers take the pipes with
    /// [`GroupChild::take_stdout`] and its siblings instead.
    #[cfg(test)]
    pub(crate) fn child(&mut self) -> &mut Child {
        &mut self.child
    }

    /// The leader's stdin pipe, if it was piped and not taken yet.
    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin.take()
    }

    /// The leader's stdout pipe, if it was piped and not taken yet.
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    /// The leader's stderr pipe, if it was piped and not taken yet.
    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }

    /// Resolve once the leader has exited, without reaping it.
    ///
    /// # Errors
    ///
    /// Fails when the leader's state cannot be observed.
    pub async fn exited(&mut self) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            match self.group.as_ref() {
                Some(group) => group.exited().await,
                None => self.child.wait().await.map(drop),
            }
        }
        #[cfg(not(unix))]
        {
            self.child.wait().await.map(drop)
        }
    }

    /// Kill every process in the group (only the leader outside Unix).
    pub fn kill_group(&mut self) {
        #[cfg(unix)]
        if let Some(group) = &self.group {
            group.kill();
        }
        #[cfg(not(unix))]
        {
            let _ = self.child.start_kill();
        }
    }

    /// Wait for the leader to exit, kill every process still in its group,
    /// then unregister the group and reap the leader. An action does not
    /// outlive its leader, so background processes it left behind are killed
    /// even when the caller never called [`GroupChild::kill_group`]. The group
    /// stays registered until the leader has exited, so dropping this future
    /// (cancellation) still kills the whole group. Processes that left the
    /// group are not reached.
    ///
    /// # Errors
    ///
    /// Fails when the leader's exit cannot be observed or it cannot be
    /// waited on. The group stays registered when observing its exit fails.
    pub async fn reap(&mut self) -> std::io::Result<ExitStatus> {
        #[cfg(unix)]
        if let Some(group) = &self.group {
            group.exited().await?;
            group.kill();
            self.group = None;
        }
        self.child.wait().await
    }
}

impl Drop for GroupChild {
    /// Kill and unregister the group here, before the `child` field drops:
    /// Tokio reaps an exited child when its handle drops, and the group must
    /// leave the registry before its id can be released.
    fn drop(&mut self) {
        self.kill_group();
        #[cfg(unix)]
        {
            self.group = None;
        }
    }
}

/// Send `signal` to every live action process group, then refuse to spawn
/// further local actions.
///
/// Action groups do not receive terminal signals aimed at Struo's own process
/// group, so a termination handler calls this before the process exits.
/// Async-signal-safe: it only touches atomics and calls `killpg` and
/// `sched_yield`. It waits for a group that is being spawned to publish its
/// id, and a spawn that starts after this call fails instead of starting a
/// group the exiting process would leave behind.
#[cfg(unix)]
pub fn signal_action_process_groups(signal: libc::c_int) {
    registry::terminate_all(signal);
}

#[cfg(all(test, unix))]
pub(super) fn registered(pgid: u32) -> bool {
    libc::pid_t::try_from(pgid).is_ok_and(registry::registered)
}
