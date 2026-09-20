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
    AckDisposition, ArgvHashKey, BatchAck, GetArgvHashKeyRequest, GetRunAckRequest,
    GetServerCapabilitiesRequest, RunEventAck, RunEventBatch, RunFinalization, ServerCapabilities,
    run_event_service_server::{RunEventService, RunEventServiceServer},
};

#[derive(Default)]
struct RecordedRun {
    batches: Vec<RunEventBatch>,
    expected_next_seq: u64,
    authorizations: Vec<String>,
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
            live_url_template: String::new(),
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
        req: Request<GetArgvHashKeyRequest>,
    ) -> Result<Response<ArgvHashKey>, Status> {
        self.recorded.lock().await.authorizations.push(
            req.metadata()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string(),
        );
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
        self.recorded.lock().await.authorizations.push(
            req.metadata()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string(),
        );
        let recorded = self.recorded.clone();
        let mut inbound = req.into_inner();
        let first = inbound
            .message()
            .await?
            .ok_or_else(|| Status::invalid_argument("missing first batch"))?;
        let mut inbound = tokio_stream::once(Ok(first)).chain(inbound);
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

#[tokio::test]
async fn authenticated_shutdown_drains_individual_actions_and_metadata() {
    use once_events_client::proto::{RunStarted, run_event::Payload};
    let (channel, recorded) = start_server().await;
    let mut client = EventClient::authenticated(
        channel,
        TransportConfig {
            run_id: "action-run".into(),
            batch_flush: std::time::Duration::from_millis(5),
            ..TransportConfig::default()
        },
        "test-token",
    )
    .unwrap();
    assert_eq!(
        client
            .argv_hash_key("owner/project".into())
            .await
            .unwrap()
            .key_id,
        "test-key"
    );
    let client = client.with_metadata(RunStarted {
        once_version: "test-version".into(),
        ..Default::default()
    });
    let bus = RunEventBus::new(16);
    let rx = bus.subscribe();
    bus.publish(RunEvent::RunStarted { at_epoch_ms: 100 });
    for index in 0..2 {
        bus.publish(RunEvent::ActionCompleted {
            at_epoch_ms: 200 + i64::from(index),
            target_id: "target".into(),
            capability: "build".into(),
            action_index: index,
            identifier: Some(format!("action-{index}")),
            result: once_core::TargetResult::Succeeded,
            was_cached: index == 0,
            duration_ms: 15,
            exit_code: 0,
            start_at_epoch_ms: 200 + i64::from(index) - 15,
            worker_id: format!("worker-{index}"),
        });
    }
    bus.publish(RunEvent::RunCompleted {
        at_epoch_ms: 300,
        exit_status: 0,
    });
    let (tx, shutdown) = tokio::sync::oneshot::channel();
    tx.send(()).unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.run_until_shutdown(rx, shutdown),
    )
    .await
    .unwrap()
    .unwrap();
    let recorded = recorded.lock().await;
    assert_eq!(
        recorded.authorizations,
        ["Bearer test-token", "Bearer test-token"]
    );
    let unique: std::collections::BTreeMap<_, _> = recorded
        .batches
        .iter()
        .flat_map(|b| &b.events)
        .map(|e| (e.seq, e))
        .collect();
    let events: Vec<_> = unique.values().filter_map(|e| e.payload.as_ref()).collect();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Payload::RunStarted(s) if s.once_version == "test-version"))
    );
    let actions: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Payload::ActionCompleted(a) => Some(a),
            _ => None,
        })
        .collect();
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].identifier, "action-0");
    assert!(actions[0].was_cached);
    assert_eq!(actions[1].identifier, "action-1");
    assert!(!actions[1].was_cached);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Payload::RunCompleted(c) if c.wall_ms == 200))
    );
}
