//! gRPC client for the Once live run event protocol.
//!
//! Implements the client half of `once.events.v1.RunEventService`.
//! The protocol is specified in `rfcs/0008-live-run-event-protocol.md`;
//! this crate owns the wire types (compiled from the `.proto` file
//! shipped alongside it), the ring buffer with reserved terminal
//! capacity, the mutable loss interval set that produces the batch's
//! `gap_advances` control records, the four-case batch acceptance
//! logic that drives the client's reaction to acknowledgements, and
//! the bounded final drain.
//!
//! The types map from the internal [`once_core::RunEvent`] bus onto
//! the wire proto. Server-side ingest and the projector live in the
//! Tuist server repository.

/// Generated Rust bindings for the wire proto.
#[allow(
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    unreachable_pub,
    missing_docs
)]
pub mod proto {
    tonic::include_proto!("once.events.v1");
}

mod bridge;
mod buffer;
mod loss;
mod session;
mod transport;

pub use bridge::{heartbeat_payload, translate, Translated};
pub use buffer::{PendingEvent, RingBuffer, RingPushOutcome};
pub use loss::{LossIntervals, LossPushOutcome};
pub use session::{AckAction, AckDisposition, EventSession, SessionLimits};
pub use transport::{EventClient, ReconnectPolicy, TransportConfig, TransportError};
