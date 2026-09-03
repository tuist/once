//! Optional bridge from the local run event bus to the Tuist
//! ingest service.
//!
//! Enabled by setting `ONCE_EVENTS_ENDPOINT` in the environment. The
//! endpoint is a fully-qualified gRPC URI (e.g. `https://build.tuist.dev`).
//! When enabled, a background task subscribes to the RFC 0008 event
//! bus and drives an [`once_events_client::EventClient`] against the
//! endpoint. Failures are logged and never abort the run.

use std::env;
use std::time::Duration;

use once_events_client::{EventClient, TransportConfig, TransportError};
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;
use tonic::transport::{Channel, Uri};
use tracing::{debug, warn};

use crate::commands::ui::UiServer;

const ENDPOINT_ENV: &str = "ONCE_EVENTS_ENDPOINT";

/// Handle to a running event-client task.
pub struct EventClientHandle {
    shutdown: Option<oneshot::Sender<()>>,
    handle: JoinHandle<Result<u64, TransportError>>,
}

impl EventClientHandle {
    /// Signal shutdown and await the task with a bounded timeout.
    /// Never returns an error; failures are logged.
    pub async fn shutdown_with_timeout(mut self, timeout: Duration) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        match tokio::time::timeout(timeout, &mut self.handle).await {
            Ok(Ok(Ok(_))) => debug!("event client drained cleanly"),
            Ok(Ok(Err(error))) => warn!(%error, "event client returned error on drain"),
            Ok(Err(error)) => warn!(%error, "event client task panicked"),
            Err(_) => {
                warn!("event client drain deadline elapsed; aborting");
                self.handle.abort();
            }
        }
    }
}

/// Try to start an event client if the environment configures one.
/// Returns `Ok(None)` when disabled (no env var); `Ok(Some(handle))`
/// when a task was spawned; `Err` for a hard startup failure (which
/// callers should log and ignore rather than abort the run for).
pub async fn try_start(
    ui_server: &UiServer,
    run_id: String,
) -> Result<Option<EventClientHandle>, StartError> {
    let Ok(endpoint) = env::var(ENDPOINT_ENV) else {
        return Ok(None);
    };
    let uri: Uri = endpoint.parse().map_err(|_| StartError::InvalidEndpoint)?;
    let channel = Channel::builder(uri)
        .connect()
        .await
        .map_err(StartError::Connect)?;

    let bus_rx: broadcast::Receiver<once_core::RunEvent> = ui_server.subscribe_events();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let config = TransportConfig {
        run_id,
        ..TransportConfig::default()
    };
    let client = EventClient::new(channel, config);
    let handle = tokio::spawn(async move { client.run_until_shutdown(bus_rx, shutdown_rx).await });
    Ok(Some(EventClientHandle {
        shutdown: Some(shutdown_tx),
        handle,
    }))
}

/// Errors that prevent starting the event client. Callers log and
/// continue; the run itself never fails because ingest cannot start.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("ONCE_EVENTS_ENDPOINT is not a valid gRPC URI")]
    InvalidEndpoint,
    #[error("failed to connect to events endpoint: {0}")]
    Connect(#[from] tonic::transport::Error),
}
