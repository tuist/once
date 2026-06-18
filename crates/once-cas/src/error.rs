use std::io;
use std::path::PathBuf;

use crate::Digest;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("corrupt action result at {0}: {1}")]
    Corrupt(PathBuf, serde_json::Error),
    #[error("blob not found: {0}")]
    BlobNotFound(Digest),
    #[error("cache provider `{provider}` is misconfigured: {message}")]
    InvalidConfig {
        provider: &'static str,
        message: String,
    },
    #[error("cache provider `{provider}` failed during `{operation}`: {message}")]
    Remote {
        provider: &'static str,
        operation: &'static str,
        message: String,
    },
}

pub type Result<T> = std::result::Result<T, Error>;
