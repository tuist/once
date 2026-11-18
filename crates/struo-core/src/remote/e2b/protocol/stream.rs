use std::collections::BTreeMap;

use base64::Engine;
use struo_cas::{ActionResult, CacheProvider};
use tokio::io::{AsyncRead, AsyncWriteExt};

use super::frames::{io_error, protocol_error, FrameReader};
use super::StartResponse;
use crate::executor::staging::CapturePermit;
use crate::remote::http::check_size;
use crate::stream::{self, Destination, PIPE_CAPACITY};
use crate::{Error, RemoteIoLimits, Result};

#[allow(clippy::too_many_lines)]
pub(in crate::remote::e2b) async fn capture_command<R: AsyncRead + Unpin>(
    reader: R,
    cache: &CacheProvider,
    stream_to_parent: bool,
    limits: RemoteIoLimits,
    budget: Option<CapturePermit>,
) -> Result<ActionResult> {
    let (stdout_reader, mut stdout) = tokio::io::duplex(PIPE_CAPACITY);
    let (stderr_reader, mut stderr) = tokio::io::duplex(PIPE_CAPACITY);
    let decode = async {
        let mut frames = FrameReader::new(reader, limits);
        let mut exit_code = None;
        let mut captured = 0_u64;
        while let Some(payload) = frames.next().await? {
            let response: StartResponse = serde_json::from_slice(&payload)
                .map_err(|_| protocol_error("invalid command event"))?;
            if let Some(event) = response.event {
                if let Some(data) = event.data {
                    for (value, destination, writer) in [
                        (data.stdout, Destination::Stdout, &mut stdout),
                        (data.stderr, Destination::Stderr, &mut stderr),
                    ] {
                        if let Some(value) = value {
                            let bytes = base64::engine::general_purpose::STANDARD
                                .decode(value)
                                .map_err(|_| protocol_error("invalid command output encoding"))?;
                            if let Some(budget) = &budget {
                                budget.reserve(bytes.len() as u64)?;
                            }
                            captured = captured.saturating_add(bytes.len() as u64);
                            check_size(
                                captured,
                                limits.max_capture_bytes,
                                "e2b",
                                "captured output bytes",
                            )?;
                            stream::write_parent(&bytes, destination, stream_to_parent).await?;
                            stream::write_pipe(writer, &bytes).await?;
                        }
                    }
                }
                if let Some(end) = event.end {
                    if exit_code.is_some() {
                        return Err(protocol_error("duplicate command completion"));
                    }
                    let status = match (end.exit_status(), end.diagnostic()) {
                        (Ok(0) | Err(_), Some(_)) => {
                            return Err(protocol_error(
                                "command failed before reporting a usable exit code",
                            ))
                        }
                        (status, _) => status?,
                    };
                    if let Some(error) = end.diagnostic() {
                        if let Some(budget) = &budget {
                            budget.reserve((error.len() as u64).saturating_add(1))?;
                        }
                        captured = captured
                            .saturating_add(error.len() as u64)
                            .saturating_add(1);
                        check_size(
                            captured,
                            limits.max_capture_bytes,
                            "e2b",
                            "captured output bytes",
                        )?;
                        stream::write_parent(
                            error.as_bytes(),
                            Destination::Stderr,
                            stream_to_parent,
                        )
                        .await?;
                        stream::write_pipe(&mut stderr, error.as_bytes()).await?;
                        stderr.write_all(b"\n").await.map_err(io_error)?;
                    }
                    exit_code = Some(status);
                }
            }
        }
        let exit_code =
            exit_code.ok_or_else(|| protocol_error("command stream ended without an exit code"))?;
        stream::shutdown_pipe(&mut stdout).await?;
        stream::shutdown_pipe(&mut stderr).await?;
        Ok(exit_code)
    };
    let (exit_code, stdout, stderr) = tokio::try_join!(
        decode,
        async { cache.put_stream(stdout_reader).await.map_err(Error::from) },
        async { cache.put_stream(stderr_reader).await.map_err(Error::from) },
    )?;
    Ok(ActionResult {
        exit_code,
        stdout: Some(stdout),
        stderr: Some(stderr),
        outputs: BTreeMap::new(),
    })
}

#[cfg(test)]
mod tests;
