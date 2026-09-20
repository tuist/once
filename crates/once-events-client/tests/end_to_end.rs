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
    authorizations: Vec<String>,
    stall: bool,
    break_first_stream: bool,
    invalid_hash_key: bool,
}

#[derive(Default, Clone)]
struct TestServer {
    recorded: Arc<Mutex<RecordedRun>>,
}

#[tonic::async_trait]
impl RunEventService for TestServer {
    async fn get_server_capabilities(
        &self,
        req: Request<GetServerCapabilitiesRequest>,
    ) -> Result<Response<ServerCapabilities>, Status> {
        self.recorded.lock().await.authorizations.push(
            req.metadata()
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string(),
        );
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
            key_bytes: if self.recorded.lock().await.invalid_hash_key {
                Vec::new()
            } else {
                vec![0; 32]
            },
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
                if state.stall {
                    drop(state);
                    std::future::pending::<()>().await;
                    return;
                }
                if state.expected_next_seq == 0 {
                    assert_eq!(batch.seq_from, 1);
                    assert!(batch.gap_advances.is_empty());
                    assert!(matches!(
                        batch.events[0].payload,
                        Some(once_events_client::proto::run_event::Payload::RunStarted(_))
                    ));
                    state.expected_next_seq = 1;
                }
                for pair in batch.events.windows(2) {
                    assert_eq!(pair[0].seq + 1, pair[1].seq);
                }
                for gap in &batch.gap_advances {
                    assert!(gap.last_dropped_seq < batch.seq_from);
                    assert!(gap.first_dropped_seq <= state.expected_next_seq);
                    if gap.last_dropped_seq + 1 > state.expected_next_seq {
                        state.expected_next_seq = gap.last_dropped_seq + 1;
                    }
                }
                let last_event_seq = batch.events.last().map(|e| e.seq).unwrap_or_default();
                let acked_seq = if last_event_seq > 0 {
                    assert!(batch.seq_from <= state.expected_next_seq);
                    state.expected_next_seq = state.expected_next_seq.max(last_event_seq + 1);
                    state.expected_next_seq - 1
                } else if state.expected_next_seq > 0 {
                    state.expected_next_seq - 1
                } else {
                    0
                };
                let run_id = batch.run_id.clone();
                let batch_id = batch.batch_id.clone();
                let expected_next = state.expected_next_seq;
                state.batches.push(batch);
                if state.break_first_stream {
                    state.break_first_stream = false;
                    drop(state);
                    let _ = tx
                        .send(Err(Status::unavailable("lost acknowledgement")))
                        .await;
                    return;
                }
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
                    dashboard_url: "https://dashboard.example/runs/canonical".into(),
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
        req: Request<GetRunAckRequest>,
    ) -> Result<Response<RunEventAck>, Status> {
        let authorization = req
            .metadata()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let mut state = self.recorded.lock().await;
        state.authorizations.push(authorization);
        Ok(Response::new(RunEventAck {
            run_id: req.into_inner().run_id,
            acked_seq: state.expected_next_seq.saturating_sub(1),
            expected_next_seq: state.expected_next_seq,
            observed_high_water_seq: 0,
            finalization: RunFinalization::Active as i32,
            dashboard_url: "https://dashboard.example/runs/canonical".into(),
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
async fn rejects_unrecognized_argument_policy_before_publishing() {
    let (channel, recorded) = start_server().await;
    let client = EventClient::new(
        channel,
        TransportConfig {
            run_id: "unsupported-policy".into(),
            ..Default::default()
        },
    )
    .with_metadata(once_events_client::proto::RunStarted {
        safe_literal_allowlist_version: "newer-than-server".into(),
        ..Default::default()
    });
    let bus = RunEventBus::new(4);
    let delivery = client.run_with_bus(bus.clone());
    bus.publish(RunEvent::RunStarted { at_epoch_ms: 1 });
    assert!(matches!(
        delivery.await,
        Err(once_events_client::TransportError::UnsupportedCapabilities)
    ));
    assert!(recorded.lock().await.batches.is_empty());
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
#[allow(clippy::too_many_lines)]
async fn authenticated_shutdown_drains_individual_actions_and_metadata() {
    use once_events_client::proto::{run_event::Payload, RunStarted};
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
        bus.publish(RunEvent::ActionAttemptStarted {
            at_epoch_ms: 180 + i64::from(index),
            target_id: "target".into(),
            capability: "build".into(),
            action_index: index,
            attempt: 1,
            worker_id: format!("worker-{index}"),
        });
        bus.publish(RunEvent::ActionAttemptCompleted {
            at_epoch_ms: 200 + i64::from(index),
            target_id: "target".into(),
            capability: "build".into(),
            action_index: index,
            attempt: 1,
            result: once_core::TargetResult::Succeeded,
            exit_code: 0,
            duration_ms: 15,
            was_cached: index == 0,
        });
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
            prepare_ms: 5,
            execute_ms: 10,
            cache_key: String::new(),
            selected_attempt: 1,
        });
    }
    bus.publish(RunEvent::TestCaseCompleted {
        at_epoch_ms: 250,
        target_id: "tests".into(),
        case_id: "parser::case".into(),
        name: "case".into(),
        suite_id: "parser".into(),
        attempt: 1,
        result: once_core::TestCaseResult::Unknown,
        duration_ms: 0,
        duration_known: false,
        failure_message: None,
    });
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
        [
            "Bearer test-token",
            "Bearer test-token",
            "Bearer test-token"
        ]
    );
    let unique: std::collections::BTreeMap<_, _> = recorded
        .batches
        .iter()
        .flat_map(|b| &b.events)
        .map(|e| (e.seq, e))
        .collect();
    let events: Vec<_> = unique.values().filter_map(|e| e.payload.as_ref()).collect();
    assert!(events
        .iter()
        .any(|e| matches!(e, Payload::RunStarted(s) if s.once_version == "test-version")));
    let actions: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Payload::ActionCompleted(a) => Some(a),
            _ => None,
        })
        .collect();
    assert_eq!(actions.len(), 2);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Payload::ActionAttemptStarted(_)))
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Payload::ActionAttemptCompleted(_)))
            .count(),
        2
    );
    assert!(actions.iter().all(|action| action.selected_attempt == 1));
    assert!(events.iter().any(|event| matches!(event,
        Payload::TestCaseCompleted(case) if case.case_id == "parser::case"
            && case.name == "case" && case.suite_id == "parser"
            && case.result == once_events_client::proto::TestCaseResult::Unknown as i32
            && case.observed_duration_ms.is_none()
    )));
    assert_eq!(actions[0].identifier, "action-0");
    assert!(actions[0].was_cached);
    assert_eq!(actions[1].identifier, "action-1");
    assert!(!actions[1].was_cached);
    assert!(events
        .iter()
        .any(|e| matches!(e, Payload::RunCompleted(c) if c.wall_ms == 200)));
}

