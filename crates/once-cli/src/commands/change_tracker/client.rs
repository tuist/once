use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use once_cas::Digest;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::sleep;

use super::protocol::{Request, Response};
use super::{ChangePosition, ChangeSnapshot};

/// Read timeout for one barrier round trip.
///
/// Derived from the server's fence wait rather than set independently:
/// the fence is the slow part, and a client that gave up first would
/// report a bare timeout in place of the server's specific reason.
fn response_timeout() -> Duration {
    super::server::fence_timeout() + Duration::from_secs(3)
}

/// Take a barrier snapshot, starting the tracker daemon if it is not
/// running yet.
///
/// Returns `None` when no snapshot could be obtained. Every cause is
/// logged with its reason, because callers treat a missing snapshot as
/// "fall back to full validation" and must not fail the build over it.
pub(super) async fn snapshot(
    workspace: &Path,
    socket: &Path,
    outputs: &[String],
    since: Option<&ChangePosition>,
) -> Option<ChangeSnapshot> {
    snapshot_with(workspace, socket, outputs, since, "barrier").await
}

/// Where the journal stands, without waiting for the platform to report writes
/// this process just made.
///
/// Waiting for those is the expensive part of a barrier: after a compiler has
/// written its output the platform's notification service takes hundreds of
/// milliseconds to deliver the burst, and the barrier sits behind it. A caller
/// that produced those writes does not need to be told about them, so long as
/// whatever reads the resulting position can recognise them later.
pub(super) async fn position(
    workspace: &Path,
    socket: &Path,
    outputs: &[String],
    since: Option<&ChangePosition>,
) -> Option<ChangeSnapshot> {
    snapshot_with(workspace, socket, outputs, since, "position").await
}

async fn snapshot_with(
    workspace: &Path,
    socket: &Path,
    outputs: &[String],
    since: Option<&ChangePosition>,
    command: &str,
) -> Option<ChangeSnapshot> {
    let first = request(socket, outputs, since, command).await;
    let first = match first {
        Err(error) if error.is_unknown_command() => {
            tracing::debug!(
                command,
                "filesystem change tracker predates this request; falling back to a barrier"
            );
            request(socket, outputs, since, "barrier").await
        }
        other => other,
    };
    match first {
        Ok(snapshot) => {
            clear_startup_failure(socket);
            return Some(snapshot);
        }
        // Not being able to reach the socket is the expected first-run
        // path, so it stays at trace level; everything else is a real
        // surprise worth seeing at debug.
        Err(error) if error.is_unreachable() => {
            tracing::trace!(%error, "filesystem change tracker not running yet");
        }
        Err(error) => {
            tracing::debug!(%error, "filesystem change tracker snapshot failed");
        }
    }
    // A build asks for a barrier twice, once before the work and once after.
    // When the first attempt could not get the tracker up, the second cannot
    // either, and paying the startup wait again doubles a cost the build
    // already decided to do without.
    if startup_already_failed() {
        tracing::trace!("skipping tracker startup that already failed in this process");
        return None;
    }
    // A startup that failed recently is not worth repeating on every
    // invocation. Without a cooldown each build launches another daemon and
    // waits for it, so a host whose change notification service is unhealthy
    // pays the wait forever and accumulates processes; with one, the cost is
    // a single slow build per cooldown window.
    if let Some(remaining) = startup_cooldown_remaining(socket) {
        tracing::debug!(
            ?remaining,
            "skipping filesystem change tracker startup after a recent failure"
        );
        // Only the in-process flag. Rewriting the marker would reset its
        // timestamp, so a workspace built more often than the cooldown window
        // would push the window forward forever and never retry the tracker
        // even after the watch service recovered.
        STARTUP_FAILED.store(true, Ordering::Relaxed);
        return None;
    }
    if let Err(error) = spawn_tracker(workspace, socket) {
        tracing::debug!(%error, "failed to start filesystem change tracker");
        record_startup_failure(socket);
        return None;
    }
    let mut last_error = None;
    for _ in 0..50 {
        match request(socket, outputs, since, command).await {
            Ok(snapshot) => {
                clear_startup_failure(socket);
                return Some(snapshot);
            }
            // Keep polling only while the daemon is not accepting
            // connections yet, since that is the only state a retry can
            // clear. Any other outcome means the daemon answered, or its
            // own bounded barrier wait elapsed; re-issuing a full barrier
            // would multiply that per-request timeout by the remaining
            // iterations and stall the build for minutes. Fall back to
            // full validation instead.
            Err(error) if error.is_unreachable() => last_error = Some(error),
            Err(error) => {
                last_error = Some(error);
                break;
            }
        }
        sleep(Duration::from_millis(10)).await;
    }
    if let Some(error) = last_error {
        tracing::debug!(%error, "filesystem change tracker did not become ready");
    } else {
        tracing::debug!("filesystem change tracker did not become ready");
    }
    record_startup_failure(socket);
    None
}

/// Whether starting the tracker already failed in this process.
static STARTUP_FAILED: AtomicBool = AtomicBool::new(false);

