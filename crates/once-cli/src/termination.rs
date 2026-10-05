//! Termination signals while a run streams to an event service.
//!
//! A supervisor such as a CI runner stops a job with SIGINT or SIGTERM and
//! escalates to SIGKILL after a grace period. Without a handler the process
//! dies on the first signal and the event service never learns the run
//! ended. [`SignalWatcher`] catches the first signal so the live reporter can
//! publish a cancelled completion, then [`terminate`] ends the process the
//! way the default disposition would have.
//!
//! The handler is installed once per process, and only for signals whose
//! inherited disposition is the default, so a signal the parent asked Once to
//! ignore stays ignored. Outside a streaming run, and for any signal after the
//! first, the handler performs the default action itself, so those signals
//! behave exactly as if no handler were installed. A process that streams
//! several runs in sequence catches the first signal of each. The same signal
//! repeated within a quarter of a second counts as the same request, so a
//! terminal interrupt that reaches a whole process group and is also forwarded
//! by a parent `once` does not cut the drain short.

/// A termination signal Once reacts to while streaming.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Termination {
    Interrupt,
    Terminate,
}

impl Termination {
    pub fn name(self) -> &'static str {
        match self {
            Self::Interrupt => "SIGINT",
            Self::Terminate => "SIGTERM",
        }
    }

    pub fn exit_code(self) -> i32 {
        match self {
            Self::Interrupt => 130,
            Self::Terminate => 143,
        }
    }

    #[cfg(unix)]
    fn raw(self) -> libc::c_int {
        match self {
            Self::Interrupt => libc::SIGINT,
            Self::Terminate => libc::SIGTERM,
        }
    }
}

#[cfg(unix)]
pub use unix::{forward, SignalWatcher};

#[cfg(not(unix))]
pub use fallback::{forward, SignalWatcher};

/// End the process as `signal` would have: restore its default disposition and
/// deliver it to the process, so the parent observes a death by signal. Falls
/// back to the conventional `128 + n` exit status.
pub fn terminate(signal: Termination) -> ! {
    #[cfg(unix)]
    unix::reraise(signal);
    std::process::exit(signal.exit_code())
}

// Installing the signal handler and reading a signal's inherited disposition
// go through `sigaction`, which has no safe wrapper. Scoped here so the rest of the crate stays under the
// workspace-wide `unsafe_code = "deny"`.
#[cfg(unix)]
#[allow(unsafe_code)]
mod unix {
    use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
    use std::sync::OnceLock;
    use std::time::Duration;

    use super::Termination;

    /// How often a watcher looks at the state the handler sets.
    const POLL_INTERVAL: Duration = Duration::from_millis(25);

    /// No streaming run is watching: signals take their default action.
    const IDLE: u8 = 0;
    /// A streaming run is watching and has not been signalled yet.
    const WATCHING: u8 = 1;
    /// The run caught its first signal; later signals take the default action.
    const CAUGHT_INTERRUPT: u8 = 2;
    const CAUGHT_TERMINATE: u8 = 3;

    /// The whole decision lives in one atomic so the handler and the watcher
    /// agree on who owns each delivery: the handler either moves the state
    /// from `WATCHING` to a caught state, or takes the default action.
    static STATE: AtomicU8 = AtomicU8::new(IDLE);

    /// When the run caught its first signal, in monotonic nanoseconds; zero
    /// until then.
    static FIRST_CAUGHT_NS: AtomicU64 = AtomicU64::new(0);

    /// A repeat of the first signal this soon after it is the same request
    /// arriving twice, for example a terminal interrupt that reached the whole
    /// process group and was also forwarded by a parent `once`. A different
    /// signal is never treated as a repeat.
    const REPEAT_WINDOW_NS: u64 = 250_000_000;

    /// Whether the handler is installed for at least one signal.
    static INSTALLED: OnceLock<bool> = OnceLock::new();

