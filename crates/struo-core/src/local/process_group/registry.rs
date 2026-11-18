//! The process-wide table of live action process groups.
//!
//! A signal handler reads the table while ordinary threads register, publish,
//! and unregister groups, so every transition is a lock-free handshake over
//! sequentially consistent atomics:
//!
//! - A slot goes `FREE -> RESERVED -> pgid -> FREE`. The spawning thread holds
//!   termination signals between reserving and publishing, so a handler that
//!   finds `RESERVED` is on another thread and can wait for the id.
//! - A handler marks the slot busy before reading the id it signals, and
//!   unregistering clears the id and then waits until the slot is not busy.
//!   Either the handler reads `FREE`, or the unregistering thread sees it busy
//!   and waits, so the leader is never reaped, and its id never reused, while a
//!   handler may still signal it.
//! - Terminating sets a flag before walking the table, and a spawn checks it
//!   after reserving. Either the walk finds the reservation and waits for it,
//!   or the spawn sees the flag and fails, so no group starts unseen while the
//!   process exits.

#![cfg(unix)]

use std::io;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::time::Duration;

use tokio::signal::unix::{signal, SignalKind};

use super::sys;

/// Fixed safety capacity of the table, independent of how many actions the
/// scheduler admits. The resource pool runs at most `cpu_slots` local actions
/// at once, which is far below this on any real host; the bound keeps the
/// table static (no allocation in a signal handler) and the handler's walk
/// short. Reaching it means something spawned outside admission control, and
/// the spawn fails instead of starting a group a forwarded signal could miss.
pub(super) const SLOTS: usize = 4096;

const FREE: i32 = 0;
const RESERVED: i32 = -1;

/// Upper bound between exit checks, in case a `SIGCHLD` is coalesced with one
/// that arrived before this action started listening.
const EXIT_POLL: Duration = Duration::from_millis(250);

static GROUPS: [AtomicI32; SLOTS] = [const { AtomicI32::new(FREE) }; SLOTS];
static BUSY: [AtomicU32; SLOTS] = [const { AtomicU32::new(0) }; SLOTS];
static TERMINATING: AtomicBool = AtomicBool::new(false);

pub(super) struct Registration {
    slot: usize,
    pgid: libc::pid_t,
}

impl Registration {
    pub(super) fn reserve() -> io::Result<Self> {
        let slot = GROUPS
            .iter()
            .position(|slot| {
                slot.compare_exchange(FREE, RESERVED, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            })
            .ok_or_else(|| {
                io::Error::other(format!(
                    "more than {SLOTS} local actions are running at once, beyond what \
                     admission control allows; refusing to start another"
                ))
            })?;
        let registration = Self { slot, pgid: 0 };
        if TERMINATING.load(Ordering::SeqCst) {
            return Err(io::Error::other(
                "Struo is exiting after a termination signal; not starting another action",
            ));
        }
        Ok(registration)
    }

    pub(super) fn assign(&mut self, leader: Option<u32>) -> io::Result<()> {
        let pgid = leader
            .and_then(|leader| libc::pid_t::try_from(leader).ok())
            .filter(|pgid| *pgid > 1)
            .ok_or_else(|| io::Error::other("spawned action has no process id"))?;
        self.pgid = pgid;
        GROUPS[self.slot].store(pgid, Ordering::SeqCst);
        Ok(())
    }

    pub(super) fn kill(&self) {
        if self.pgid > 1 {
            sys::kill_group(self.pgid, libc::SIGKILL);
        }
    }

    /// Wait for the leader to exit, leaving it unreaped.
    pub(super) async fn exited(&self) -> io::Result<()> {
        let mut children = signal(SignalKind::child())?;
        loop {
            if sys::has_exited(self.pgid)? {
                return Ok(());
            }
            tokio::select! {
                _ = children.recv() => {}
                () = tokio::time::sleep(EXIT_POLL) => {}
            }
        }
    }

    #[cfg(test)]
    pub(super) fn slot(&self) -> usize {
        self.slot
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        GROUPS[self.slot].store(FREE, Ordering::SeqCst);
        while BUSY[self.slot].load(Ordering::SeqCst) != 0 {
            std::hint::spin_loop();
            std::thread::yield_now();
        }
    }
}

/// Signal every published group, waiting for reserved slots to publish.
pub(super) fn signal_all(signal: libc::c_int) {
    for (slot, busy) in GROUPS.iter().zip(&BUSY) {
        if slot.load(Ordering::SeqCst) == FREE {
            continue;
        }
        busy.fetch_add(1, Ordering::SeqCst);
        let pgid = loop {
            let pgid = slot.load(Ordering::SeqCst);
            if pgid != RESERVED {
                break pgid;
            }
            std::hint::spin_loop();
            sys::yield_now();
        };
        if pgid > 1 {
            sys::kill_group(pgid, signal);
        }
        busy.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Refuse new spawns, then signal every group.
pub(super) fn terminate_all(signal: libc::c_int) {
    TERMINATING.store(true, Ordering::SeqCst);
    signal_all(signal);
}

#[cfg(test)]
pub(super) fn registered(pgid: libc::pid_t) -> bool {
    GROUPS
        .iter()
        .any(|slot| slot.load(Ordering::SeqCst) == pgid)
}

/// Hold `slot` busy as a handler between reading and signalling would.
#[cfg(test)]
pub(super) fn hold_busy(slot: usize) -> impl Drop {
    struct Held(usize);
    impl Drop for Held {
        fn drop(&mut self) {
            BUSY[self.0].fetch_sub(1, Ordering::SeqCst);
        }
    }
    BUSY[slot].fetch_add(1, Ordering::SeqCst);
    Held(slot)
}
