use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};

use super::decode_end_stream;
use crate::remote::http::check_size;
use crate::stream::PIPE_CAPACITY;
use crate::{Error, RemoteIoLimits, Result};

const ENVELOPE_HEADER_LEN: usize = 5;
const ENVELOPE_HEADER_BYTES: u64 = 5;

/// Reads E2B process messages from a Connect stream or its JSON-lines form.
///
/// Waiting for the next message is bounded only by `command_idle_timeout`, so
/// a command that is quiet for a long time is not mistaken for a stalled
/// connection. Once a message has started, every read must make progress
/// within `stall_timeout`. With a total budget, every byte read counts against
/// it, including headers, blank lines, and the end-of-stream message, so a
/// stream of empty frames cannot grow without bound either.
pub(super) struct FrameReader<R> {
    reader: BufReader<R>,
    limits: RemoteIoLimits,
    total: Option<Budget>,
    lines: Option<bool>,
    ended: bool,
}

struct Budget {
    limit: u64,
    used: u64,
    resource: &'static str,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    pub(super) fn new(reader: R, limits: RemoteIoLimits) -> Self {
        Self {
            reader: BufReader::with_capacity(PIPE_CAPACITY, reader),
            limits,
            total: None,
            lines: None,
            ended: false,
        }
    }

    /// Bound every byte the stream delivers, not only the decoded output.
    pub(super) fn with_total(mut self, limit: u64, resource: &'static str) -> Self {
        self.total = Some(Budget {
            limit,
            used: 0,
            resource,
        });
        self
    }

    /// The next process message payload, or `None` once the stream ends.
    pub(super) async fn next(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            let Some(first) = self.first_byte().await? else {
                return Ok(None);
            };
            if self.ended {
                return Err(protocol_error(
                    "data followed the end of the command stream",
                ));
            }
            // Envelope flag bytes are never whitespace, so a stream that starts
            // with `{` or a blank line can only be JSON lines.
            let lines = *self
                .lines
                .get_or_insert(first == b'{' || first.is_ascii_whitespace());
            let (flags, payload) = if lines {
                (0, self.line().await?)
            } else {
                self.envelope().await?
            };
            if flags & 1 != 0 {
                return Err(protocol_error("compressed Connect frames are unsupported"));
            }
            if flags & 2 != 0 {
                decode_end_stream(&payload)?;
                self.ended = true;
                continue;
            }
            if payload.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            return Ok(Some(payload));
        }
    }

    /// Peek the first byte of the next message. Only this wait may be quiet.
    async fn first_byte(&mut self) -> Result<Option<u8>> {
        let buffered = match self.limits.command_idle_timeout {
            Some(timeout) => tokio::time::timeout(timeout, self.reader.fill_buf())
                .await
                .map_err(|_| stalled(timeout))?
                .map_err(io_error)?,
            None => self.reader.fill_buf().await.map_err(io_error)?,
        };
        Ok(buffered.first().copied())
    }

    async fn envelope(&mut self) -> Result<(u8, Vec<u8>)> {
        let mut header = [0_u8; ENVELOPE_HEADER_LEN];
        self.read_exact(&mut header).await?;
        let len = u64::from(u32::from_be_bytes(
            header[1..].try_into().expect("four length bytes"),
        ));
        check_size(
            len,
            self.limits.max_response_bytes,
            "e2b",
            "command frame bytes",
        )?;
        self.charge(ENVELOPE_HEADER_BYTES.saturating_add(len))?;
        let len = usize::try_from(len)
            .map_err(|_| protocol_error("command frame does not fit in memory"))?;
        let mut payload = vec![0; len];
        self.read_exact(&mut payload).await?;
        Ok((header[0], payload))
    }

    async fn line(&mut self) -> Result<Vec<u8>> {
        let mut line = Vec::new();
        loop {
            let max_frame = self.limits.max_response_bytes;
            let (count, complete) = {
                let chunk = self.progressing(|reader| reader.fill_buf()).await?;
                if chunk.is_empty() {
                    return Ok(line);
                }
                let newline = chunk.iter().position(|byte| *byte == b'\n');
                let count = newline.map_or(chunk.len(), |index| index + 1);
                check_size(
                    (line.len() + count) as u64,
                    max_frame,
                    "e2b",
                    "command frame bytes",
                )?;
                line.extend_from_slice(&chunk[..count]);
                (count, newline.is_some())
            };
            self.reader.consume(count);
            self.charge(count as u64)?;
            if complete {
                return Ok(line);
            }
        }
    }

    /// Fill `bytes`, requiring each read to make progress within the stall
    /// timeout rather than the whole frame to arrive within it.
    async fn read_exact(&mut self, bytes: &mut [u8]) -> Result<()> {
        let mut filled = 0;
        while filled < bytes.len() {
            let read = self
                .progressing(|reader| reader.read(&mut bytes[filled..]))
                .await?;
            if read == 0 {
                return Err(protocol_error("command stream ended inside a message"));
            }
            filled += read;
        }
        Ok(())
    }

    async fn progressing<'a, T, F>(
        &'a mut self,
        read: impl FnOnce(&'a mut BufReader<R>) -> F,
    ) -> Result<T>
    where
        F: std::future::Future<Output = std::io::Result<T>> + 'a,
    {
        let timeout = self.limits.stall_timeout;
        tokio::time::timeout(timeout, read(&mut self.reader))
            .await
            .map_err(|_| stalled(timeout))?
            .map_err(io_error)
    }

    fn charge(&mut self, bytes: u64) -> Result<()> {
        if let Some(budget) = &mut self.total {
            budget.used = budget.used.saturating_add(bytes);
            check_size(budget.used, budget.limit, "e2b", budget.resource)?;
        }
        Ok(())
    }
}

fn stalled(timeout: std::time::Duration) -> Error {
    Error::ExecutorStalled {
        executor: "e2b".to_string(),
        phase: "command output",
        timeout,
    }
}

pub(super) fn protocol_error(message: &str) -> Error {
    super::super::client::api_error(message.to_string())
}

pub(super) fn io_error(source: std::io::Error) -> Error {
    Error::Wait {
        program: "remote command stream".to_string(),
        source,
    }
}

#[cfg(test)]
mod tests;
