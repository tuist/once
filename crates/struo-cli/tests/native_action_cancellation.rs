//! A `struo` process stopped by SIGINT or SIGTERM takes the native actions it
//! started with it: the action leader and everything in its process group,
//! even processes that ignore the forwarded signal. Covers runs that stream to
//! an event service and runs that do not, and a terminal-style interrupt that
//! reaches Struo's whole process group but not the action groups.
#![cfg(unix)]

use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::{SinkExt, Stream};
use struo_events_client::proto::{
    run_event::Payload,
    run_event_service_server::{RunEventService, RunEventServiceServer},
    AckDisposition, ArgvHashKey, BatchAck, GetArgvHashKeyRequest, GetRunAckRequest,
    GetServerCapabilitiesRequest, RunEventAck, RunEventBatch, RunFinalization, ServerCapabilities,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tonic::{Request, Response, Status, Streaming};

#[path = "support/cache_miss.rs"]
mod cache_miss;

#[derive(Clone, Default)]
struct Collector {
    started: Arc<Mutex<bool>>,
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
        let started = self.started.clone();
        let (mut tx, rx) = futures::channel::mpsc::channel(32);
        tokio::spawn(async move {
            while let Ok(Some(batch)) = inbound.message().await {
                if batch
                    .events
                    .iter()
                    .any(|event| matches!(event.payload, Some(Payload::RunStarted(_))))
                {
                    *started.lock().unwrap() = true;
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

/// Serve the event service and a discovery document pointing at it. Cache
/// endpoint discovery points at an action cache that confirms every lookup as
/// a miss, so the action runs while the Tuist provider keeps streaming.
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

    let cache_endpoint = cache_miss::endpoint().await;
    let cache_body = format!("{{\"endpoints\":[\"{cache_endpoint}\"]}}");
    let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http_port = http.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = http.accept().await {
            let cache_body = cache_body.clone();
            tokio::spawn(async move {
                let mut request = vec![0_u8; 4096];
                let read = socket.read(&mut request).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..read]);
                let response = if request.starts_with("GET /.well-known/struo ") {
                    let body = format!("{{\"events\":[\"grpc://127.0.0.1:{grpc_port}\"]}}");
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{cache_body}",
                        cache_body.len()
                    )
                };
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    format!("http://127.0.0.1:{http_port}")
}

#[derive(Clone, Copy)]
enum Reporting {
    /// The local cache provider, so the run never streams.
    Off,
    /// A Tuist provider whose discovery points at the mock event service.
    Streaming,
}

struct Scenario {
    _workspace: tempfile::TempDir,
    _home: tempfile::TempDir,
    pids: tempfile::TempDir,
    struo: Child,
}

impl Scenario {
    fn pid_file(&self, name: &str) -> PathBuf {
        self.pids.path().join(name)
    }
}

/// The action ignores SIGINT and SIGTERM, as does the background child it
/// starts, so only the group-wide SIGKILL on exit stops them.
fn action_script(pids: &Path) -> String {
    format!(
        "trap '' TERM INT; echo $$ > {leader}; sleep 60 & echo $! > {background}; wait",
        leader = pids.join("leader").display(),
        background = pids.join("background").display(),
    )
}

async fn start(reporting: Reporting, own_group: bool) -> Scenario {
    let workspace = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let pids = tempfile::tempdir().unwrap();
    let collector = Collector::default();
    let manifest = match reporting {
        Reporting::Off => String::new(),
        Reporting::Streaming => {
            let url = serve(collector.clone()).await;
            format!(
                "[infrastructure.cache]\nprovider = \"tuist\"\n\n[infrastructures.tuist]\nkind = \"tuist\"\nurl = \"{url}\"\naccount = \"acme\"\nproject = \"app\"\n"
            )
        }
    };
    std::fs::write(workspace.path().join("struo.toml"), manifest).unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_struo"));
    command
        .args(["exec", "--", "/bin/sh", "-c", &action_script(pids.path())])
        .current_dir(workspace.path())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env("XDG_CACHE_HOME", home.path().join("cache"))
        .env("XDG_STATE_HOME", home.path().join("state"))
        .env("XDG_DATA_HOME", home.path().join("data"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(pids.path().join("stderr")).unwrap());
    match reporting {
        Reporting::Off => {
            command.env("STRUO_CACHE_PROVIDER", "local");
        }
        Reporting::Streaming => {
            command.env("TUIST_TOKEN", "test-token");
        }
    }
    if own_group {
        command.process_group(0);
    }
    let struo = command.spawn().unwrap();
    let scenario = Scenario {
        _workspace: workspace,
        _home: home,
        pids,
        struo,
    };
    let stderr = scenario.pid_file("stderr");
    wait_until(&stderr, || {
        read_pid(&scenario.pid_file("background")).is_some()
    })
    .await;
    if matches!(reporting, Reporting::Streaming) {
        wait_until(&stderr, || *collector.started.lock().unwrap()).await;
    }
    // Let the termination watcher settle after the run starts.
    tokio::time::sleep(Duration::from_millis(300)).await;
    scenario
}

async fn wait_until(stderr: &Path, mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "condition not met in time; struo stderr: {}",
            std::fs::read_to_string(stderr).unwrap_or_default()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn read_pid(path: &Path) -> Option<String> {
    let pid = std::fs::read_to_string(path).ok()?;
    let pid = pid.trim();
    (!pid.is_empty()).then(|| pid.to_string())
}

fn alive(pid: &str) -> bool {
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&output.stdout);
    let stat = stat.trim();
    !stat.is_empty() && !stat.starts_with('Z')
}

fn signal_pid(pid: u32, signal: &str) {
    assert!(Command::new("kill")
        .args(["-s", signal, &pid.to_string()])
        .status()
        .unwrap()
        .success());
}

fn signal_group(pgid: u32, signal: &str) {
    assert!(Command::new("kill")
        .args(["-s", signal, "--", &format!("-{pgid}")])
        .status()
        .unwrap()
        .success());
}

async fn wait_for_exit(struo: &mut Child) -> ExitStatus {
    let started = Instant::now();
    loop {
        if let Some(status) = struo.try_wait().unwrap() {
            return status;
        }
        if started.elapsed() > Duration::from_secs(15) {
            let _ = struo.kill();
            panic!("struo did not exit");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Assert the action leader and its background child are gone, killing any
/// survivor so a failure does not leak processes.
async fn assert_action_stopped(scenario: &Scenario) {
    let pids = ["leader", "background"]
        .map(|name| read_pid(&scenario.pid_file(name)).expect("pid recorded"));
    let started = Instant::now();
    while pids.iter().any(|pid| alive(pid)) && started.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let survivors = pids
        .iter()
        .filter(|pid| alive(pid))
        .cloned()
        .collect::<Vec<_>>();
    for pid in &survivors {
        let _ = Command::new("kill").args(["-9", pid]).status();
    }
    assert!(
        survivors.is_empty(),
        "action processes outlived struo: {survivors:?}"
    );
}

async fn check(reporting: Reporting, signal: &str, raw: i32, terminal: bool) {
    let mut scenario = start(reporting, terminal).await;
    let pid = scenario.struo.id();
    if terminal {
        signal_group(pid, signal);
    } else {
        signal_pid(pid, signal);
    }
    let status = wait_for_exit(&mut scenario.struo).await;
    assert_eq!(status.signal(), Some(raw), "{status:?}");
    assert_action_stopped(&scenario).await;
}

#[tokio::test]
async fn sigint_stops_a_non_streaming_action_group() {
    check(Reporting::Off, "INT", libc::SIGINT, false).await;
}

#[tokio::test]
async fn sigterm_stops_a_non_streaming_action_group() {
    check(Reporting::Off, "TERM", libc::SIGTERM, false).await;
}

#[tokio::test]
async fn a_terminal_interrupt_stops_a_non_streaming_action_group() {
    check(Reporting::Off, "INT", libc::SIGINT, true).await;
}

#[tokio::test]
async fn sigint_stops_a_streaming_action_group() {
    check(Reporting::Streaming, "INT", libc::SIGINT, false).await;
}

#[tokio::test]
async fn sigterm_stops_a_streaming_action_group() {
    check(Reporting::Streaming, "TERM", libc::SIGTERM, false).await;
}

#[tokio::test]
async fn a_terminal_interrupt_stops_a_streaming_action_group() {
    check(Reporting::Streaming, "INT", libc::SIGINT, true).await;
}
