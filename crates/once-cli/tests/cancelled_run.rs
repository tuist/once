//! A streaming `once` process that receives SIGINT or SIGTERM reports the run
//! as cancelled before it exits, the way a CI runner cancelling a superseded
//! job expects. Spawns the real binary against an in-process discovery
//! document and event service.
#![cfg(unix)]

use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::pin::Pin;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use futures::{SinkExt, Stream};
use once_events_client::proto::{
    run_event::Payload,
    run_event_service_server::{RunEventService, RunEventServiceServer},
    AckDisposition, ArgvHashKey, BatchAck, GetArgvHashKeyRequest, GetRunAckRequest,
    GetServerCapabilitiesRequest, RunCompleted, RunEventAck, RunEventBatch, RunFinalization,
    RunResult, ServerCapabilities,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tonic::{Request, Response, Status, Streaming};

#[derive(Default)]
struct Recorded {
    payloads: Vec<Payload>,
    /// Stop acknowledging batches once the run has started.
    stall_after_start: bool,
}

#[derive(Clone, Default)]
struct Collector {
    recorded: Arc<Mutex<Recorded>>,
}

impl Collector {
    fn started(&self) -> bool {
        self.recorded
            .lock()
            .unwrap()
            .payloads
            .iter()
            .any(|payload| matches!(payload, Payload::RunStarted(_)))
    }

    fn completions(&self) -> Vec<RunCompleted> {
        self.recorded
            .lock()
            .unwrap()
            .payloads
            .iter()
            .filter_map(|payload| match payload {
                Payload::RunCompleted(completed) => Some(completed.clone()),
                _ => None,
            })
            .collect()
    }
}

#[tonic::async_trait]
impl RunEventService for Collector {
    async fn get_server_capabilities(
        &self,
        _: Request<GetServerCapabilitiesRequest>,
    ) -> Result<Response<ServerCapabilities>, Status> {
        Ok(Response::new(ServerCapabilities {
            supported_protocol_versions: vec!["1.0".into()],
            max_batch_bytes: 65_536,
            max_event_bytes: 8_192,
            max_unacked_events: 4_096,
            max_log_chunk_bytes: 16_384,
            required_features: vec![],
            log_ingestion_available: true,
            raw_event_retention_available: true,
            finalization_grace_ms: 2_000,
            dedup_retention_seconds: 86_400,
            safe_literal_allowlist_version: "2026.09.03-v1".into(),
        }))
    }

    async fn get_argv_hash_key(
        &self,
        _: Request<GetArgvHashKeyRequest>,
    ) -> Result<Response<ArgvHashKey>, Status> {
        Ok(Response::new(ArgvHashKey {
            key_id: "test-key".into(),
            key_bytes: vec![0; 32],
            expires_at_epoch_ms: i64::MAX,
            grace_after_expiry_ms: 60_000,
        }))
    }

    type PublishRunEventsStream =
        Pin<Box<dyn Stream<Item = Result<BatchAck, Status>> + Send + 'static>>;

    async fn publish_run_events(
        &self,
        request: Request<Streaming<RunEventBatch>>,
    ) -> Result<Response<Self::PublishRunEventsStream>, Status> {
        let mut inbound = request.into_inner();
        let recorded = self.recorded.clone();
        let (mut tx, rx) = futures::channel::mpsc::channel(32);
        tokio::spawn(async move {
            while let Ok(Some(batch)) = inbound.message().await {
                let stall = {
                    let mut state = recorded.lock().unwrap();
                    let stall = state.stall_after_start && !state.payloads.is_empty();
                    state.payloads.extend(
                        batch
                            .events
                            .iter()
                            .filter_map(|event| event.payload.clone()),
                    );
                    stall
                };
                if stall {
                    continue;
                }
                let acked_seq = batch
                    .events
                    .last()
                    .map_or(batch.seq_from.saturating_sub(1), |event| event.seq);
                let ack = BatchAck {
                    run_id: batch.run_id,
                    batch_id: batch.batch_id,
                    disposition: AckDisposition::Accepted as i32,
                    acked_seq,
                    expected_next_seq: acked_seq + 1,
                    observed_high_water_seq: 0,
                    retry_after_ms: 0,
                    max_in_flight_batches: 0,
                    finalization: RunFinalization::Active as i32,
                    dashboard_url: String::new(),
                };
                if tx.send(Ok(ack)).await.is_err() {
                    return;
                }
            }
        });
        Ok(Response::new(Box::pin(rx)))
    }

    async fn get_run_ack(
        &self,
        request: Request<GetRunAckRequest>,
    ) -> Result<Response<RunEventAck>, Status> {
        Ok(Response::new(RunEventAck {
            run_id: request.into_inner().run_id,
            acked_seq: 0,
            expected_next_seq: 1,
            observed_high_water_seq: 0,
            finalization: RunFinalization::Active as i32,
            dashboard_url: String::new(),
        }))
    }
}

/// Serve the event service and a discovery document pointing at it. Returns
/// the base URL to configure as the provider.
async fn serve(collector: Collector) -> String {
    let grpc = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let grpc_port = grpc.local_addr().unwrap().port();
    let incoming = futures::stream::unfold(grpc, |listener| async move {
        let accepted = listener.accept().await.map(|(socket, _)| socket);
        Some((accepted, listener))
    });
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(RunEventServiceServer::new(collector))
            .serve_with_incoming(incoming),
    );

    let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http_port = http.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = http.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut request = vec![0_u8; 4096];
                let read = socket.read(&mut request).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..read]);
                let response = if request.starts_with("GET /.well-known/once ") {
                    let body = format!("{{\"events\":[\"grpc://127.0.0.1:{grpc_port}\"]}}");
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    )
                } else {
                    // Unavailable reads as an unreachable cache, which is a
                    // miss, so the command still runs.
                    "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                        .to_string()
                };
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    format!("http://127.0.0.1:{http_port}")
}

