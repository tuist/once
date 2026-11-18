use std::io::{self, Cursor};
use std::time::Duration;

use bazel_remote_apis::google::bytestream::ReadResponse;
use futures::stream;
use tokio_util::io::StreamReader;
use tonic::{Status, Streaming};

use super::{retry, stall};
use crate::{Cas, Digest, Error};

pub(super) async fn store<G: Send + 'static>(
    cache: &Cas,
    stream: Streaming<ReadResponse>,
    timeout: Duration,
    expected: u64,
    compressed: bool,
    verification: (Digest, String),
    guard: G,
) -> std::result::Result<Digest, retry::Failure> {
    let (digest, sha256) = verification;
    let limit = if compressed {
        super::stream_limits::encoded_bytes(expected)
    } else {
        expected
    };
    let chunks = stream::try_unfold((stream, 0_u64), move |(mut stream, received)| async move {
        let message = stall::next_message(&mut stream, timeout)
            .await
            .map_err(io::Error::other)?;
        match message {
            Some(response) => {
                let received = received.saturating_add(response.data.len() as u64);
                if received > limit {
                    return Err(io::Error::other(EncodedLimit { compressed }));
                }
                Ok(Some((Cursor::new(response.data), (stream, received))))
            }
            None => Ok::<_, io::Error>(None),
        }
    });
    let reader = StreamReader::new(Box::pin(chunks));
    cache
        .mirror_remote_verified(
            reader,
            expected,
            compressed,
            Some(digest),
            Some(sha256),
            guard,
        )
        .await
        .map_err(read_error)
}

#[derive(Debug, thiserror::Error)]
#[error("remote blob body exceeds its admitted byte allowance")]
struct EncodedLimit {
    compressed: bool,
}

fn read_error(error: Error) -> retry::Failure {
    if Cas::remote_window_limit_exceeded(&error) {
        return error.into();
    }
    if let Error::Io { source, path } = &error {
        if let Some(limit) = source
            .get_ref()
            .and_then(|source| source.downcast_ref::<EncodedLimit>())
        {
            let kind = if limit.compressed {
                crate::ErrorKind::Exhausted
            } else {
                crate::ErrorKind::Corrupt
            };
            return super::status::remote(
                "get blob",
                kind,
                "remote blob body exceeds its admitted byte allowance",
            )
            .into();
        }
        if matches!(
            source.kind(),
            io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof
        ) && !error.is_corrupt()
        {
            return Error::corrupt_blob(path.clone(), Digest::of_bytes(&[]), &source.to_string())
                .into();
        }
        if let Some(status) = source
            .get_ref()
            .and_then(|source| source.downcast_ref::<Status>())
        {
            return retry::Failure::Rpc(status.clone());
        }
    }
    error.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::Code;

    #[test]
    fn transport_status_survives_native_store_io_errors() {
        let status =
            Status::with_details(Code::Unavailable, "interrupted", b"details".to_vec().into());
        let error = Error::Io {
            path: "scratch".into(),
            source: io::Error::other(status),
        };
        let retry::Failure::Rpc(status) = read_error(error) else {
            panic!("transport status was lost");
        };
        assert_eq!(status.code(), Code::Unavailable);
        assert_eq!(status.message(), "interrupted");
        assert_eq!(status.details(), b"details");
        assert!(retry::retryable(&status));
    }

    #[test]
    fn filesystem_errors_keep_their_local_path_and_kind() {
        let error = Error::Io {
            path: "scratch".into(),
            source: io::ErrorKind::PermissionDenied.into(),
        };
        let retry::Failure::Local(Error::Io { path, source }) = read_error(error) else {
            panic!("filesystem error was lost");
        };
        assert_eq!(path, std::path::Path::new("scratch"));
        assert_eq!(source.kind(), io::ErrorKind::PermissionDenied);
    }
}
