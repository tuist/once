//! System calls behind process-group supervision: `killpg`, a non-reaping
//! `waitid`, and the per-thread signal mask held around a spawn.

// None of these calls has a safe wrapper. Scoped here so the rest of the crate
// stays under the workspace-wide `unsafe_code = "deny"`.
#![cfg(unix)]
#![allow(unsafe_code)]

use std::io;

use tokio::process::Command;

pub(super) fn kill_group(pgid: libc::pid_t, signal: libc::c_int) {
    // SAFETY: `killpg` takes plain integers; callers pass ids above 1, so it
    // never addresses the caller's own group or every process.
    unsafe {
        libc::killpg(pgid, signal);
    }
}

/// `sched_yield`, which is a plain system call and safe in a signal handler.
pub(super) fn yield_now() {
    // SAFETY: no arguments, no memory access.
    unsafe {
        libc::sched_yield();
    }
}

/// Whether `pid`, a child of this process, has exited. Uses `WNOWAIT`, so the
/// child stays a zombie and keeps its group id reserved.
pub(super) fn has_exited(pid: libc::pid_t) -> io::Result<bool> {
    loop {
        // SAFETY: `siginfo_t` is plain old data; all-zero is a valid value and
        // lets an untouched struct mean "no state change".
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is a valid, writable `siginfo_t`; `WNOWAIT` leaves the
        // child waitable, so its owner can still reap it.
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                libc::id_t::try_from(pid).expect("pids are positive"),
                &raw mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if rc == 0 {
            return Ok(info.si_signo == libc::SIGCHLD);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// Holds `SIGINT` and `SIGTERM` on the current thread until dropped. A signal
/// that arrives meanwhile is handled on another thread or, if every thread
/// holds it, here once the guard drops.
pub(super) struct TerminationSignalsHeld {
    previous: libc::sigset_t,
}

impl TerminationSignalsHeld {
    pub(super) fn new() -> io::Result<Self> {
        // SAFETY: `sigset_t` is plain old data, initialised by `sigemptyset`
        // before use.
        let mut held: libc::sigset_t = unsafe { std::mem::zeroed() };
        let mut previous: libc::sigset_t = unsafe { std::mem::zeroed() };
        // SAFETY: both sets are valid and writable.
        let rc = unsafe {
            libc::sigemptyset(&raw mut held);
            libc::sigaddset(&raw mut held, libc::SIGINT);
            libc::sigaddset(&raw mut held, libc::SIGTERM);
            libc::pthread_sigmask(libc::SIG_BLOCK, &raw const held, &raw mut previous)
        };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc));
        }
        Ok(Self { previous })
    }

    /// The standard library passes the spawning thread's mask to the child,
    /// so give the child the mask this thread had before the signals were
    /// held. Otherwise the action would start with termination signals held.
    pub(super) fn restore_in_child(&self, command: &mut Command) {
        let previous = self.previous;
        // SAFETY: the closure runs in the child between fork and exec and only
        // calls `pthread_sigmask`, which is async-signal-safe.
        unsafe {
            command.pre_exec(move || {
                let rc = libc::pthread_sigmask(
                    libc::SIG_SETMASK,
                    &raw const previous,
                    std::ptr::null_mut(),
                );
                if rc == 0 {
                    Ok(())
                } else {
                    Err(io::Error::from_raw_os_error(rc))
                }
            });
        }
    }
}

impl Drop for TerminationSignalsHeld {
    fn drop(&mut self) {
        // SAFETY: `previous` was filled by `pthread_sigmask` in `new`.
        unsafe {
            libc::pthread_sigmask(
                libc::SIG_SETMASK,
                &raw const self.previous,
                std::ptr::null_mut(),
            );
        }
    }
}