fn workspace(provider_url: Option<&str>) -> tempfile::TempDir {
    let workspace = tempfile::tempdir().unwrap();
    let manifest = match provider_url {
        Some(url) => format!(
            "[infrastructure.cache]\nprovider = \"tuist\"\n\n[infrastructures.tuist]\nkind = \"tuist\"\nurl = \"{url}\"\naccount = \"acme\"\nproject = \"app\"\n"
        ),
        None => String::new(),
    };
    std::fs::write(workspace.path().join("once.toml"), manifest).unwrap();
    workspace
}

/// A spawned `once` whose stderr is collected as it is written, so a test can
/// wait for something the process reports before acting.
struct OnceProcess {
    child: Child,
    stderr: Arc<Mutex<String>>,
    reader: JoinHandle<()>,
}

impl OnceProcess {
    fn stderr(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }
}

fn spawn_once(workspace: &Path, home: &Path) -> OnceProcess {
    spawn_once_with(workspace, home, Command::new(env!("CARGO_BIN_EXE_once")))
}

fn spawn_once_with(workspace: &Path, home: &Path, mut command: Command) -> OnceProcess {
    let mut child = command
        .args(["exec", "--", "sleep", "60"])
        .current_dir(workspace)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("TUIST_TOKEN", "test-token")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stderr.take().unwrap();
    let stderr = Arc::new(Mutex::new(String::new()));
    let sink = stderr.clone();
    let reader = std::thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        while let Ok(read) = std::io::Read::read(&mut pipe, &mut buffer) {
            if read == 0 {
                return;
            }
            sink.lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&buffer[..read]));
        }
    });
    OnceProcess {
        child,
        stderr,
        reader,
    }
}

