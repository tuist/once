use std::future::Future;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(super) async fn timeout_after_ready(
    root: &Path,
    execution: impl Future<Output = crate::Result<once_cas::ActionResult>>,
) {
    // Exclude fixture initialization from the ownership scenario, then let the
    // unchanged public four-second deadline expire against a real running command.
    tokio::time::pause();
    let mut execution = Box::pin(execution);
    let ready = tokio::select! {
        result = &mut execution => Err(format!("public execution completed before readiness: {result:?}")),
        ready = ready(root) => ready,
    };
    tokio::time::resume();
    let setup = ready.unwrap();
    let started = Instant::now();
    let result = execution.await;
    assert!(
        matches!(result, Err(crate::Error::Timeout(bound)) if bound == Duration::from_secs(4)),
        "{result:?}"
    );
    eprintln!(
        "ready after {setup:?} of setup; public four-second timeout after {:?} of real time",
        started.elapsed()
    );
}

async fn ready(root: &Path) -> Result<Duration, String> {
    let root = root.to_owned();
    // A blocking observer inhibits Tokio's paused-clock auto-advance while real
    // Docker I/O proceeds. Both setup and each observation use the wall deadline.
    tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        let bound = Duration::from_mins(1);
        while started.elapsed() < bound {
            if let Ok(id) = std::fs::read_to_string(root.join("container-id")) {
                let mut command = Command::new(std::env::var_os("ONCE_DOCKER_REAL_BIN").unwrap());
                for key in [
                    "DOCKER_HOST",
                    "DOCKER_CONTEXT",
                    "DOCKER_TLS",
                    "DOCKER_TLS_VERIFY",
                    "DOCKER_CERT_PATH",
                ] {
                    command.env_remove(key);
                }
                let mut child = command
                    .arg("--config")
                    .arg(root.join("config"))
                    .args([
                        "--context",
                        "original",
                        "exec",
                        id.trim(),
                        "test",
                        "-f",
                        "/tmp/once-context-ready",
                    ])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|error| error.to_string())?;
                loop {
                    if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
                        if status.success() {
                            return Ok(started.elapsed());
                        }
                        break;
                    }
                    if started.elapsed() >= bound {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(
                            "Docker readiness observation exceeded the 60-second setup bound"
                                .into(),
                        );
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        Err("real Docker command did not become ready within 60 seconds of setup".into())
    })
    .await
    .map_err(|error| error.to_string())?
}
