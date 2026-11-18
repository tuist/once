use tokio::io::AsyncRead;

use super::frames::{protocol_error, FrameReader};
use super::{decode_event, ProcessResult};
use crate::{RemoteIoLimits, Result};

/// Collect a short process's output and exit status, such as a setup command.
///
/// Quiet periods between messages are allowed, so a setup step that prints
/// nothing for a long time still completes. Everything the stream delivers,
/// including framing and empty messages, counts against the control budget,
/// so the buffered output can never exceed it.
pub(in crate::remote::e2b) async fn collect_process<R: AsyncRead + Unpin>(
    reader: R,
    limits: RemoteIoLimits,
) -> Result<ProcessResult> {
    let mut frames =
        FrameReader::new(reader, limits).with_total(limits.control_bytes(), "setup response bytes");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_code = None;
    while let Some(payload) = frames.next().await? {
        decode_event(&payload, &mut stdout, &mut stderr, &mut exit_code)?;
    }
    let exit_code =
        exit_code.ok_or_else(|| protocol_error("command stream ended without an exit code"))?;
    Ok(ProcessResult {
        exit_code,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests;