    fn caught_from(state: u8) -> Option<Termination> {
        match state {
            CAUGHT_INTERRUPT => Some(Termination::Interrupt),
            CAUGHT_TERMINATE => Some(Termination::Terminate),
            _ => None,
        }
    }

    fn installed() -> bool {
        *INSTALLED.get_or_init(|| {
            let interrupt = install_handler(Termination::Interrupt);
            let terminate = install_handler(Termination::Terminate);
            interrupt || terminate
        })
    }

    fn install_handler(termination: Termination) -> bool {
        let raw = termination.raw();
        let Some(previous) = current_action(raw) else {
            return false;
        };
        if previous.sa_sigaction != libc::SIG_DFL {
            tracing::debug!(
                signal = termination.name(),
                "signal disposition is not the default; leaving it alone"
            );
            return false;
        }
        // One `sigaction` call, so the handler is complete from the moment it
        // is installed and no delivery can fall between partial registrations.
        // SAFETY: `sigaction` is plain old data; all-zero is a valid value.
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = handler as extern "C" fn(libc::c_int) as libc::sighandler_t;
        action.sa_flags = libc::SA_RESTART;
        // SAFETY: `action` is fully initialised with an empty mask and a
        // handler that only uses async-signal-safe operations.
        let status = unsafe {
            libc::sigemptyset(&raw mut action.sa_mask);
            libc::sigaction(raw, &raw const action, std::ptr::null_mut())
        };
        if status != 0 {
            tracing::debug!(
                signal = termination.name(),
                error = %std::io::Error::last_os_error(),
                "could not watch termination signal"
            );
            return false;
        }
        true
    }

    /// Claims the first signal for the watching run, ignores a repeat of it
    /// inside the repeat window, and otherwise takes the default action. Uses only
    /// atomics, `clock_gettime`, and `emulate_default_handler`, which
    /// `signal-hook` itself runs from signal handlers; all are
    /// async-signal-safe.
    extern "C" fn handler(raw: libc::c_int) {
        let caught = match raw {
            libc::SIGINT => CAUGHT_INTERRUPT,
            _ => CAUGHT_TERMINATE,
        };
        let now = monotonic_ns();
        match STATE.compare_exchange(WATCHING, caught, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => FIRST_CAUGHT_NS.store(now.max(1), Ordering::SeqCst),
            Err(state) if state == caught && is_repeat(now) => {}
            Err(_) => {
                let _ = signal_hook::low_level::emulate_default_handler(raw);
            }
        }
    }

    fn is_repeat(now: u64) -> bool {
        // Zero means the first delivery is still inside the handler.
        let first = FIRST_CAUGHT_NS.load(Ordering::SeqCst);
        first == 0 || now.saturating_sub(first) < REPEAT_WINDOW_NS
    }