/// How long to leave the tracker alone after a startup attempt failed.
///
/// Short enough that a machine which recovers picks the tracker back up
/// within a build or two, long enough that a machine which does not stops
/// paying the startup wait on every invocation.
const STARTUP_COOLDOWN: Duration = Duration::from_mins(1);

fn startup_already_failed() -> bool {
    STARTUP_FAILED.load(Ordering::Relaxed)
}

fn startup_cooldown_path(socket: &Path) -> PathBuf {
    socket.with_extension("startup-failed")
}

/// How much of the cooldown from a previous failed startup is left, or `None`
/// when the tracker is free to be started again.
fn startup_cooldown_remaining(socket: &Path) -> Option<Duration> {
    let modified = std::fs::metadata(startup_cooldown_path(socket))
        .ok()?
        .modified()
        .ok()?;
    let elapsed = modified.elapsed().ok()?;
    STARTUP_COOLDOWN.checked_sub(elapsed)
}

fn record_startup_failure(socket: &Path) {
    STARTUP_FAILED.store(true, Ordering::Relaxed);
    let path = startup_cooldown_path(socket);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // The marker's timestamp is the whole payload, so an empty file is
    // enough and a failure to write it only costs the next invocation a
    // retry.
    let _ = std::fs::write(&path, b"");
}

/// Forget a past failure once the tracker answers again.
fn clear_startup_failure(socket: &Path) {
    let _ = std::fs::remove_file(startup_cooldown_path(socket));
}

/// Why a barrier request could not be answered.
///
/// Kept as a typed error rather than a bare `Option` so a failed
/// snapshot reports its reason. The server already sends a precise
/// message for a rejected barrier; collapsing that into `None` left
/// callers and tests with no way to tell a cold socket apart from a
/// fence timeout.
#[derive(Debug, thiserror::Error)]
pub(super) enum SnapshotError {
    #[error("connecting to tracker socket {socket}: {source}")]
    Connect {
        socket: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("sending barrier request: {0}")]
    Send(#[source] std::io::Error),
    #[error("reading barrier response: {0}")]
    Receive(#[source] std::io::Error),
    #[error("tracker did not answer within {0:?}")]
    ResponseTimeout(Duration),
    #[error("decoding barrier response: {0}")]
    Decode(#[source] serde_json::Error),
    #[error("tracker rejected the barrier: {0}")]
    Rejected(String),
    #[error("tracker answered with neither a snapshot nor an error")]
    Empty,
}

impl SnapshotError {
    /// True when a daemon that predates this executable does not recognise the
    /// request.
    ///
    /// The daemon outlives the invocation that started it, so upgrading Once
    /// leaves the old one running until the working copy goes quiet. A request
    /// it has never heard of is not a failure, it is an older peer, and the
    /// caller falls back to a request every version understands.
    fn is_unknown_command(&self) -> bool {
        matches!(self, Self::Rejected(message) if message.contains("unknown tracker command"))
    }

    /// True when the tracker simply is not listening yet, which is the
    /// normal state before the daemon has been spawned.
    fn is_unreachable(&self) -> bool {
        matches!(self, Self::Connect { .. })
    }
}

#[cfg(test)]
pub(super) async fn request_snapshot(
    socket: &Path,
    outputs: &[String],
    since: Option<&ChangePosition>,
) -> std::result::Result<ChangeSnapshot, SnapshotError> {
    request(socket, outputs, since, "barrier").await
}

async fn request(
    socket: &Path,
    outputs: &[String],
    since: Option<&ChangePosition>,
    command: &str,
) -> std::result::Result<ChangeSnapshot, SnapshotError> {
    let stream = UnixStream::connect(socket)
        .await
        .map_err(|source| SnapshotError::Connect {
            socket: socket.to_path_buf(),
            source,
        })?;
    let (read, mut write) = stream.into_split();
    let mut raw = serde_json::to_vec(&Request {
        command: command.to_string(),
        outputs: outputs.to_vec(),
        since: since.cloned(),
    })
    // The request is built from owned local data, so the only way
    // serialisation fails is a bug in this function.
    .expect("tracker request is serializable");
    raw.push(b'\n');
    write.write_all(&raw).await.map_err(SnapshotError::Send)?;
    write.shutdown().await.map_err(SnapshotError::Send)?;
    let mut line = String::new();
    let timeout = response_timeout();
    tokio::time::timeout(timeout, BufReader::new(read).read_line(&mut line))
        .await
        .map_err(|_| SnapshotError::ResponseTimeout(timeout))?
        .map_err(SnapshotError::Receive)?;
    let response = serde_json::from_str::<Response>(&line).map_err(SnapshotError::Decode)?;
    if let Some(error) = response.error {
        return Err(SnapshotError::Rejected(error));
    }
    response.snapshot.ok_or(SnapshotError::Empty)
}

/// Shell prologue that closes every descriptor above stderr and then replaces
/// itself with the daemon.
///
/// The ceiling comes from the process's own descriptor limit rather than a
/// fixed number, because a pipe can sit above any guess and one left open is
/// enough to hang a harness. It is capped so a host with an enormous limit
/// does not spend the daemon's startup closing descriptors that cannot exist.
///
/// The standard library marks the descriptors it opens close-on-exec, but one
/// this process inherited from its own parent carries no such mark, and the
/// daemon is meant to outlive the build that started it. A harness that reads
/// the build's output through a pipe would then wait forever for an end of
/// file the daemon still holds open: `shellspec` finished every example and
/// hung before its summary, because the daemon had inherited two duplicates of
/// the harness's output file. Closing the range needs `close` on each
/// descriptor, which the standard library does not expose without unsafe code,
/// so the shell does it instead.
const DETACH_AND_EXEC: &str = r#"limit=$(ulimit -n 2>/dev/null || echo 256)
case "$limit" in
  unlimited|*[!0-9]*) limit=4096 ;;
esac
if [ "$limit" -gt 4096 ]; then limit=4096; fi
fd=3
while [ "$fd" -lt "$limit" ]; do
  eval "exec $fd>&-" 2>/dev/null || true
  fd=$((fd + 1))
done
exec "$@"
"#;

fn spawn_tracker(workspace: &Path, socket: &Path) -> std::io::Result<()> {
    let launcher = tracker_launcher(socket)?;
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(DETACH_AND_EXEC)
        // `sh -c script name args...` binds `name` to `$0`, so the daemon and
        // its arguments start at `$1` where `exec "$@"` picks them up.
        .arg("once-change-tracker")
        .arg(&launcher)
        .arg("-C")
        .arg(workspace)
        .arg("__change-tracker")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

/// Return a private copy of the running executable to launch the long-lived
/// tracker from. The daemon keeps its executable file open for its whole
/// lifetime, and on Linux overwriting a running executable fails with
/// `ETXTBSY`. Launching from a copy under the runtime directory keeps the
/// user-facing `once` binary free to be rebuilt, replaced, or upgraded in
/// place while a tracker is running.
fn tracker_launcher(socket: &Path) -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let directory = socket.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(directory)?;
    // Content-address the copy by a cheap fingerprint of the executable so a
    // new `once` build lands at a new path instead of overwriting a copy a
    // running daemon still holds open.
    let launcher = directory.join(format!("tracker-{}", launcher_fingerprint(&exe)));
    if launcher.exists() {
        return Ok(launcher);
    }
    let temp = launcher.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::copy(&exe, &temp)?;
    let mut permissions = std::fs::metadata(&temp)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&temp, permissions)?;
    match std::fs::rename(&temp, &launcher) {
        Ok(()) => Ok(launcher),
        // Lost a race against another invocation that created the same copy.
        Err(_) if launcher.exists() => {
            let _ = std::fs::remove_file(&temp);
            Ok(launcher)
        }
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error)
        }
    }
}

