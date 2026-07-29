use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

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
    if let Err(error) = spawn_tracker(workspace) {
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

fn spawn_tracker(workspace: &Path) -> std::io::Result<()> {
    std::process::Command::new(std::env::current_exe()?)
        .arg("-C")
        .arg(workspace)
        .arg("__change-tracker")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}