fn send(process: &OnceProcess, signal: &str) {
    let status = Command::new("kill")
        .args(["-s", signal, &process.child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

async fn wait_until(deadline: Duration, mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(started.elapsed() < deadline, "condition not met in time");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_for_exit(
    mut process: OnceProcess,
    deadline: Duration,
) -> (ExitStatus, Duration, String) {
    let started = Instant::now();
    loop {
        if let Some(status) = process.child.try_wait().unwrap() {
            let elapsed = started.elapsed();
            // A descendant that outlives `once` can hold the pipe open, so only
            // give the reader a moment to catch up.
            let grace = Instant::now();
            while !process.reader.is_finished() && grace.elapsed() < Duration::from_millis(500) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            return (status, elapsed, process.stderr());
        }
        if started.elapsed() > deadline {
            let _ = process.child.kill();
            panic!("once did not exit within {deadline:?}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn streaming_run(
    stall_after_start: bool,
) -> (Collector, tempfile::TempDir, tempfile::TempDir, OnceProcess) {
    streaming_run_with(stall_after_start, Command::new(env!("CARGO_BIN_EXE_once"))).await
}

async fn streaming_run_with(
    stall_after_start: bool,
    command: Command,
) -> (Collector, tempfile::TempDir, tempfile::TempDir, OnceProcess) {
    let collector = Collector::default();
    collector.recorded.lock().unwrap().stall_after_start = stall_after_start;
    let url = serve(collector.clone()).await;
    let workspace = workspace(Some(&url));
    let home = tempfile::tempdir().unwrap();
    let child = spawn_once_with(workspace.path(), home.path(), command);
    let probe = collector.clone();
    wait_until(Duration::from_secs(30), || probe.started()).await;
    (collector, workspace, home, child)
}

#[tokio::test]
async fn sigterm_reports_a_cancelled_run_before_exiting() {
    let (collector, _workspace, _home, child) = streaming_run(false).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    send(&child, "TERM");
    let (status, elapsed, stderr) = wait_for_exit(child, Duration::from_secs(10)).await;

    assert_eq!(status.signal(), Some(libc::SIGTERM), "stderr: {stderr}");
    assert!(elapsed < Duration::from_secs(4), "took {elapsed:?}");
    let completions = collector.completions();
    assert_eq!(completions.len(), 1, "{completions:?}");
    assert_eq!(completions[0].result, RunResult::Cancelled as i32);
    assert_eq!(completions[0].cancellation_reason, "SIGTERM");
    assert!(completions[0].wall_ms > 0);
}

#[tokio::test]
async fn sigint_reports_the_signal_it_received() {
    let (collector, _workspace, _home, child) = streaming_run(false).await;
    send(&child, "INT");
    let (status, _, stderr) = wait_for_exit(child, Duration::from_secs(10)).await;

    assert_eq!(status.signal(), Some(libc::SIGINT), "stderr: {stderr}");
    let completions = collector.completions();
    assert_eq!(completions.len(), 1, "{completions:?}");
    assert_eq!(completions[0].result, RunResult::Cancelled as i32);
    assert_eq!(completions[0].cancellation_reason, "SIGINT");
}

#[tokio::test]
async fn an_unresponsive_service_cannot_hold_the_exit_past_the_deadline() {
    let (_collector, _workspace, _home, child) = streaming_run(true).await;
    send(&child, "TERM");
    let (status, elapsed, stderr) = wait_for_exit(child, Duration::from_secs(10)).await;

    assert_eq!(status.signal(), Some(libc::SIGTERM), "stderr: {stderr}");
    assert!(elapsed < Duration::from_secs(5), "took {elapsed:?}");
}

#[tokio::test]
async fn a_second_signal_exits_without_waiting_for_the_drain() {
    let (_collector, _workspace, _home, child) = streaming_run(true).await;
    send(&child, "TERM");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let signalled = Instant::now();
    send(&child, "INT");
    let (status, _, stderr) = wait_for_exit(child, Duration::from_secs(10)).await;

    assert_eq!(status.signal(), Some(libc::SIGINT), "stderr: {stderr}");
    assert!(signalled.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn a_run_that_is_not_streaming_keeps_the_default_signal_behavior() {
    let workspace = workspace(None);
    let home = tempfile::tempdir().unwrap();
    let child = spawn_once(workspace.path(), home.path());
    tokio::time::sleep(Duration::from_secs(1)).await;
    send(&child, "TERM");
    let (status, elapsed, stderr) = wait_for_exit(child, Duration::from_secs(5)).await;

    assert_eq!(status.signal(), Some(libc::SIGTERM), "stderr: {stderr}");
    assert!(elapsed < Duration::from_millis(500), "took {elapsed:?}");
    assert!(!stderr.contains("reporting the cancelled run"), "{stderr}");
}

#[tokio::test]
async fn a_signal_the_parent_ignored_stays_ignored() {
    // `sh` execs Once with SIGTERM ignored, the way `nohup` or a
    // non-interactive shell's background job hands down an ignored signal.
    let mut command = Command::new("/bin/sh");
    command.args([
        "-c",
        "trap '' TERM; exec \"$0\" \"$@\"",
        env!("CARGO_BIN_EXE_once"),
    ]);
    let (collector, _workspace, _home, mut child) = streaming_run_with(false, command).await;
    send(&child, "TERM");
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert!(
        child.child.try_wait().unwrap().is_none(),
        "once exited on an ignored SIGTERM"
    );
    assert!(collector.completions().is_empty());
    send(&child, "INT");
    let (status, _, stderr) = wait_for_exit(child, Duration::from_secs(10)).await;
    assert_eq!(status.signal(), Some(libc::SIGINT), "stderr: {stderr}");
    assert_eq!(collector.completions()[0].cancellation_reason, "SIGINT");
}

#[tokio::test]
async fn repeating_the_same_signal_exits_without_waiting_for_the_drain() {
    let (_collector, _workspace, _home, child) = streaming_run(true).await;
    send(&child, "TERM");
    // Pending deliveries of one signal merge in the kernel, so wait until Once
    // reports the first one before repeating it.
    wait_until(Duration::from_secs(5), || {
        child.stderr().contains("Once received SIGTERM")
    })
    .await;
    // A repeat within a quarter of a second counts as the same request.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let signalled = Instant::now();
    send(&child, "TERM");
    let (status, _, stderr) = wait_for_exit(child, Duration::from_secs(10)).await;

    assert_eq!(status.signal(), Some(libc::SIGTERM), "stderr: {stderr}");
    assert!(
        signalled.elapsed() < Duration::from_secs(1),
        "took {:?}",
        signalled.elapsed()
    );
}
