use std::io;
use std::path::PathBuf;

use crate::Digest;

/// Everything the store and its providers can fail with.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filesystem operation failed, carrying the path it was on.
    #[error("io error at {path}: {source}")]
    Io {
        /// Path the failed operation targeted.
        path: PathBuf,
        /// Underlying operating system error.
        #[source]
        source: io::Error,
    },
    /// A stored action record could not be decoded, so the entry is
    /// treated as a miss rather than trusted.
    #[error("corrupt action result at {0}: {1}")]
    Corrupt(PathBuf, serde_json::Error),
    /// The requested blob is in neither the local store nor any
    /// configured remote tier.
    #[error("blob not found: {0}")]
    BlobNotFound(Digest),
    /// A provider was selected but its configuration is unusable.
    #[error("cache provider `{provider}` is misconfigured: {message}")]
    InvalidConfig {
        /// Provider that rejected its configuration.
        provider: &'static str,
        /// What was wrong with it.
        message: String,
    },
    /// A remote tier returned an error for a specific operation.
    #[error("cache provider `{provider}` failed during `{operation}`: {message}")]
    Remote {
        /// Provider that failed.
        provider: &'static str,
        /// Operation being attempted, such as `get_blob`.
        operation: &'static str,
        /// Message reported by the remote.
        message: String,
    },
}

/// Result alias for every fallible store operation.
pub type Result<T> = std::result::Result<T, Error>;
