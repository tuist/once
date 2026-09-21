//! Live gRPC transport that drives an [`EventSession`] over the
//! wire.
//!
//! Subscribes to an [`once_core::RunEventBus`], translates each
//! internal event into a wire payload via [`crate::bridge`], feeds
//! the session, and pushes batches through a bidirectional
//! `PublishRunEvents` stream. Server acks flow back through the
//! session's ack handler.
//!
//! Backpressure and cadence are driven by two triggers: a periodic
//! flush timer, and an immediate flush on any "significant" event
//! (terminal, `TargetCompleted`, first log chunk after quiet). The
//! transport shuts down gracefully once the bus is closed and the
//! session drains, honouring the RFC's bounded final drain deadline.

use std::time::Duration;

use once_core::{RunEvent as CoreEvent, RunEventBus};
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::{sleep, timeout, Instant};
use tokio_stream::{wrappers::ReceiverStream, StreamExt};
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::{interceptor::InterceptedService, Interceptor};
use tonic::transport::Channel;
use tonic::Request;

use crate::bridge::{translate, Translated};
use crate::proto::run_event_service_client::RunEventServiceClient;
use crate::proto::{GetServerCapabilitiesRequest, RunEventBatch, ServerCapabilities};
use crate::session::{AckAction, EventSession, SessionLimits};

/// Configuration for a live transport.
#[derive(Clone, Debug)]
pub struct TransportConfig {
    pub run_id: String,
    pub batch_flush: Duration,
    pub final_drain: Duration,
    pub limits: SessionLimits,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            run_id: String::new(),
            batch_flush: Duration::from_millis(150),
            final_drain: Duration::from_secs(2),
            limits: SessionLimits::default(),
        }
    }
}

/// Errors surfaced to callers.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("gRPC transport error: {0}")]
    Grpc(#[from] tonic::Status),
    #[error("gRPC connection error: {0}")]
    Connect(#[from] tonic::transport::Error),
    #[error("event service returned an invalid batch rejection or acknowledgement")]
    InvalidBatchRejected,
    #[error("server dropped the ack stream before drain completed")]
    AckStreamClosed,
    #[error("event delivery did not finish within the final drain deadline")]
    DrainTimeout,
    #[error("server returned an invalid or expired argument hash key")]
    InvalidHashKey,
    #[error("event service preflight timed out")]
    PreflightTimeout,
    #[error("event service does not support Once event version 1.0 or its required limits")]
    UnsupportedCapabilities,
    #[error("event exceeds the negotiated wire size limit")]
    EventTooLarge,
}

/// Live transport bound to a specific gRPC channel and run.
pub struct EventClient {
    client: RunEventServiceClient<InterceptedService<Channel, Authorization>>,
    config: TransportConfig,
    metadata: Option<crate::proto::RunStarted>,
    started_at_ms: Option<i64>,
    dashboard: crate::dashboard::DashboardLink,
}

#[derive(Clone, Default)]
struct Authorization(Option<MetadataValue<Ascii>>);

impl Interceptor for Authorization {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, tonic::Status> {
        if let Some(value) = &self.0 {
            request
                .metadata_mut()
                .insert("authorization", value.clone());
        }
        Ok(request)
    }
}

/// Reconnect strategy configuration.
#[derive(Clone, Debug)]
pub struct ReconnectPolicy {
    /// Initial backoff after the first failure.
    pub initial_backoff: Duration,
    /// Backoff cap; each attempt doubles the previous backoff up to
    /// this value.
    pub max_backoff: Duration,
    /// Give up after this many consecutive failures.
    pub max_attempts: u32,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_backoff: Duration::from_millis(200),
            max_backoff: Duration::from_secs(5),
            max_attempts: 6,
        }
    }
}

impl EventClient {
    /// Construct a client from a shared tonic channel. The caller is
    /// responsible for TLS, auth interceptors, and connection reuse.
    pub fn new(channel: Channel, config: TransportConfig) -> Self {
        Self {
            client: RunEventServiceClient::with_interceptor(channel, Authorization::default()),
            config,
            metadata: None,
            started_at_ms: None,
            dashboard: crate::dashboard::DashboardLink::default(),
        }
    }

