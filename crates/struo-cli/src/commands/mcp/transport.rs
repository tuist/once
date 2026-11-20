//! Newline-delimited JSON-RPC transport over stdio.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use struo_core::ResourceLimits;

use super::{DispatchOutcome, Server};

/// Largest single newline-delimited JSON-RPC request the server will
/// buffer. The transport is one request per line, so without a cap a
/// client (or a prompt-injected agent) could send one unterminated
/// multi-gigabyte line and OOM the process before it is ever parsed.
/// Tool calls are orders of magnitude smaller than this.
pub(super) const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;

pub(super) fn serve_blocking(
    workspace: PathBuf,
    allow_run: bool,
    resource_limits: ResourceLimits,
) -> Result<()> {
    let stdin = io::stdin();
    let mut handle = stdin.lock();
    let mut stdout = io::stdout().lock();
    let server = Server::with_resource_limits(workspace, allow_run, resource_limits);

    let mut buf: Vec<u8> = Vec::new();
    loop {
        match read_capped_line(&mut handle, &mut buf, MAX_REQUEST_BYTES)
            .context("reading MCP request")?
        {
            CappedLine::Eof => break,
            CappedLine::TooLong => {
                // Refuse the oversize request instead of buffering it, and
                // keep serving: the rest of that line was drained.
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": {
                        "code": -32600,
                        "message": "request exceeds maximum size",
                    },
                });
                write_message(&mut stdout, &response)?;
                continue;
            }
            CappedLine::Line => {}
        }
        let Ok(line) = std::str::from_utf8(&buf) else {
            continue;
        };
        if line.trim().is_empty() {
            continue;
        }
        match server.dispatch_line(line) {
            DispatchOutcome::Reply(response) => write_message(&mut stdout, &response)?,
            DispatchOutcome::Notification => {
                // Notifications get no reply per JSON-RPC 2.0.
            }
        }
    }
    Ok(())
}

/// Write one response as a JSON line. It is encoded first so it reaches
/// stdout, which flushes every kilobyte, in one write.
fn write_message(out: &mut impl Write, message: &impl serde::Serialize) -> Result<()> {
    let mut line = serde_json::to_vec(message).context("encoding MCP response")?;
    line.push(b'\n');
    out.write_all(&line)?;
    out.flush()?;
    Ok(())
}

/// Result of reading one newline-delimited record under a byte cap.
pub(super) enum CappedLine {
    /// A complete line (newline stripped) is in the buffer.
    Line,
    /// The line exceeded the cap; its bytes were drained and dropped, so
    /// the next read resumes at the following record.
    TooLong,
    /// End of input.
    Eof,
}

/// Read one `\n`-delimited record into `buf` (without the newline),
/// buffering at most `max` bytes. A longer line is drained to its
/// terminator and reported as [`CappedLine::TooLong`] without ever
/// holding more than `max` bytes in memory.
pub(super) fn read_capped_line<R: BufRead>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    max: usize,
) -> io::Result<CappedLine> {
    buf.clear();
    let mut over = false;
    loop {
        let available = match reader.fill_buf() {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            return Ok(if over {
                CappedLine::TooLong
            } else if buf.is_empty() {
                CappedLine::Eof
            } else {
                CappedLine::Line
            });
        }
        if let Some(pos) = available.iter().position(|&b| b == b'\n') {
            if !over && buf.len() + pos <= max {
                buf.extend_from_slice(&available[..pos]);
            } else {
                over = true;
            }
            reader.consume(pos + 1);
            return Ok(if over {
                CappedLine::TooLong
            } else {
                CappedLine::Line
            });
        }
        if !over && buf.len() + available.len() <= max {
            buf.extend_from_slice(available);
        } else if !over {
            // Would exceed the cap: stop accumulating and drop what we
            // have so memory stays bounded while we drain to the newline.
            over = true;
            buf.clear();
        }
        let consumed = available.len();
        reader.consume(consumed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capped_reader_reads_normal_lines() {
        let data = b"first\nsecond\n";
        let mut reader = std::io::BufReader::new(&data[..]);
        let mut buf = Vec::new();
        assert!(matches!(
            read_capped_line(&mut reader, &mut buf, 1024).unwrap(),
            CappedLine::Line
        ));
        assert_eq!(buf, b"first");
        assert!(matches!(
            read_capped_line(&mut reader, &mut buf, 1024).unwrap(),
            CappedLine::Line
        ));
        assert_eq!(buf, b"second");
        assert!(matches!(
            read_capped_line(&mut reader, &mut buf, 1024).unwrap(),
            CappedLine::Eof
        ));
    }

    #[test]
    fn capped_reader_rejects_oversize_line_without_buffering_it() {
        // An 8 KiB line under a 16-byte cap: reported TooLong, and the
        // buffer never holds more than the cap.
        let mut data = vec![b'x'; 8192];
        data.push(b'\n');
        data.extend_from_slice(b"next\n");
        let mut reader = std::io::BufReader::with_capacity(64, &data[..]);
        let mut buf = Vec::new();
        assert!(matches!(
            read_capped_line(&mut reader, &mut buf, 16).unwrap(),
            CappedLine::TooLong
        ));
        assert!(buf.len() <= 16, "buffer must stay within the cap");
        // The reader resumes cleanly at the following record.
        assert!(matches!(
            read_capped_line(&mut reader, &mut buf, 16).unwrap(),
            CappedLine::Line
        ));
        assert_eq!(buf, b"next");
    }

    #[test]
    fn capped_reader_accepts_a_line_exactly_at_the_cap() {
        let data = b"1234567890\n";
        let mut reader = std::io::BufReader::new(&data[..]);
        let mut buf = Vec::new();
        assert!(matches!(
            read_capped_line(&mut reader, &mut buf, 10).unwrap(),
            CappedLine::Line
        ));
        assert_eq!(buf, b"1234567890");
    }
}