fn launcher_fingerprint(exe: &Path) -> String {
    let metadata = std::fs::metadata(exe).ok();
    let length = metadata.as_ref().map_or(0, std::fs::Metadata::len);
    let modified = metadata
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_nanos());
    Digest::of_bytes(format!("{length}:{modified}").as_bytes()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_is_a_private_copy_under_the_runtime_directory() {
        let directory = tempfile::TempDir::new().unwrap();
        let socket = directory.path().join("tracker.sock");
        let launcher = tracker_launcher(&socket).unwrap();

        // The daemon must launch from a copy under the runtime directory, not
        // from the running executable, so the user-facing binary is never held
        // open (which would fail an in-place overwrite with ETXTBSY on Linux).
        assert_eq!(launcher.parent(), Some(directory.path()));
        assert_ne!(launcher, std::env::current_exe().unwrap());
        assert!(launcher.is_file());
        assert_ne!(
            std::fs::metadata(&launcher).unwrap().permissions().mode() & 0o111,
            0,
            "launcher copy must be executable"
        );

        // A second call for the same executable reuses the same copy.
        assert_eq!(tracker_launcher(&socket).unwrap(), launcher);
    }

    // A daemon that is reachable but keeps rejecting the barrier stands in
    // for a workspace whose filesystem events never reach the watcher. The
    // client must fall back to full validation after a single answered
    // barrier rather than re-issuing it once per retry iteration, which
    // would multiply the per-request timeout into a multi-minute stall.
    #[tokio::test]
    async fn snapshot_stops_polling_once_the_daemon_answers() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        use tokio::net::UnixListener;

        let directory = tempfile::TempDir::new().unwrap();
        let socket = directory.path().join("tracker.sock");
        let listener = UnixListener::bind(&socket).unwrap();

        let barriers = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&barriers);
        let server = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                let _ = BufReader::new(read).read_line(&mut line).await;
                let _ = write.write_all(b"{\"error\":\"barrier rejected\"}\n").await;
                let _ = write.shutdown().await;
            }
        });

        let outcome = snapshot(directory.path(), &socket, &[], None).await;
        server.abort();

        assert!(
            outcome.is_none(),
            "a rejected barrier must fall back to full validation"
        );
        let observed = barriers.load(Ordering::SeqCst);
        assert!(
            observed <= 5,
            "client re-issued the barrier {observed} times instead of falling back once the daemon answered"
        );
    }
}
