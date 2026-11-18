//! Drains a child's piped stdout and stderr into the CAS.

use struo_cas::{CacheProvider, Digest};
use tokio::io::AsyncRead;
use tokio::process::{ChildStderr, ChildStdout};

use super::Capture;
use crate::stream::{self, ActionOutputObserver, Destination};
use crate::Result;

/// Capture a piped stream into the CAS, or resolve to `None` when the
/// stream was redirected to a file (and therefore not piped).
async fn capture_stream<R: AsyncRead + Unpin>(
    cache: &CacheProvider,
    pipe: Option<R>,
) -> Result<Option<Digest>> {
    match pipe {
        Some(pipe) => Ok(Some(cache.put_stream(pipe).await?)),
        None => Ok(None),
    }
}

/// Streaming counterpart of [`capture_stream`]: tee a piped stream to the
/// parent while capturing it, or resolve to `None` when redirected.
async fn capture_stream_streaming<R: AsyncRead + Unpin>(
    pipe: Option<R>,
    destination: Destination,
    cache: &CacheProvider,
) -> Result<Option<Digest>> {
    match pipe {
        Some(pipe) => Ok(Some(
            stream::to_cache(pipe, destination, cache, true).await?,
        )),
        None => Ok(None),
    }
}

async fn capture_stream_observed<R: AsyncRead + Unpin>(
    pipe: Option<R>,
    destination: Destination,
    cache: &CacheProvider,
    stream_to_parent: bool,
    observer: &dyn ActionOutputObserver,
) -> Result<Option<Digest>> {
    match pipe {
        Some(pipe) => Ok(Some(
            stream::to_cache_with_observer(
                pipe,
                destination,
                cache,
                stream_to_parent,
                Some(observer),
            )
            .await?,
        )),
        None => Ok(None),
    }
}

pub(super) async fn capture_pipes(
    capture: Capture<'_>,
    cache: &CacheProvider,
    stdout_pipe: Option<ChildStdout>,
    stderr_pipe: Option<ChildStderr>,
) -> Result<(Option<Digest>, Option<Digest>)> {
    match capture {
        Capture::Cached => tokio::try_join!(
            capture_stream(cache, stdout_pipe),
            capture_stream(cache, stderr_pipe)
        ),
        Capture::Streaming => tokio::try_join!(
            capture_stream_streaming(stdout_pipe, Destination::Stdout, cache),
            capture_stream_streaming(stderr_pipe, Destination::Stderr, cache)
        ),
        Capture::Observed {
            stream_to_parent,
            observer,
        } => tokio::try_join!(
            capture_stream_observed(
                stdout_pipe,
                Destination::Stdout,
                cache,
                stream_to_parent,
                observer
            ),
            capture_stream_observed(
                stderr_pipe,
                Destination::Stderr,
                cache,
                stream_to_parent,
                observer
            )
        ),
    }
}
