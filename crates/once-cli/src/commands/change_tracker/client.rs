use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
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
    match request_snapshot(socket, outputs, since).await {
        Ok(snapshot) => return Some(snapshot),
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
    if let Err(error) = spawn_tracker(workspace, socket) {
        tracing::debug!(%error, "failed to start filesystem change tracker");
        return None;
    }
    let mut last_error = None;
    for _ in 0..50 {
        match request_snapshot(socket, outputs, since).await {
            Ok(snapshot) => return Some(snapshot),
            Err(error) => last_error = Some(error),
        }
        sleep(Duration::from_millis(10)).await;
    }
    if let Some(error) = last_error {
        tracing::debug!(%error, "filesystem change tracker did not become ready");
    } else {
        tracing::debug!("filesystem change tracker did not become ready");
    }
    None
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
    /// True when the tracker simply is not listening yet, which is the
    /// normal state before the daemon has been spawned.
    fn is_unreachable(&self) -> bool {
        matches!(self, Self::Connect { .. })
    }
}

pub(super) async fn request_snapshot(
    socket: &Path,
    outputs: &[String],
    since: Option<&ChangePosition>,
) -> std::result::Result<ChangeSnapshot, SnapshotError> {
    let stream = UnixStream::connect(socket)
        .await
        .map_err(|source| SnapshotError::Connect {
            socket: socket.to_path_buf(),
            source,
        })?;
    let (read, mut write) = stream.into_split();
    let mut raw = serde_json::to_vec(&Request {
        command: "barrier".to_string(),
        outputs: outputs.to_vec(),
        since: since.cloned(),
    })
    // The request is built from owned local data, so the only way
    // serialisation fails is a bug in this function.
    .expect("barrier request is serializable");
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

fn spawn_tracker(workspace: &Path, socket: &Path) -> std::io::Result<()> {
    let launcher = tracker_launcher(socket)?;
    std::process::Command::new(&launcher)
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
}