#[tokio::test]
async fn canonical_dashboard_link_arrives_while_run_is_active() {
    let (channel, _) = start_server().await;
    let (links, mut received) = tokio::sync::mpsc::unbounded_channel();
    let client = EventClient::new(
        channel,
        TransportConfig {
            run_id: "live-link".into(),
            ..Default::default()
        },
    )
    .with_dashboard_link(move |link| {
        links.send(link.to_string()).unwrap();
    });
    let bus = RunEventBus::new(16);
    let delivery = client.run_with_bus(bus.clone());
    // Publish before polling: run_with_bus must already be subscribed.
    bus.publish(RunEvent::RunStarted { at_epoch_ms: 1 });
    let task = tokio::spawn(delivery);
    let link = tokio::time::timeout(std::time::Duration::from_secs(2), received.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(link, "https://dashboard.example/runs/canonical");
    bus.publish(RunEvent::RunCompleted {
        at_epoch_ms: 2,
        exit_status: 0,
    });
    drop(bus);
    task.await.unwrap().unwrap();
    assert!(received.recv().await.is_none());
}

#[tokio::test]
async fn shutdown_deadline_covers_stalled_acknowledgements() {
    let (channel, recorded) = start_server().await;
    recorded.lock().await.stall = true;
    let client = EventClient::new(
        channel,
        TransportConfig {
            run_id: "stalled".into(),
            final_drain: std::time::Duration::from_millis(50),
            ..Default::default()
        },
    );
    let bus = RunEventBus::new(16);
    let rx = bus.subscribe();
    bus.publish(RunEvent::RunStarted { at_epoch_ms: 1 });
    bus.publish(RunEvent::RunCompleted {
        at_epoch_ms: 2,
        exit_status: 0,
    });
    let (tx, shutdown) = tokio::sync::oneshot::channel();
    tx.send(()).unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        client.run_with_reconnect(rx, shutdown, once_events_client::ReconnectPolicy::default()),
    )
    .await
    .unwrap();
    assert!(matches!(
        result,
        Err(once_events_client::TransportError::DrainTimeout)
    ));
}

#[tokio::test]
async fn reconnect_after_lost_ack_preserves_shutdown_and_terminal() {
    let (channel, recorded) = start_server().await;
    recorded.lock().await.break_first_stream = true;
    let client = EventClient::authenticated(
        channel,
        TransportConfig {
            run_id: "reconnect".into(),
            ..Default::default()
        },
        "reconnect-token",
    )
    .unwrap();
    let bus = RunEventBus::new(16);
    let rx = bus.subscribe();
    bus.publish(RunEvent::RunStarted { at_epoch_ms: 1 });
    bus.publish(RunEvent::RunCompleted {
        at_epoch_ms: 2,
        exit_status: 0,
    });
    let (tx, shutdown) = tokio::sync::oneshot::channel();
    tx.send(()).unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        client.run_with_reconnect(
            rx,
            shutdown,
            once_events_client::ReconnectPolicy {
                initial_backoff: std::time::Duration::from_millis(5),
                ..Default::default()
            },
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result, 4);
    assert_eq!(recorded.lock().await.expected_next_seq, 4);
    assert!(recorded
        .lock()
        .await
        .authorizations
        .iter()
        .all(|value| value == "Bearer reconnect-token"));
}

#[tokio::test]
async fn malformed_hash_key_is_rejected_before_argument_disclosure() {
    let (channel, recorded) = start_server().await;
    recorded.lock().await.invalid_hash_key = true;
    let mut client = EventClient::new(channel, TransportConfig::default());
    assert!(matches!(
        client.argv_hash_key("owner/project".into()).await,
        Err(once_events_client::TransportError::InvalidHashKey)
    ));
    assert!(recorded.lock().await.batches.is_empty());
}
