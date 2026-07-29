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

pub(super) async fn snapshot(
    workspace: &Path,
    socket: &Path,
    outputs: &[String],
    since: Option<&ChangePosition>,
) -> Option<ChangeSnapshot> {
    if let Some(snapshot) = request_snapshot(socket, outputs, since).await {
        return Some(snapshot);
    }
    if let Err(error) = spawn_tracker(workspace, socket) {
        tracing::debug!(%error, "failed to start filesystem change tracker");
        return None;
    }
    for _ in 0..50 {
        if let Some(snapshot) = request_snapshot(socket, outputs, since).await {
            return Some(snapshot);
        }
        sleep(Duration::from_millis(10)).await;
    }
    tracing::debug!("filesystem change tracker did not become ready");
    None
}

pub(super) async fn request_snapshot(
    socket: &Path,
    outputs: &[String],
    since: Option<&ChangePosition>,
) -> Option<ChangeSnapshot> {
    let stream = UnixStream::connect(socket).await.ok()?;
    let (read, mut write) = stream.into_split();
    let raw = serde_json::to_vec(&Request {
        command: "barrier".to_string(),
        outputs: outputs.to_vec(),
        since: since.cloned(),
    })
    .ok()?;
    write.write_all(&raw).await.ok()?;
    write.write_all(b"\n").await.ok()?;
    write.shutdown().await.ok()?;
    let mut line = String::new();
    tokio::time::timeout(
        Duration::from_secs(3),
        BufReader::new(read).read_line(&mut line),
    )
    .await
    .ok()?
    .ok()?;
    let response = serde_json::from_str::<Response>(&line).ok()?;
    if let Some(error) = response.error {
        tracing::debug!(%error, "filesystem change tracker rejected snapshot");
        return None;
    }
    response.snapshot
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
