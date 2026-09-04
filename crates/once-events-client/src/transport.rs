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
use tonic::transport::Channel;
use tonic::{Request, Streaming};

use crate::bridge::{translate, Translated};
use crate::proto::run_event_service_client::RunEventServiceClient;
use crate::proto::{BatchAck, GetServerCapabilitiesRequest, RunEventBatch, ServerCapabilities};
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
    #[error("server rejected batch as invalid; client bug")]
    InvalidBatchRejected,
    #[error("server dropped the ack stream before drain completed")]
    AckStreamClosed,
}

/// Live transport bound to a specific gRPC channel and run.
pub struct EventClient {
    client: RunEventServiceClient<Channel>,
    config: TransportConfig,
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
            client: RunEventServiceClient::new(channel),
            config,
        }
    }

    /// One-shot preflight against the server.
    pub async fn capabilities(&mut self) -> Result<ServerCapabilities, TransportError> {
        let response = self
            .client
            .get_server_capabilities(Request::new(GetServerCapabilitiesRequest {}))
            .await?;
        Ok(response.into_inner())
    }

    /// Drive one run to completion using a bus subscription the
    /// caller supplies. The caller must have created the receiver
    /// via `bus.subscribe()` before publishing anything, so no
    /// events are missed to a subscribe-after-publish race. See
    /// [`Self::run_with_bus`] for the convenience wrapper.
    pub async fn run(self, bus_rx: broadcast::Receiver<CoreEvent>) -> Result<u64, TransportError> {
        self.run_impl(bus_rx, None).await
    }

    /// Subscribe to `bus` synchronously and drive one run to
    /// completion. Safe from subscribe-after-publish because the
    /// subscription is created before this method's returned future
    /// is polled.
    pub async fn run_with_bus(self, bus: RunEventBus) -> Result<u64, TransportError> {
        let rx = bus.subscribe();
        drop(bus);
        self.run_impl(rx, None).await
    }

    /// Drive one run to completion with automatic reconnect on
    /// transport-level stream failures. On each break the client
    /// calls `GetRunAck` on a fresh unary call to reconcile the
    /// server's durable state, then re-opens `PublishRunEvents` and
    /// resumes from the mirror. Ring buffer and loss intervals
    /// survive across reconnects because they live in the caller-
    /// owned session state.
    ///
    /// The session is threaded across attempts so events accumulate
    /// during the backoff window; producers keep publishing through
    /// the outage without observing it.
    pub async fn run_with_reconnect(
        mut self,
        bus_rx: broadcast::Receiver<CoreEvent>,
        shutdown: oneshot::Receiver<()>,
        policy: ReconnectPolicy,
    ) -> Result<u64, TransportError> {
        let mut session = EventSession::new(self.config.run_id.clone(), self.config.limits);
        let mut bus_rx = bus_rx;
        let mut shutdown = Some(shutdown);
        let mut attempt: u32 = 0;
        let mut backoff = policy.initial_backoff;
        loop {
            let result = self
                .run_session(&mut session, &mut bus_rx, shutdown.take())
                .await;
            match result {
                Ok(seq) => return Ok(seq),
                Err(fatal @ TransportError::InvalidBatchRejected) => {
                    return Err(fatal);
                }
                Err(recoverable) => {
                    attempt += 1;
                    if attempt >= policy.max_attempts {
                        tracing::error!(
                            attempt,
                            "event ingest reached max reconnect attempts; giving up: {recoverable}"
                        );
                        return Err(recoverable);
                    }
                    tracing::warn!(
                        attempt,
                        backoff_ms = backoff.as_millis(),
                        "event ingest stream broke: {recoverable}; reconnecting"
                    );
                    sleep(backoff).await;
                    backoff = (backoff * 2).min(policy.max_backoff);
                    // Re-sync via GetRunAck so the client's mirror
                    // matches whatever the server actually has.
                    if let Ok(ack) = self
                        .client
                        .get_run_ack(Request::new(crate::proto::GetRunAckRequest {
                            run_id: session.run_id().to_string(),
                        }))
                        .await
                        .map(tonic::Response::into_inner)
                    {
                        session.handle_get_run_ack(&ack);
                    }
                }
            }
        }
    }

    async fn run_session(
        &mut self,
        session: &mut EventSession,
        bus_rx: &mut broadcast::Receiver<CoreEvent>,
        mut shutdown: Option<oneshot::Receiver<()>>,
    ) -> Result<u64, TransportError> {
        let (batch_tx, batch_rx) = mpsc::channel::<RunEventBatch>(32);
        let outgoing = ReceiverStream::new(batch_rx);
        let mut ack_stream: Streaming<BatchAck> = self
            .client
            .publish_run_events(Request::new(outgoing))
            .await?
            .into_inner();
        let mut last_flush = Instant::now();
        let mut bus_open = true;
        let mut last_expected_next_seq = session.server_expected_next_seq();
        loop {
            let should_flush_now =
                session_should_flush(session, last_flush, self.config.batch_flush);
            if let Some(mut rx) = shutdown.take() {
                match rx.try_recv() {
                    Err(oneshot::error::TryRecvError::Empty) => shutdown = Some(rx),
                    Ok(()) | Err(oneshot::error::TryRecvError::Closed) => bus_open = false,
                }
            }
            tokio::select! {
                biased;
                maybe_ack = ack_stream.next() => match maybe_ack {
                    Some(Ok(ack)) => {
                        last_expected_next_seq = ack.expected_next_seq;
                        match session.handle_ack(&ack) {
                            AckAction::Continue { .. } | AckAction::StaleDropped => {}
                            AckAction::InvalidRejected => {
                                return Err(TransportError::InvalidBatchRejected);
                            }
                            AckAction::NeedsResync => {
                                let resync = self
                                    .client
                                    .get_run_ack(Request::new(crate::proto::GetRunAckRequest {
                                        run_id: session.run_id().to_string(),
                                    }))
                                    .await?
                                    .into_inner();
                                last_expected_next_seq = resync.expected_next_seq;
                                session.handle_get_run_ack(&resync);
                            }
                        }
                    }
                    Some(Err(status)) => return Err(TransportError::Grpc(status)),
                    None => {
                        if session.is_drained() && !bus_open {
                            return Ok(last_expected_next_seq);
                        }
                        return Err(TransportError::AckStreamClosed);
                    }
                },
                bus_msg = bus_rx.recv(), if bus_open => match bus_msg {
                    Ok(event) => enqueue_event(session, event),
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!(lagged_by = n, "bus receiver lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        bus_open = false;
                    }
                },
                () = sleep_until_or_immediate(should_flush_now, self.config.batch_flush) => {
                    if let Some(batch) = session.next_batch() {
                        if batch_tx.send(batch).await.is_err() {
                            return Err(TransportError::AckStreamClosed);
                        }
                        last_flush = Instant::now();
                    }
                    if !bus_open && session.is_drained() {
                        drop(batch_tx);
                        let deadline = self.config.final_drain;
                        return await_final_ack(&mut ack_stream, session, deadline)
                            .await
                            .map(|()| last_expected_next_seq);
                    }
                }
            }
        }
    }

    /// Like [`Self::run`] but also treats a fired shutdown signal
    /// as "no more events; drain and exit." Useful when the bus
    /// itself outlives the run (for example, a long-lived UI store
    /// that other publishers hold references to).
    pub async fn run_until_shutdown(
        self,
        bus_rx: broadcast::Receiver<CoreEvent>,
        shutdown: oneshot::Receiver<()>,
    ) -> Result<u64, TransportError> {
        self.run_impl(bus_rx, Some(shutdown)).await
    }

    async fn run_impl(
        mut self,
        mut bus_rx: broadcast::Receiver<CoreEvent>,
        mut shutdown: Option<oneshot::Receiver<()>>,
    ) -> Result<u64, TransportError> {
        let mut session = EventSession::new(self.config.run_id.clone(), self.config.limits);
        let (batch_tx, batch_rx) = mpsc::channel::<RunEventBatch>(32);
        let outgoing = ReceiverStream::new(batch_rx);
        let mut ack_stream: Streaming<BatchAck> = self
            .client
            .publish_run_events(Request::new(outgoing))
            .await?
            .into_inner();

        let mut last_flush = Instant::now();
        let mut bus_open = true;
        let mut last_expected_next_seq = 0;

        loop {
            let should_flush_now =
                session_should_flush(&session, last_flush, self.config.batch_flush);

            // Poll a taken shutdown receiver once per loop; a fired
            // signal is treated as "no more events, drain."
            if let Some(mut rx) = shutdown.take() {
                match rx.try_recv() {
                    Err(oneshot::error::TryRecvError::Empty) => {
                        shutdown = Some(rx);
                    }
                    Ok(()) | Err(oneshot::error::TryRecvError::Closed) => {
                        bus_open = false;
                    }
                }
            }

            tokio::select! {
                biased;
                // Deliver acks as fast as they arrive.
                maybe_ack = ack_stream.next() => match maybe_ack {
                    Some(Ok(ack)) => {
                        tracing::trace!(
                            acked_seq = ack.acked_seq,
                            expected_next = ack.expected_next_seq,
                            disposition = ack.disposition,
                            "batch ack",
                        );
                        last_expected_next_seq = ack.expected_next_seq;
                        match session.handle_ack(&ack) {
                            AckAction::Continue { .. } | AckAction::StaleDropped => {}
                            AckAction::InvalidRejected => {
                                return Err(TransportError::InvalidBatchRejected);
                            }
                            AckAction::NeedsResync => {
                                let resync = self
                                    .client
                                    .get_run_ack(Request::new(crate::proto::GetRunAckRequest {
                                        run_id: session.run_id().to_string(),
                                    }))
                                    .await?
                                    .into_inner();
                                last_expected_next_seq = resync.expected_next_seq;
                                session.handle_get_run_ack(&resync);
                            }
                        }
                    }
                    Some(Err(status)) => return Err(TransportError::Grpc(status)),
                    None => {
                        if session.is_drained() && !bus_open {
                            return Ok(last_expected_next_seq);
                        }
                        return Err(TransportError::AckStreamClosed);
                    }
                },
                // Consume bus events while the bus is open.
                bus_msg = bus_rx.recv(), if bus_open => match bus_msg {
                    Ok(event) => enqueue_event(&mut session, event),
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!(lagged_by = n, "bus receiver lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        bus_open = false;
                    }
                },
                // Periodic flush.
                () = sleep_until_or_immediate(should_flush_now, self.config.batch_flush) => {
                    if let Some(batch) = session.next_batch() {
                        tracing::trace!(
                            batch_id = %batch.batch_id,
                            seq_from = batch.seq_from,
                            events = batch.events.len(),
                            gaps = batch.gap_advances.len(),
                            "sending batch",
                        );
                        if batch_tx.send(batch).await.is_err() {
                            return Err(TransportError::AckStreamClosed);
                        }
                        last_flush = Instant::now();
                    }
                    if !bus_open && session.is_drained() {
                        drop(batch_tx);
                        // Await terminal ack up to the drain deadline.
                        let deadline = self.config.final_drain;
                        return await_final_ack(&mut ack_stream, &mut session, deadline).await
                            .map(|()| last_expected_next_seq);
                    }
                }
            }
        }
    }
}