    pub fn authenticated(
        channel: Channel,
        config: TransportConfig,
        token: &str,
    ) -> Result<Self, tonic::metadata::errors::InvalidMetadataValue> {
        let mut authorization: MetadataValue<Ascii> = format!("Bearer {token}").parse()?;
        authorization.set_sensitive(true);
        Ok(Self {
            client: RunEventServiceClient::with_interceptor(
                channel,
                Authorization(Some(authorization)),
            ),
            config,
            metadata: None,
            started_at_ms: None,
            dashboard: crate::dashboard::DashboardLink::default(),
        })
    }

    /// Report a validated dashboard link once, after durable run creation.
    /// Servers may omit it without affecting delivery.
    #[must_use]
    pub fn with_dashboard_link(mut self, handler: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.dashboard = crate::dashboard::DashboardLink::new(Box::new(handler));
        self
    }

    #[must_use]
    pub fn with_metadata(mut self, metadata: crate::proto::RunStarted) -> Self {
        self.metadata = Some(metadata);
        self
    }

    pub async fn argv_hash_key(
        &mut self,
        project_id: String,
    ) -> Result<crate::proto::ArgvHashKey, TransportError> {
        let key = self
            .client
            .get_argv_hash_key(crate::proto::GetArgvHashKeyRequest { project_id })
            .await?
            .into_inner();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        if key.key_bytes.len() != 32
            || key.key_id.is_empty()
            || i128::from(key.expires_at_epoch_ms) <= i128::try_from(now_ms).unwrap_or(i128::MAX)
        {
            return Err(TransportError::InvalidHashKey);
        }
        Ok(key)
    }

    /// One-shot preflight against the server.
    pub async fn capabilities(&mut self) -> Result<ServerCapabilities, TransportError> {
        let response = self
            .client
            .get_server_capabilities(Request::new(GetServerCapabilitiesRequest {}))
            .await?;
        Ok(response.into_inner())
    }

    pub async fn run(self, bus_rx: broadcast::Receiver<CoreEvent>) -> Result<u64, TransportError> {
        self.drive(
            bus_rx,
            None,
            ReconnectPolicy {
                max_attempts: 1,
                ..Default::default()
            },
        )
        .await
    }

    /// Subscribe before returning the future, so publication can begin immediately.
    pub fn run_with_bus(
        self,
        bus: RunEventBus,
    ) -> impl std::future::Future<Output = Result<u64, TransportError>> {
        let rx = bus.subscribe();
        drop(bus);
        self.run(rx)
    }

    pub async fn run_with_reconnect(
        self,
        bus_rx: broadcast::Receiver<CoreEvent>,
        shutdown: oneshot::Receiver<()>,
        policy: ReconnectPolicy,
    ) -> Result<u64, TransportError> {
        self.drive(bus_rx, Some(shutdown), policy).await
    }

    pub async fn run_until_shutdown(
        self,
        bus_rx: broadcast::Receiver<CoreEvent>,
        shutdown: oneshot::Receiver<()>,
    ) -> Result<u64, TransportError> {
        self.drive(
            bus_rx,
            Some(shutdown),
            ReconnectPolicy {
                max_attempts: 1,
                ..Default::default()
            },
        )
        .await
    }

