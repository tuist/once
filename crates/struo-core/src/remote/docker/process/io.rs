use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::io_error;
use crate::executor::staging::CapturePermit;
use crate::stream::{self, Destination};
use crate::Result;

pub(super) async fn bounded_bytes(reader: impl AsyncRead + Unpin, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(io_error)?;
    crate::remote::http::check_size(
        bytes.len() as u64,
        limit,
        "docker",
        "control response bytes",
    )?;
    Ok(bytes)
}

pub(super) async fn pump(
    mut reader: impl AsyncRead + Unpin,
    writer: &mut (impl AsyncWrite + Unpin),
    limit: u64,
    capture: Option<(&AtomicU64, Option<&CapturePermit>)>,
    parent: Option<(Destination, bool)>,
) -> Result<()> {
    let mut buffer = vec![0_u8; 32 * 1024];
    let mut count = 0_u64;
    loop {
        let len = reader.read(&mut buffer).await.map_err(io_error)?;
        if len == 0 {
            break;
        }
        count = match capture {
            Some((total, budget)) => {
                if let Some(budget) = budget {
                    budget.reserve(len as u64)?;
                }
                total
                    .fetch_add(len as u64, Ordering::Relaxed)
                    .saturating_add(len as u64)
            }
            None => count.saturating_add(len as u64),
        };
        crate::remote::http::check_size(count, limit, "docker", "streamed bytes")?;
        if let Some((destination, enabled)) = parent {
            stream::write_parent(&buffer[..len], destination, enabled).await?;
        }
        writer.write_all(&buffer[..len]).await.map_err(io_error)?;
    }
    writer.shutdown().await.map_err(io_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    #[tokio::test]
    async fn controls_stop_after_the_first_byte_beyond_the_bound() {
        assert_eq!(bounded_bytes(&b"four"[..], 4).await.unwrap(), b"four");
        let mut reader = &b"sixsix"[..];
        assert!(matches!(
            bounded_bytes(&mut reader, 4).await,
            Err(Error::ExecutorResourceLimit { .. })
        ));
        assert_eq!(reader, b"x");
    }

    #[tokio::test]
    async fn stdout_and_stderr_share_one_capture_limit() {
        let total = AtomicU64::new(0);
        let mut writer = tokio::io::sink();
        pump(&b"four"[..], &mut writer, 6, Some((&total, None)), None)
            .await
            .unwrap();
        assert!(matches!(
            pump(&b"four"[..], &mut writer, 6, Some((&total, None)), None).await,
            Err(Error::ExecutorResourceLimit { .. })
        ));
    }

    #[tokio::test]
    async fn oversized_archives_are_not_written_to_the_sink() {
        let mut sink = Vec::new();
        assert!(pump(&b"oversized"[..], &mut sink, 4, None, None)
            .await
            .is_err());
        assert!(sink.is_empty());
    }
}
