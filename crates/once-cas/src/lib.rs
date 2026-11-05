#![deny(missing_docs)]

//! Content-addressed storage and action-result caches.
//!
//! Blobs are addressed by their BLAKE3 digest; action results are keyed by an
//! action digest supplied by the caller. [`Cas`] is the local filesystem store,
//! while [`CacheProvider`] composes it with an optional remote cache.
//!
//! Locally produced writes synchronize their contents before atomic rename and
//! then synchronize the parent directory. Remote mirrors synchronize their
//! contents before an atomic rename but may skip the directory sync because
//! they can be fetched again. Blobs read back for restoration are hashed while
//! they are copied, and a blob that fails that check is removed rather than
//! restored.

mod blob;
mod budget;
mod digest;
mod error;
mod filesystem;
mod gc;
mod integrity;
mod model;
mod provider;
mod stats;
mod store;
pub mod transfer;
mod tuist;

pub use budget::{
    TransferBudget, DEFAULT_TRANSFER_MEMORY_BYTES, DEFAULT_TRANSFER_STAGING_BYTES,
    MIN_TRANSFER_MEMORY_BYTES,
};
pub use digest::Digest;
pub use error::{Error, ErrorKind, Result};
pub use gc::GcReport;
pub use model::{ActionResult, Stats};
pub use provider::{ActionCacheHit, CacheProvider};
pub use store::{Cas, StagedBlob, StagedBlobReader};
pub use tuist::{
    Executed, ProjectCreateError, RemoteProject, TuistAuth, TuistAuthPrompt, TuistCacheConfig,
    TuistExecution, TuistProjects, DEFAULT_EXECUTION_TIMEOUT, TUIST_APP_OAUTH_CLIENT_ID,
    TUIST_OAUTH_CLIENT_ID_ENV,
};

/// Remote Execution API messages exchanged with execution services.
pub use bazel_remote_apis::build::bazel::remote::execution::v2 as reapi;
/// Well-known protobuf types [`reapi`] messages embed, such as `Duration`.
pub use bazel_remote_apis::google::protobuf as reapi_protobuf;
/// Protobuf encoding for [`reapi`] messages.
pub use prost::Message as ReapiMessage;
