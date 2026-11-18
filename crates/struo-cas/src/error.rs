use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::integrity::CorruptBlob;
use crate::Digest;

/// Everything the store and its providers can fail with.
///
/// Use [`kind`](Self::kind) rather than matching on variants to decide how to
/// react: only [`ErrorKind::NotFound`] is evidence that content is absent.
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
    /// A stored action record could not be decoded. It is reported rather
    /// than trusted or mistaken for a miss.
    #[error("corrupt action result at {0}: {1}")]
    Corrupt(PathBuf, serde_json::Error),
    /// The requested blob is confirmed absent from the local store and from
    /// every configured remote tier.
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
    /// A remote tier failed in a way the provider could classify.
    #[error("cache provider `{provider}` failed during `{operation}`: {message}")]
    RemoteFailure {
        /// Provider that failed.
        provider: &'static str,
        /// Operation being attempted, such as `get_blob`.
        operation: &'static str,
        /// What the failure says about the remote tier.
        kind: ErrorKind,
        /// Message reported by the remote.
        message: String,
    },
}

/// What a failure says about the cache, independent of where it happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The content is confirmed absent. This is the only kind that is a miss.
    NotFound,
    /// The cache could not be reached or did not answer in time. Retrying
    /// later may succeed; the content may still exist.
    Unavailable,
    /// The cache refused the request, for example because credentials were
    /// rejected or lack access.
    Refused,
    /// Content or a stored record failed its integrity check.
    Corrupt,
    /// The remote answered with something this client cannot interpret.
    Protocol,
    /// A provider configuration is unusable.
    Misconfigured,
    /// The work needs more than its configured budget can ever grant, such
    /// as a download larger than the whole staging allowance. Retrying with
    /// the same budget cannot succeed.
    Exhausted,
    /// A local filesystem operation failed.
    Io,
    /// A failure without a more specific classification.
    Other,
}

impl Error {
    /// Classify this error.
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::BlobNotFound(_) => ErrorKind::NotFound,
            Self::Corrupt(..) => ErrorKind::Corrupt,
            Self::Io { source, .. } if CorruptBlob::is(source) => ErrorKind::Corrupt,
            Self::Io { .. } => ErrorKind::Io,
            Self::InvalidConfig { .. } => ErrorKind::Misconfigured,
            Self::Remote { .. } => ErrorKind::Other,
            Self::RemoteFailure { kind, .. } => *kind,
        }
    }

    /// True only when the content is confirmed absent.
    pub fn is_not_found(&self) -> bool {
        self.kind() == ErrorKind::NotFound
    }

    /// True when the cache could not answer and the content may still exist.
    pub fn is_unavailable(&self) -> bool {
        self.kind() == ErrorKind::Unavailable
    }

    /// True when content or a stored record failed its integrity check.
    pub fn is_corrupt(&self) -> bool {
        self.kind() == ErrorKind::Corrupt
    }

    pub(crate) fn remote(
        provider: &'static str,
        operation: &'static str,
        kind: ErrorKind,
        message: impl Into<String>,
    ) -> Self {
        Self::RemoteFailure {
            provider,
            operation,
            kind,
            message: message.into(),
        }
    }

    pub(crate) fn corrupt_blob(path: PathBuf, digest: Digest, detail: &str) -> Self {
        Self::Io {
            path,
            source: CorruptBlob::io_error(digest, detail),
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotFound => "not found",
            Self::Unavailable => "unavailable",
            Self::Refused => "refused",
            Self::Corrupt => "corrupt",
            Self::Protocol => "protocol",
            Self::Misconfigured => "misconfigured",
            Self::Exhausted => "exhausted",
            Self::Io => "io",
            Self::Other => "other",
        })
    }
}

/// Result alias for every fallible store operation.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_confirmed_absence_is_not_found() {
        let digest = Digest::of_bytes(b"absent");
        assert!(Error::BlobNotFound(digest).is_not_found());
        for kind in [
            ErrorKind::Unavailable,
            ErrorKind::Refused,
            ErrorKind::Protocol,
            ErrorKind::Corrupt,
        ] {
            let error = Error::remote("tuist", "get blob", kind, "failed");
            assert_eq!(error.kind(), kind);
            assert!(!error.is_not_found());
        }
        let legacy = Error::Remote {
            provider: "tuist",
            operation: "get blob",
            message: "failed".into(),
        };
        assert_eq!(legacy.kind(), ErrorKind::Other);
    }

    #[test]
    fn corrupt_local_blobs_are_classified_through_their_io_source() {
        let digest = Digest::of_bytes(b"blob");
        let corrupt = Error::corrupt_blob(PathBuf::from("/cas/ab/cd"), digest, "hash mismatch");
        assert!(corrupt.is_corrupt());
        assert!(corrupt.to_string().contains(&digest.to_hex()));
        let plain = Error::Io {
            path: PathBuf::from("/cas"),
            source: io::Error::new(io::ErrorKind::InvalidData, "other"),
        };
        assert_eq!(plain.kind(), ErrorKind::Io);
    }

    #[test]
    fn classified_remote_failures_render_like_legacy_remote_errors() {
        let classified = Error::remote("tuist", "get blob", ErrorKind::Unavailable, "offline");
        let legacy = Error::Remote {
            provider: "tuist",
            operation: "get blob",
            message: "offline".into(),
        };
        assert_eq!(classified.to_string(), legacy.to_string());
    }
}