    async fn drive(
        mut self,
        mut bus_rx: broadcast::Receiver<CoreEvent>,
        shutdown: Option<oneshot::Receiver<()>>,
        policy: ReconnectPolicy,
    ) -> Result<u64, TransportError> {
        let capabilities = self.capabilities().await?;
        self.apply_capabilities(&capabilities)?;
        let stopping = tokio_util::sync::CancellationToken::new();
        let final_drain = self.config.final_drain;
        let stop = stopping.clone();
        let delivery = async {
            let mut session = EventSession::new(self.config.run_id.clone(), self.config.limits);
            let mut backoff = policy.initial_backoff;
            for attempt in 0..policy.max_attempts.max(1) {
                if attempt > 0 {
                    // Keep translating during outages, using the session's bounded ring.
                    let delay = sleep(backoff);
                    tokio::pin!(delay);
                    loop {
                        tokio::select! {
                            () = &mut delay => break,
                            event = bus_rx.recv() => match event {
                                Ok(event) => enqueue_event(&mut session, event, self.metadata.as_ref(), &mut self.started_at_ms),
                                Err(broadcast::error::RecvError::Lagged(n)) => {
                                    session.record_producer_loss(n);
                                    tracing::warn!(lagged_by = n, "run event bus lost events");
                                },
                                Err(broadcast::error::RecvError::Closed) => break,
                            }
                        }
                    }
                    backoff = (backoff * 2).min(policy.max_backoff);
                    if let Ok(ack) = self
                        .client
                        .get_run_ack(crate::proto::GetRunAckRequest {
                            run_id: session.run_id().to_string(),
                        })
                        .await
                    {
                        let ack = ack.into_inner();
                        if session.handle_get_run_ack(&ack) == AckAction::InvalidRejected {
                            return Err(TransportError::InvalidBatchRejected);
                        }
                        if ack.acked_seq >= 1 {
                            self.dashboard.accepted(&ack.dashboard_url);
                        }
                        if session.finalized_locally() && session.is_drained() {
                            return Ok(session.server_expected_next_seq());
                        }
                    }
                }
                let result = self.run_session(&mut session, &mut bus_rx, &stopping).await;
                match result {
                    Ok(seq) => return Ok(seq),
                    Err(
                        error @ (TransportError::InvalidBatchRejected
                        | TransportError::DrainTimeout),
                    ) => return Err(error),
                    Err(error) if attempt + 1 >= policy.max_attempts.max(1) => return Err(error),
                    Err(error) => {
                        tracing::warn!(attempt, %error, "event stream failed; reconnecting");
                    }
                }
            }
            unreachable!()
        };
        tokio::pin!(delivery);
        let shutdown = async {
            match shutdown {
                Some(rx) => {
                    let _ = rx.await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            result = &mut delivery => result,
            () = shutdown => {
                stop.cancel();
                timeout(final_drain, &mut delivery).await.map_err(|_| TransportError::DrainTimeout)?
            }
        }
    }

    fn apply_capabilities(&mut self, caps: &ServerCapabilities) -> Result<(), TransportError> {
        if !caps
            .supported_protocol_versions
            .iter()
            .any(|version| version == "1.0")
            || !caps.required_features.is_empty()
            || caps.max_batch_bytes < 1024
            || caps.max_event_bytes < 512
            || caps.max_unacked_events == 0
            || caps.max_log_chunk_bytes == 0
        {
            return Err(TransportError::UnsupportedCapabilities);
        }
        let limits = &mut self.config.limits;
        limits.max_batch_bytes = limits.max_batch_bytes.min(caps.max_batch_bytes as usize);
        limits.max_event_bytes = limits
            .max_event_bytes
            .min(caps.max_event_bytes as usize)
            .min(limits.max_batch_bytes.saturating_sub(256));
        limits.max_log_chunk_bytes = limits
            .max_log_chunk_bytes
            .min(caps.max_log_chunk_bytes as usize)
            .min(limits.max_event_bytes.saturating_sub(128));
        limits.ordinary_capacity = limits
            .ordinary_capacity
            .min(caps.max_unacked_events as usize);
        limits.max_events_per_batch = limits.max_events_per_batch.min(limits.ordinary_capacity);
        if limits.max_event_bytes < 256
            || limits.max_log_chunk_bytes == 0
            || limits.max_unacked_bytes < limits.max_event_bytes
        {
            return Err(TransportError::UnsupportedCapabilities);
        }
        if let Some(metadata) = &mut self.metadata {
            if !metadata.safe_literal_allowlist_version.is_empty()
                && metadata.safe_literal_allowlist_version != caps.safe_literal_allowlist_version
            {
                return Err(TransportError::UnsupportedCapabilities);
            }
            metadata.protocol_version = "1.0".into();
            metadata.effective_limits = Some(crate::proto::EffectiveLimits {
                max_batch_bytes: u32::try_from(limits.max_batch_bytes).unwrap_or(u32::MAX),
                max_event_bytes: u32::try_from(limits.max_event_bytes).unwrap_or(u32::MAX),
                max_unacked_events: u32::try_from(limits.ordinary_capacity).unwrap_or(u32::MAX),
                max_log_chunk_bytes: u32::try_from(limits.max_log_chunk_bytes).unwrap_or(u32::MAX),
                log_ingestion_enabled: caps.log_ingestion_available,
                raw_event_retention_enabled: caps.raw_event_retention_available,
            });
        }
        Ok(())
    }

    async fn run_session(
        &mut self,
        session: &mut EventSession,
        bus_rx: &mut broadcast::Receiver<CoreEvent>,
        stopping: &tokio_util::sync::CancellationToken,
    ) -> Result<u64, TransportError> {
        let (batch_tx, batch_rx) = mpsc::channel::<RunEventBatch>(1);
        let mut batch_id = prime_stream(
            session,
            bus_rx,
            self.metadata.as_ref(),
            &mut self.started_at_ms,
            &batch_tx,
            self.config.limits,
        )
        .await?;
        let mut ack_stream = self
            .client
            .publish_run_events(ReceiverStream::new(batch_rx))
            .await?
            .into_inner();
        let mut in_flight = true;
        let mut bus_open = true;
        let mut drain_deadline = None;
        let mut next_flush = Instant::now() + self.config.batch_flush;
        let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        heartbeat.tick().await;
        loop {
            if stopping.is_cancelled() && bus_open {
                drain_bus(
                    session,
                    bus_rx,
                    self.metadata.as_ref(),
                    &mut self.started_at_ms,
                );
                bus_open = false;
            }
            if !bus_open || session.finalized_locally() {
                drain_deadline.get_or_insert_with(|| Instant::now() + self.config.final_drain);
            }
            if !bus_open && session.is_drained() {
                return Ok(session.server_expected_next_seq());
            }
            let deadline = async {
                match drain_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                () = deadline => return Err(TransportError::DrainTimeout),
                () = stopping.cancelled(), if bus_open => {},
                _ = heartbeat.tick(), if bus_open && !session.finalized_locally() => {
                    let epoch_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
                        .unwrap_or_default();
                    session.push_ordinary(crate::bridge::heartbeat_payload(), epoch_ms, 0);
                },
                ack = ack_stream.next() => match ack {
                    Some(Ok(ack)) => {
                        if !in_flight || ack.batch_id != batch_id {
                            return Err(TransportError::InvalidBatchRejected);
                        }
                        in_flight = false;
                        match session.handle_ack(&ack) {
                            AckAction::Continue { retry_after_ms } => {
                                if ack.acked_seq >= 1 { self.dashboard.accepted(&ack.dashboard_url); }
                                next_flush = Instant::now() + Duration::from_millis(u64::from(retry_after_ms));
                            }
                            AckAction::StaleDropped => next_flush = Instant::now(),
                            AckAction::InvalidRejected => return Err(TransportError::InvalidBatchRejected),
                            AckAction::NeedsResync => {
                                // Reopen the stream before resending, fencing old responses.
                                return Err(TransportError::Grpc(tonic::Status::aborted("event stream needs resynchronization")));
                            }
                        }
                    }
                    Some(Err(status)) => return Err(status.into()),
                    None => return Err(TransportError::AckStreamClosed),
                },
                event = bus_rx.recv(), if bus_open => match event {
                    Ok(event) => enqueue_event(session, event, self.metadata.as_ref(), &mut self.started_at_ms),
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        session.record_producer_loss(n);
                        tracing::warn!(lagged_by = n, "run event bus lost events");
                    },
                    Err(broadcast::error::RecvError::Closed) => bus_open = false,
                },
                () = tokio::time::sleep_until(next_flush), if !in_flight => {
                    if let Some(batch) = session.next_batch() {
                        if prost::Message::encoded_len(&batch) > self.config.limits.max_batch_bytes
                            || batch.events.iter().any(|event| prost::Message::encoded_len(event) > self.config.limits.max_event_bytes) {
                            return Err(TransportError::EventTooLarge);
                        }
                        batch_id.clone_from(&batch.batch_id);
                        batch_tx.send(batch).await.map_err(|_| TransportError::AckStreamClosed)?;
                        in_flight = true;
                    }
                    next_flush = Instant::now() + self.config.batch_flush;
                }
            }
        }
    }
}

async fn prime_stream(
    session: &mut EventSession,
    bus_rx: &mut broadcast::Receiver<CoreEvent>,
    metadata: Option<&crate::proto::RunStarted>,
    started_at_ms: &mut Option<i64>,
    batch_tx: &mpsc::Sender<RunEventBatch>,
    limits: SessionLimits,
) -> Result<String, TransportError> {
    // Some servers send response headers only after receiving the first batch.
    loop {
        if let Some(batch) = session.next_batch() {
            if prost::Message::encoded_len(&batch) > limits.max_batch_bytes
                || batch
                    .events
                    .iter()
                    .any(|event| prost::Message::encoded_len(event) > limits.max_event_bytes)
            {
                return Err(TransportError::EventTooLarge);
            }
            let batch_id = batch.batch_id.clone();
            batch_tx
                .send(batch)
                .await
                .map_err(|_| TransportError::AckStreamClosed)?;
            return Ok(batch_id);
        }
        match bus_rx.recv().await {
            Ok(event) => enqueue_event(session, event, metadata, started_at_ms),
            Err(broadcast::error::RecvError::Lagged(n)) => session.record_producer_loss(n),
            Err(broadcast::error::RecvError::Closed) => {
                return Err(TransportError::AckStreamClosed);
            }
        }
    }
}

fn drain_bus(
    session: &mut EventSession,
    bus_rx: &mut broadcast::Receiver<CoreEvent>,
    metadata: Option<&crate::proto::RunStarted>,
    started_at_ms: &mut Option<i64>,
) {
    loop {
        match bus_rx.try_recv() {
            Ok(event) => enqueue_event(session, event, metadata, started_at_ms),
            Err(broadcast::error::TryRecvError::Lagged(n)) => session.record_producer_loss(n),
            Err(_) => break,
        }
    }
}

fn enqueue_event(
    session: &mut EventSession,
    event: CoreEvent,
    metadata: Option<&crate::proto::RunStarted>,
    started_at_ms: &mut Option<i64>,
) {
    if started_at_ms.is_none() && !matches!(event, CoreEvent::RunStarted { .. }) {
        return;
    }
    let mono_ns = 0; // Producers do not thread monotonic timing yet; RFC allows zero.
    match translate(event, mono_ns) {
        Translated::Ordinary {
            mut payload,
            epoch_ms,
            mono_ns,
        } => {
            if matches!(payload, crate::proto::run_event::Payload::RunStarted(_)) {
                *started_at_ms = Some(epoch_ms);
                if let Some(metadata) = metadata {
                    payload = crate::proto::run_event::Payload::RunStarted(metadata.clone());
                }
            }
            session.push_ordinary(payload, epoch_ms, mono_ns);
        }
        Translated::Terminal {
            mut result,
            epoch_ms,
            mono_ns,
        } => {
            if let Some(start) = started_at_ms {
                result.wall_ms = epoch_ms.saturating_sub(*start).max(0);
            }
            result.producer_dropped_events = session.producer_dropped_events();
            session.push_terminal(result, epoch_ms, mono_ns);
        }
        Translated::Skip => {}
    }
}