    fn monotonic_ns() -> u64 {
        let mut now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `now` is a valid, writable `timespec`.
        unsafe {
            libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut now);
        }
        u64::try_from(now.tv_sec)
            .unwrap_or_default()
            .saturating_mul(1_000_000_000)
            .saturating_add(u64::try_from(now.tv_nsec).unwrap_or_default())
    }

    /// Deliver `termination` to process `pid`, a child this process started.
    pub fn forward(pid: u32, termination: Termination) {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return;
        };
        // SAFETY: `kill` with a positive pid only signals that process.
        unsafe {
            libc::kill(pid, termination.raw());
        }
    }

    /// Catches the first SIGINT or SIGTERM while a run streams.
    pub struct SignalWatcher(());

    impl SignalWatcher {
        /// Start catching termination signals for a streaming run. Returns
        /// `None` when neither signal can be caught, for example because the
        /// parent ignored both.
        pub fn install() -> Option<Self> {
            if !installed() {
                return None;
            }
            FIRST_CAUGHT_NS.store(0, Ordering::SeqCst);
            STATE.store(WATCHING, Ordering::SeqCst);
            Some(Self(()))
        }

        /// The first signal caught during this run.
        pub async fn recv(&mut self) -> Termination {
            loop {
                if let Some(termination) = caught_from(STATE.load(Ordering::SeqCst)) {
                    return termination;
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }

        /// Stop catching signals. Returns a signal caught before that, which
        /// the caller should act on instead of losing it; any later signal
        /// takes its default action.
        pub fn restore(self) -> Option<Termination> {
            let caught = caught_from(STATE.swap(IDLE, Ordering::SeqCst));
            std::mem::forget(self);
            caught
        }
    }

    impl Drop for SignalWatcher {
        /// Unwinding out of a run must not swallow a signal it caught.
        fn drop(&mut self) {
            if let Some(termination) = caught_from(STATE.swap(IDLE, Ordering::SeqCst)) {
                super::terminate(termination);
            }
        }
    }

    fn current_action(raw: libc::c_int) -> Option<libc::sigaction> {
        // SAFETY: `sigaction` is plain old data; all-zero is a valid value.
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        // SAFETY: a null new action only reads the current disposition into
        // `action`, which is a valid, writable `sigaction`.
        let status = unsafe { libc::sigaction(raw, std::ptr::null(), &raw mut action) };
        (status == 0).then_some(action)
    }

    pub(super) fn reraise(termination: Termination) {
        // Restores the default disposition, unblocks the signal, and raises it.
        if let Err(error) = signal_hook::low_level::emulate_default_handler(termination.raw()) {
            tracing::debug!(%error, signal = termination.name(), "could not re-raise signal");
        }
    }
}

#[cfg(not(unix))]
mod fallback {
    use super::Termination;

    /// Signals are not watched on this platform; runs keep the default
    /// behavior.
    pub struct SignalWatcher;

    pub fn forward(_pid: u32, _termination: Termination) {}

    impl SignalWatcher {
        pub fn install() -> Option<Self> {
            None
        }

        pub async fn recv(&mut self) -> Termination {
            std::future::pending().await
        }

        pub fn restore(self) -> Option<Termination> {
            None
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use super::{terminate, SignalWatcher};

    const CHILD_MODE: &str = "ONCE_TERMINATION_TEST_CHILD";

    // Signals reach the whole process, so each scenario runs in a re-executed
    // copy of this test binary instead of the shared test harness.
    #[test]
    #[ignore = "run by the scenarios below in a child process"]
    fn child() {
        let Ok(mode) = std::env::var(CHILD_MODE) else {
            return;
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let first = SignalWatcher::install().unwrap();
            assert!(first.restore().is_none());
            let ready = |line: &str| {
                println!("{line}");
                std::io::stdout().flush().unwrap();
            };
            if mode == "second_run" {
                let mut second = SignalWatcher::install().unwrap();
                ready("ready");
                let signal = second.recv().await;
                ready(signal.name());
                terminate(signal);
            }
            ready("ready");
            tokio::time::sleep(Duration::from_secs(10)).await;
            ready("survived");
        });
    }

    fn run_child(mode: &str) -> (Vec<String>, std::process::ExitStatus) {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "termination::tests::child",
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD_MODE, mode)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        for line in lines.by_ref() {
            if line.unwrap() == "ready" {
                break;
            }
        }
        let status = Command::new("kill")
            .args(["-s", "TERM", &child.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
        let rest = lines.map(Result::unwrap).collect();
        (rest, child.wait().unwrap())
    }

    #[test]
    fn a_later_run_in_the_same_process_still_catches_the_signal() {
        let (output, status) = run_child("second_run");
        assert!(output.iter().any(|line| line == "SIGTERM"), "{output:?}");
        assert_eq!(status.signal(), Some(libc::SIGTERM));
    }

    #[test]
    fn signals_between_runs_keep_their_default_behavior() {
        let (output, status) = run_child("idle");
        assert!(!output.iter().any(|line| line == "survived"), "{output:?}");
        assert_eq!(status.signal(), Some(libc::SIGTERM));
    }
}
