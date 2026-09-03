//! End-to-end test: an in-process gRPC server implementing
//! `RunEventService` sits behind a tonic channel, and the transport
//! streams events through a live bidi call. Proves the codegen, the
//! transport, the session, and the bridge all cooperate.

use std::pin::Pin;
use std::sync::Arc;

use once_core::{RunEvent, RunEventBus};
use once_events_client::{EventClient, TransportConfig};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tokio_stream::{Stream, StreamExt};
use tonic::transport::{Channel, Server, Uri};
use tonic::{Request, Response, Status, Streaming};

use once_events_client::proto::{
    run_event_service_server::{RunEventService, RunEventServiceServer},
    AckDisposition, ArgvHashKey, BatchAck, GetArgvHashKeyRequest, GetRunAckRequest,
    GetServerCapabilitiesRequest, RunEventAck, RunEventBatch, RunFinalization, ServerCapabilities,
};

#[derive(Default)]
struct RecordedRun {
    batches: Vec<RunEventBatch>,
    expected_next_seq: u64,
}

#[derive(Default, Clone)]
struct TestServer {
    recorded: Arc<Mutex<RecordedRun>>,
}

#[tonic::async_trait]
impl RunEventService for TestServer {
    async fn get_server_capabilities(
        &self,
        _req: Request<GetServerCapabilitiesRequest>,
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
        _req: Request<GetArgvHashKeyRequest>,
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
        req: Request<Streaming<RunEventBatch>>,
    ) -> Result<Response<Self::PublishRunEventsStream>, Status> {
        let recorded = self.recorded.clone();
        let mut inbound = req.into_inner();
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<BatchAck, Status>>(32);
        tokio::spawn(async move {
            while let Some(item) = inbound.next().await {
                let batch = match item {
                    Ok(b) => b,
                    Err(status) => {
                        let _ = tx.send(Err(status)).await;
                        return;
                    }
                };
                let mut state = recorded.lock().await;
                for gap in &batch.gap_advances {
                    if gap.last_dropped_seq + 1 > state.expected_next_seq {
                        state.expected_next_seq = gap.last_dropped_seq + 1;
                    }
                }
                let last_event_seq = batch.events.last().map(|e| e.seq).unwrap_or_default();
                let acked_seq = if last_event_seq > 0 {
                    state.expected_next_seq = last_event_seq + 1;
                    last_event_seq
                } else if state.expected_next_seq > 0 {
                    state.expected_next_seq - 1
                } else {
                    0
                };
                let run_id = batch.run_id.clone();
                let batch_id = batch.batch_id.clone();
                let expected_next = state.expected_next_seq;
                state.batches.push(batch);
                drop(state);
                let ack = BatchAck {
                    run_id,
                    batch_id,
                    disposition: AckDisposition::Accepted as i32,
                    acked_seq,
                    expected_next_seq: expected_next,
                    observed_high_water_seq: 0,
                    retry_after_ms: 0,
                    max_in_flight_batches: 0,
                    finalization: RunFinalization::Active as i32,
                };
                if tx.send(Ok(ack)).await.is_err() {
                    return;
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn get_run_ack(
        &self,
        _req: Request<GetRunAckRequest>,
    ) -> Result<Response<RunEventAck>, Status> {
        let state = self.recorded.lock().await;
        Ok(Response::new(RunEventAck {
            run_id: String::new(),
            acked_seq: state.expected_next_seq.saturating_sub(1),
            expected_next_seq: state.expected_next_seq,
            observed_high_water_seq: 0,
            finalization: RunFinalization::Active as i32,
        }))
    }
}

async fn start_server() -> (Channel, Arc<Mutex<RecordedRun>>) {
    let recorded = Arc::new(Mutex::new(RecordedRun::default()));
    let service = TestServer {
        recorded: recorded.clone(),
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        Server::builder()
            .add_service(RunEventServiceServer::new(service))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let channel = Channel::builder(Uri::try_from(format!("http://{addr}")).unwrap())
        .connect()
        .await
        .unwrap();
    (channel, recorded)
}

#[tokio::test]
async fn capabilities_roundtrip() {
    let (channel, _) = start_server().await;
    let mut client = EventClient::new(channel, TransportConfig::default());
    let caps = client.capabilities().await.unwrap();
    assert_eq!(caps.finalization_grace_ms, 2_000);
    assert_eq!(caps.safe_literal_allowlist_version, "2026.09.03-v1");
}

#[tokio::test]
async fn delivers_run_lifecycle_and_target_events() {
    let (channel, recorded) = start_server().await;

    let config = TransportConfig {
        run_id: "e2e-run-real".into(),
        batch_flush: std::time::Duration::from_millis(15),
        final_drain: std::time::Duration::from_secs(2),
        ..TransportConfig::default()
    };
    let client = EventClient::new(channel, config);

    let bus = RunEventBus::new(64);
    let bus_rx = bus.subscribe();
    let producer_bus = bus.clone();
    drop(bus);
    let handle = tokio::spawn(async move { client.run(bus_rx).await });

    producer_bus.publish(RunEvent::RunStarted { at_epoch_ms: 100 });
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    producer_bus.publish(RunEvent::TargetCompleted {
        at_epoch_ms: 110,
        target_id: "//foo:bar".into(),
        result: once_core::TargetResult::Succeeded,
        was_cached: false,
        duration_ms: 8,
    });
    producer_bus.publish(RunEvent::RunCompleted {
        at_epoch_ms: 120,
        exit_status: 0,
    });
    drop(producer_bus);

    let last_expected = handle
        .await
        .expect("task joins")
        .expect("transport succeeds");

    let state = recorded.lock().await;
    let all_seqs: Vec<u64> = state
        .batches
        .iter()
        .flat_map(|b| b.events.iter().map(|e| e.seq))
        .collect();
    let batch_count = state.batches.len();
    let last_batch_events: Vec<u64> = state
        .batches
        .last()
        .map(|b| b.events.iter().map(|e| e.seq).collect())
        .unwrap_or_default();
    assert!(
        all_seqs.contains(&1) && all_seqs.contains(&2) && all_seqs.contains(&3),
        "missing events; observed {all_seqs:?} across {batch_count} batches; last batch events {last_batch_events:?}; last_expected={last_expected}"
    );
    assert!(
        last_expected >= 4,
        "expected_next_seq was {last_expected}; batches {batch_count}; seqs {all_seqs:?}"
    );
}
