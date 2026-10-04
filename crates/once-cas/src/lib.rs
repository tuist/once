#![deny(missing_docs)]

//! Content-addressed storage and action-result caches.
//!
//! Blobs are addressed by their BLAKE3 digest; action results are keyed by an
//! action digest supplied by the caller. [`Cas`] is the local filesystem store,
//! while [`CacheProvider`] composes it with an optional remote cache.
//!
//! Locally produced writes synchronize their contents before atomic rename and
//! then synchronize the parent directory. Remote mirrors retain atomic
//! visibility but may use weaker durability because they can be fetched again.

mod blob;
mod digest;
mod error;
mod filesystem;
mod gc;
mod model;
mod provider;
mod stats;
mod store;
mod tuist;

pub use digest::Digest;
pub use error::{Error, Result};
pub use gc::GcReport;
pub use model::{ActionResult, Stats};
pub use provider::CacheProvider;
pub use store::Cas;
pub use tuist::{
    ProjectCreateError, RemoteProject, TuistAuth, TuistAuthPrompt, TuistCacheConfig, TuistProjects,
    TUIST_APP_OAUTH_CLIENT_ID, TUIST_OAUTH_CLIENT_ID_ENV,
};
