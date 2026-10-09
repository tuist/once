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

#[derive(Default, PartialEq)]
enum StreamFailure {
    #[default]
    None,
    BeforeAck,
    AfterAck,
}

#[derive(Default)]
struct RecordedRun {
    batches: Vec<RunEventBatch>,
    expected_next_seq: u64,
    authorizations: Vec<String>,
    projects: Vec<String>,
    stall: bool,
    stream_failure: StreamFailure,
    expired_bearer: Option<String>,
    invalid_hash_key: bool,
    ack_delay: std::time::Duration,
}

async fn send_ack(
    tx: &tokio::sync::mpsc::Sender<Result<BatchAck, Status>>,
    ack: BatchAck,
    delay: std::time::Duration,
    close_after_ack: bool,
) -> bool {
    tokio::time::sleep(delay).await;
    if tx.send(Ok(ack)).await.is_err() {
        return false;
    }
    if close_after_ack {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let _ = tx.send(Err(Status::unavailable("closed after ack"))).await;
        return false;
    }
    true
}

fn project_id_header(metadata: &tonic::metadata::MetadataMap) -> String {
    metadata
        .get(once_events_client::PROJECT_ID_METADATA)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

#[derive(Default, Clone)]
struct TestServer {
    recorded: Arc<Mutex<RecordedRun>>,
}

impl TestServer {
    /// Records what the call presented and refuses a bearer the test marked expired,
    /// the way the real service refuses one it can no longer authenticate.
    async fn admit(&self, metadata: &tonic::metadata::MetadataMap) -> Result<(), Status> {
        let bearer = metadata
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let mut state = self.recorded.lock().await;
        state.projects.push(project_id_header(metadata));
        let refused = state.expired_bearer.as_deref() == Some(bearer.as_str());
        state.authorizations.push(bearer);
        if refused {
            return Err(Status::unauthenticated("missing or invalid bearer"));
        }
        Ok(())
    }
}

#[tonic::async_trait]
impl RunEventService for TestServer {
    async fn get_server_capabilities(
        &self,
        req: Request<GetServerCapabilitiesRequest>,
    ) -> Result<Response<ServerCapabilities>, Status> {
        let project = project_id_header(req.metadata());
        let mut recorded = self.recorded.lock().await;
        recorded.projects.push(project);
        recorded.authorizations.push(
            req.metadata()
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string(),
        );
        drop(recorded);
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
        self.admit(req.metadata()).await?;
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
                if state.stream_failure == StreamFailure::BeforeAck {
                    state.stream_failure = StreamFailure::None;
                    drop(state);
                    let _ = tx
                        .send(Err(Status::unavailable("lost acknowledgement")))
                        .await;
                    return;
                }
                let ack_delay = state.ack_delay;
                let close_after_ack = state.stream_failure == StreamFailure::AfterAck;
                state.stream_failure = StreamFailure::None;
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
                if !send_ack(&tx, ack, ack_delay, close_after_ack).await {
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
        self.admit(req.metadata()).await?;
        let state = self.recorded.lock().await;
        let acked_seq = state.expected_next_seq.saturating_sub(1);
        Ok(Response::new(RunEventAck {
            run_id: req.into_inner().run_id,
            acked_seq,
            expected_next_seq: acked_seq + 1,
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
async fn sends_the_project_id_with_the_credentials_on_a_call() {
    let (channel, recorded) = start_server().await;
    let mut client = EventClient::authenticated_with_project_id(
        channel,
        TransportConfig::default(),
        "session-token",
        "some-project-id",
    )
    .unwrap();
    client.capabilities().await.unwrap();

    let recorded = recorded.lock().await;
    assert_eq!(recorded.projects, vec!["some-project-id".to_string()]);
    assert_eq!(
        recorded.authorizations,
        vec!["Bearer session-token".to_string()]
    );
}

#[tokio::test]
async fn sends_no_project_id_when_none_is_given() {
    let (channel, recorded) = start_server().await;
    let mut client =
        EventClient::authenticated(channel, TransportConfig::default(), "session-token").unwrap();
    client.capabilities().await.unwrap();

    assert_eq!(recorded.lock().await.projects, vec![String::new()]);
}

#[tokio::test]
async fn names_the_project_on_every_call_across_a_reconnect() {
    let (channel, recorded) = start_server().await;
    recorded.lock().await.stream_failure = StreamFailure::BeforeAck;
    let client = EventClient::authenticated_with_project_id(
        channel,
        TransportConfig {
            run_id: "reconnect-project".into(),
            ..Default::default()
        },
        "reconnect-token",
        "some-project-id",
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
    tokio::time::timeout(
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

    let recorded = recorded.lock().await;
    assert!(
        recorded.authorizations.len() >= 3,
        "capabilities, two streams and a run ack"
    );
    assert_eq!(recorded.projects.len(), recorded.authorizations.len());
    assert!(recorded
        .projects
        .iter()
        .all(|project| project == "some-project-id"));
}

#[tokio::test]
async fn rejects_project_ids_that_are_not_valid_metadata() {
    let (channel, _) = start_server().await;
    for invalid in [
        "",
        "has space",
        "line\nbreak",
        "caf\u{e9}",
        &"x".repeat(257),
    ] {
        let result = EventClient::authenticated_with_project_id(
            channel.clone(),
            TransportConfig::default(),
            "token",
            invalid,
        );
        assert!(
            matches!(result, Err(once_events_client::CredentialsError::ProjectId)),
            "{invalid:?} should be rejected"
        );
    }
    assert!(EventClient::authenticated_with_project_id(
        channel,
        TransportConfig::default(),
        "token",
        &"x".repeat(256),
    )
    .is_ok());
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
    let committed = std::iter::once("src/source-0.c".to_string())
        .chain(
            (0..3_000)
                .filter(|file| file % 2 == 0)
                .map(|file| format!("source-files/file-{file}.c")),
        )
        .collect();
    let client = client
        .with_metadata(RunStarted {
            once_version: "test-version".into(),
            ..Default::default()
        })
        .with_source_files(once_events_client::SourceFileSnapshot::from_committed_paths(committed));
    let bus = RunEventBus::new(16);
    let rx = bus.subscribe();
    bus.publish(RunEvent::RunStarted { at_epoch_ms: 100 });
    for index in 0..3 {
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
            history: Some(Box::new(once_core::ActionHistoryKey {
                namespace: "test.history.v1".into(),
                key: format!("action-{index}"),
            })),
            presentation: Some(Box::new(once_core::ActionPresentation {
                context: vec![once_core::ActionContext {
                    key: "custom.mode".into(),
                    value: "release".into(),
                    label: "Release".into(),
                }],
                ..Default::default()
            })),
            display_name: Some(format!("Compile source-{index}.c")),
            source_files: if index == 2 {
                (0..3_000)
                    .map(|file| format!("source-files/file-{file}.c"))
                    .collect()
            } else {
                vec![format!("src/source-{index}.c")]
            },
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
    assert_eq!(actions.len(), 3);
    for action in &actions {
        let history = action.history.as_ref().expect("field 19 retains history");
        assert_eq!(history.namespace, "test.history.v1");
        assert_eq!(history.key, format!("action-{}", action.action_index));
    }
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Payload::ActionAttemptStarted(_)))
            .count(),
        3
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Payload::ActionAttemptCompleted(_)))
            .count(),
        3
    );
    assert!(actions.iter().all(|action| action.selected_attempt == 1));
    for action in &actions {
        let index = action.action_index;
        assert_eq!(
            action.display_name.as_deref(),
            Some(format!("Compile source-{index}.c").as_str())
        );
        assert_eq!(
            action.presentation.as_ref().unwrap().context[0].value,
            "release"
        );
        assert_eq!(action.source_files.len(), action.source_file_statuses.len());
        for (path, status) in action.source_files.iter().zip(&action.source_file_statuses) {
            let committed = path == "src/source-0.c"
                || path
                    .strip_prefix("source-files/file-")
                    .and_then(|path| path.strip_suffix(".c"))
                    .and_then(|index| index.parse::<usize>().ok())
                    .is_some_and(|index| index.is_multiple_of(2));
            assert_eq!(*status, if committed { 1 } else { 2 });
        }
        if index == 2 {
            assert!(!action.source_files.is_empty());
            assert!(action.source_files.len() < 3_000);
            assert_eq!(action.source_files[0], "source-files/file-0.c");
        } else {
            assert_eq!(action.source_files, [format!("src/source-{index}.c")]);
        }
    }
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
async fn shutdown_drains_cached_action_burst_beyond_two_seconds() {
    use once_events_client::proto::run_event::Payload;
    let (channel, recorded) = start_server().await;
    recorded.lock().await.ack_delay = std::time::Duration::from_millis(150);
    let client = EventClient::new(
        channel,
        TransportConfig {
            run_id: "cached-burst".into(),
            limits: once_events_client::SessionLimits {
                max_events_per_batch: 16,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    let bus = RunEventBus::new(512);
    let rx = bus.subscribe();
    bus.publish(RunEvent::RunStarted { at_epoch_ms: 1 });
    for index in 0..256 {
        bus.publish(RunEvent::ActionCompleted {
            at_epoch_ms: 2,
            target_id: "cached-target".into(),
            capability: "build".into(),
            action_index: index,
            identifier: Some(format!("action-{index}")),
            display_name: None,
            source_files: Vec::new(),
            history: None,
            presentation: None,
            result: once_core::TargetResult::Succeeded,
            was_cached: true,
            duration_ms: 0,
            exit_code: 0,
            start_at_epoch_ms: 2,
            worker_id: "local".into(),
            prepare_ms: 0,
            execute_ms: 0,
            cache_key: String::new(),
            selected_attempt: 1,
        });
    }
    bus.publish(RunEvent::RunCompleted {
        at_epoch_ms: 3,
        exit_status: 0,
    });
    let (tx, shutdown) = tokio::sync::oneshot::channel();
    tx.send(()).unwrap();
    let started = std::time::Instant::now();
    let expected_next = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client.run_until_shutdown(rx, shutdown),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(started.elapsed() > std::time::Duration::from_secs(2));
    let recorded = recorded.lock().await;
    let actions: std::collections::BTreeSet<_> = recorded
        .batches
        .iter()
        .flat_map(|batch| &batch.events)
        .filter_map(|event| match &event.payload {
            Some(Payload::ActionCompleted(action)) => Some(action.identifier.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(actions.len(), 256);
    assert!(recorded
        .batches
        .iter()
        .flat_map(|batch| &batch.events)
        .any(|event| matches!(&event.payload, Some(Payload::RunCompleted(_)))));
    assert_eq!(recorded.expected_next_seq, expected_next);
}

#[tokio::test]
async fn abandoned_producer_is_finalized_as_failed() {
    use once_events_client::proto::{run_event::Payload, RunResult};
    for dropped_shutdown in [false, true] {
        let (channel, recorded) = start_server().await;
        let client = EventClient::new(
            channel,
            TransportConfig {
                run_id: "abandoned".into(),
                ..Default::default()
            },
        );
        let bus = RunEventBus::new(16);
        let rx = bus.subscribe();
        bus.publish(RunEvent::RunStarted { at_epoch_ms: 1 });
        let (tx, shutdown) = tokio::sync::oneshot::channel();
        let result = if dropped_shutdown {
            drop(tx);
            client.run_until_shutdown(rx, shutdown).await
        } else {
            drop(bus);
            client.run(rx).await
        };
        result.unwrap();
        let recorded = recorded.lock().await;
        let terminal: Vec<_> = recorded
            .batches
            .iter()
            .flat_map(|batch| &batch.events)
            .filter_map(|event| match &event.payload {
                Some(Payload::RunCompleted(completed)) => Some(completed.result),
                _ => None,
            })
            .collect();
        assert_eq!(terminal, [RunResult::Failed as i32]);
    }
}

#[tokio::test]
async fn shutdown_unblocks_reconnect_with_an_empty_acked_session() {
    use once_events_client::proto::{run_event::Payload, RunResult};
    for closed_bus in [false, true] {
        let (channel, recorded) = start_server().await;
        recorded.lock().await.stream_failure = StreamFailure::AfterAck;
        let client = EventClient::new(
            channel,
            TransportConfig {
                run_id: "acked-before-reconnect".into(),
                final_drain: std::time::Duration::from_secs(1),
                ..Default::default()
            },
        );
        let bus = RunEventBus::new(16);
        let rx = bus.subscribe();
        bus.publish(RunEvent::RunStarted { at_epoch_ms: 1 });
        let (tx, shutdown) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            client
                .run_with_reconnect(
                    rx,
                    shutdown,
                    once_events_client::ReconnectPolicy {
                        initial_backoff: std::time::Duration::from_millis(5),
                        ..Default::default()
                    },
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if recorded.lock().await.authorizations.len() >= 3 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        if closed_bus {
            drop(bus);
        } else {
            tx.send(()).unwrap();
        }
        tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(recorded.lock().await.batches.iter().flat_map(|batch| &batch.events).any(|event| matches!(&event.payload, Some(Payload::RunCompleted(completed)) if completed.result == RunResult::Failed as i32)));
    }
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
    recorded.lock().await.stream_failure = StreamFailure::BeforeAck;
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

async fn run_to_completion(client: EventClient, run_started: bool) -> u64 {
    let bus = RunEventBus::new(16);
    let rx = bus.subscribe();
    if run_started {
        bus.publish(RunEvent::RunStarted { at_epoch_ms: 1 });
    }
    bus.publish(RunEvent::RunCompleted {
        at_epoch_ms: 2,
        exit_status: 0,
    });
    let (tx, shutdown) = tokio::sync::oneshot::channel();
    tx.send(()).unwrap();
    tokio::time::timeout(
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
    .unwrap()
}

#[tokio::test]
async fn a_renewed_token_is_used_when_the_old_one_expires_mid_run() {
    let (channel, recorded) = start_server().await;
    recorded.lock().await.expired_bearer = Some("Bearer old-token".to_string());
    let client = EventClient::authenticated_with_project_id(
        channel,
        TransportConfig {
            run_id: "expiring".into(),
            ..Default::default()
        },
        "old-token",
        "some-project-id",
    )
    .unwrap()
    .with_token_provider(|| async { Some("new-token".to_string()) });

    assert_eq!(run_to_completion(client, true).await, 4);

    let recorded = recorded.lock().await;
    assert_eq!(recorded.expected_next_seq, 4);
    let bearers = &recorded.authorizations;
    assert_eq!(
        bearers.first().map(String::as_str),
        Some("Bearer old-token")
    );
    assert_eq!(bearers.last().map(String::as_str), Some("Bearer new-token"));
    assert!(
        bearers.iter().filter(|b| *b == "Bearer new-token").count() >= 2,
        "the recovery probe and the reopened stream both use the renewed token: {bearers:?}"
    );
    assert!(recorded
        .projects
        .iter()
        .all(|project| project == "some-project-id"));
}

#[tokio::test]
async fn the_old_token_is_kept_when_renewal_yields_nothing_usable() {
    for renewed in [None, Some("bad\nvalue".to_string()), Some(String::new())] {
        let (channel, recorded) = start_server().await;
        recorded.lock().await.stream_failure = StreamFailure::BeforeAck;
        let client = EventClient::authenticated(
            channel,
            TransportConfig {
                run_id: "keeps-old".into(),
                ..Default::default()
            },
            "old-token",
        )
        .unwrap()
        .with_token_provider(move || {
            let renewed = renewed.clone();
            async move { renewed }
        });

        assert_eq!(run_to_completion(client, true).await, 4);

        assert!(recorded
            .lock()
            .await
            .authorizations
            .iter()
            .all(|value| value == "Bearer old-token"));
    }
}

#[tokio::test]
async fn without_a_provider_the_token_never_changes() {
    let (channel, recorded) = start_server().await;
    recorded.lock().await.stream_failure = StreamFailure::BeforeAck;
    let client = EventClient::authenticated(
        channel,
        TransportConfig {
            run_id: "fixed".into(),
            ..Default::default()
        },
        "fixed-token",
    )
    .unwrap();

    assert_eq!(run_to_completion(client, true).await, 4);

    assert!(recorded
        .lock()
        .await
        .authorizations
        .iter()
        .all(|value| value == "Bearer fixed-token"));
}

#[tokio::test]
async fn renewal_is_not_attempted_before_the_first_connection() {
    let (channel, _) = start_server().await;
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = calls.clone();
    let client = EventClient::authenticated(channel, TransportConfig::default(), "token")
        .unwrap()
        .with_token_provider(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async { Some("renewed".to_string()) }
        });
    drop(client);

    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}

fn terminal_events(recorded: &RecordedRun) -> Vec<once_events_client::proto::RunCompleted> {
    use once_events_client::proto::run_event::Payload;
    recorded
        .batches
        .iter()
        .flat_map(|batch| &batch.events)
        .filter_map(|event| match &event.payload {
            Some(Payload::RunCompleted(completed)) => Some(completed.clone()),
            _ => None,
        })
        .collect()
}

fn epoch_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

#[tokio::test]
async fn cancellation_completes_the_run_as_cancelled_with_its_reason() {
    use once_events_client::proto::RunResult;
    let (channel, recorded) = start_server().await;
    let cancellation = once_events_client::RunCancellation::new();
    let client = EventClient::new(
        channel,
        TransportConfig {
            run_id: "cancelled".into(),
            ..Default::default()
        },
    )
    .with_cancellation(cancellation.clone());
    let bus = RunEventBus::new(16);
    let rx = bus.subscribe();
    bus.publish(RunEvent::RunStarted {
        at_epoch_ms: epoch_ms() - 1_500,
    });
    let (_tx, shutdown) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        client
            .run_with_reconnect(rx, shutdown, once_events_client::ReconnectPolicy::default())
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while recorded.lock().await.expected_next_seq < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    cancellation.cancel("SIGTERM");
    tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // The producer is still alive; its late completion must not add a second terminal.
    bus.publish(RunEvent::RunCompleted {
        at_epoch_ms: epoch_ms(),
        exit_status: 0,
    });

    let terminals = terminal_events(&*recorded.lock().await);
    assert_eq!(terminals.len(), 1, "{terminals:?}");
    assert_eq!(terminals[0].result, RunResult::Cancelled as i32);
    assert_eq!(terminals[0].cancellation_reason, "SIGTERM");
    assert!(terminals[0].wall_ms >= 1_500, "{}", terminals[0].wall_ms);
}

#[tokio::test]
async fn cancellation_keeps_a_completion_that_was_already_published() {
    use once_events_client::proto::RunResult;
    let (channel, recorded) = start_server().await;
    let cancellation = once_events_client::RunCancellation::new();
    let client = EventClient::new(
        channel,
        TransportConfig {
            run_id: "completed-then-cancelled".into(),
            ..Default::default()
        },
    )
    .with_cancellation(cancellation.clone());
    let bus = RunEventBus::new(16);
    let rx = bus.subscribe();
    bus.publish(RunEvent::RunStarted { at_epoch_ms: 1 });
    bus.publish(RunEvent::RunCompleted {
        at_epoch_ms: 2,
        exit_status: 0,
    });
    cancellation.cancel("SIGINT");
    let (_tx, shutdown) = tokio::sync::oneshot::channel();
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        client.run_with_reconnect(rx, shutdown, once_events_client::ReconnectPolicy::default()),
    )
    .await
    .unwrap()
    .unwrap();

    let terminals = terminal_events(&*recorded.lock().await);
    assert_eq!(terminals.len(), 1, "{terminals:?}");
    assert_eq!(terminals[0].result, RunResult::Succeeded as i32);
    assert!(terminals[0].cancellation_reason.is_empty());
}

#[tokio::test]
async fn cancellation_shortens_a_shutdown_drain_already_in_progress() {
    let (channel, recorded) = start_server().await;
    let cancellation = once_events_client::RunCancellation::new();
    let client = EventClient::new(
        channel,
        TransportConfig {
            run_id: "stalled-shutdown".into(),
            final_drain: std::time::Duration::from_secs(30),
            cancel_drain: std::time::Duration::from_millis(100),
            ..Default::default()
        },
    )
    .with_cancellation(cancellation.clone());
    recorded.lock().await.stall = true;
    let bus = RunEventBus::new(16);
    let rx = bus.subscribe();
    bus.publish(RunEvent::RunStarted { at_epoch_ms: 1 });
    bus.publish(RunEvent::RunCompleted {
        at_epoch_ms: 2,
        exit_status: 0,
    });
    let (tx, shutdown) = tokio::sync::oneshot::channel();
    tx.send(()).unwrap();
    let task = tokio::spawn(async move {
        client
            .run_with_reconnect(rx, shutdown, once_events_client::ReconnectPolicy::default())
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let cancelled_at = std::time::Instant::now();
    cancellation.cancel("SIGTERM");
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        result,
        Err(once_events_client::TransportError::DrainTimeout)
    ));
    assert!(cancelled_at.elapsed() < std::time::Duration::from_secs(1));
}

#[tokio::test]
async fn cancellation_before_the_run_started_sends_no_completion() {
    let (channel, recorded) = start_server().await;
    let cancellation = once_events_client::RunCancellation::new();
    let client = EventClient::new(
        channel,
        TransportConfig {
            run_id: "never-started".into(),
            ..Default::default()
        },
    )
    .with_cancellation(cancellation.clone());
    let bus = RunEventBus::new(16);
    let rx = bus.subscribe();
    cancellation.cancel("SIGINT");
    let (_tx, shutdown) = tokio::sync::oneshot::channel();
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        client.run_with_reconnect(rx, shutdown, once_events_client::ReconnectPolicy::default()),
    )
    .await
    .unwrap();
    drop(bus);

    assert!(recorded.lock().await.batches.is_empty());
}

#[tokio::test]
async fn a_completion_published_after_cancellation_reports_the_cancellation() {
    use once_events_client::proto::RunResult;
    let (channel, recorded) = start_server().await;
    let cancellation = once_events_client::RunCancellation::new();
    let client = EventClient::new(
        channel,
        TransportConfig {
            run_id: "cancelled-then-completed".into(),
            ..Default::default()
        },
    )
    .with_cancellation(cancellation.clone());
    let bus = RunEventBus::new(16);
    let rx = bus.subscribe();
    bus.publish(RunEvent::RunStarted { at_epoch_ms: 1 });
    // The producer finishes after the signal, for example because the child
    // process died of the same signal; that outcome is still the cancellation.
    cancellation.cancel("SIGINT");
    bus.publish(RunEvent::RunCompleted {
        at_epoch_ms: epoch_ms(),
        exit_status: 1,
    });
    let (_tx, shutdown) = tokio::sync::oneshot::channel();
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        client.run_with_reconnect(rx, shutdown, once_events_client::ReconnectPolicy::default()),
    )
    .await
    .unwrap()
    .unwrap();

    let terminals = terminal_events(&*recorded.lock().await);
    assert_eq!(terminals.len(), 1, "{terminals:?}");
    assert_eq!(terminals[0].result, RunResult::Cancelled as i32);
    assert_eq!(terminals[0].cancellation_reason, "SIGINT");
}

#[tokio::test]
async fn cancellation_cuts_a_reconnect_backoff_short() {
    use once_events_client::proto::RunResult;
    let (channel, recorded) = start_server().await;
    recorded.lock().await.stream_failure = StreamFailure::AfterAck;
    let cancellation = once_events_client::RunCancellation::new();
    let client = EventClient::new(
        channel,
        TransportConfig {
            run_id: "cancelled-in-backoff".into(),
            cancel_drain: std::time::Duration::from_secs(1),
            ..Default::default()
        },
    )
    .with_cancellation(cancellation.clone());
    let bus = RunEventBus::new(16);
    let rx = bus.subscribe();
    bus.publish(RunEvent::RunStarted { at_epoch_ms: 1 });
    let (_tx, shutdown) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        client
            .run_with_reconnect(
                rx,
                shutdown,
                once_events_client::ReconnectPolicy {
                    initial_backoff: std::time::Duration::from_secs(30),
                    ..Default::default()
                },
            )
            .await
    });
    // Wait for the stream to fail after its first acknowledgement.
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while recorded.lock().await.expected_next_seq < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    cancellation.cancel("SIGTERM");
    tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let terminals = terminal_events(&*recorded.lock().await);
    assert_eq!(terminals.len(), 1, "{terminals:?}");
    assert_eq!(terminals[0].result, RunResult::Cancelled as i32);
}