fn enqueue_event(session: &mut EventSession, event: CoreEvent) {
    let mono_ns = 0; // Producers do not thread monotonic timing yet; RFC allows zero.
    match translate(event, mono_ns) {
        Translated::Ordinary {
            payload,
            epoch_ms,
            mono_ns,
        } => {
            session.push_ordinary(payload, epoch_ms, mono_ns);
        }
        Translated::Terminal {
            result,
            epoch_ms,
            mono_ns,
        } => {
            session.push_terminal(result, epoch_ms, mono_ns);
        }
        Translated::Skip => {}
    }
}

fn session_should_flush(session: &EventSession, last_flush: Instant, period: Duration) -> bool {
    // Immediate flush if we have any pending state and either the
    // period has elapsed or the session is finalized (drain hurry).
    let has_pending = session.next_seq() > 1 && !session.is_drained();
    let period_elapsed = last_flush.elapsed() >= period;
    let finalizing = session.finalized_locally();
    has_pending && (period_elapsed || finalizing)
}

async fn sleep_until_or_immediate(should_flush_now: bool, period: Duration) {
    if should_flush_now {
        // Yield to let other select! branches drain first, then return.
        tokio::task::yield_now().await;
    } else {
        sleep(period).await;
    }
}

async fn await_final_ack(
    ack_stream: &mut Streaming<BatchAck>,
    session: &mut EventSession,
    deadline: Duration,
) -> Result<(), TransportError> {
    let fut = async {
        while !session.is_drained() {
            match ack_stream.next().await {
                Some(Ok(ack)) => match session.handle_ack(&ack) {
                    AckAction::Continue { .. } | AckAction::StaleDropped => {}
                    AckAction::InvalidRejected => {
                        return Err(TransportError::InvalidBatchRejected);
                    }
                    AckAction::NeedsResync => {
                        // Best-effort: no time to renegotiate.
                        return Ok(());
                    }
                },
                Some(Err(status)) => return Err(TransportError::Grpc(status)),
                None => return Ok(()),
            }
        }
        Ok(())
    };
    match timeout(deadline, fut).await {
        Ok(res) => res,
        Err(_) => Ok(()), // Drain deadline elapsed; caller may inspect finalization state on projection.
    }
}
