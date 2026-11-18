mod encode;
pub(in crate::remote) use encode::encode;
mod progress;
pub(in crate::remote) use progress::Progress;

use std::path::Path;

use futures::StreamExt;
use serde::de::DeserializeOwned;
use tokio::io::AsyncWriteExt;

use crate::remote::{phase_permit, PhasePermit};
use crate::{Error, ExecutionPermit, RemoteIoLimits, Result};

pub(super) fn checked(
    response: reqwest::Response,
    provider: &'static str,
) -> Result<reqwest::Response> {
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(Error::RemoteProviderStatus {
            provider: provider.to_string(),
            status: response.status().as_u16(),
        })
    }
}

pub(super) fn http_error(provider: &'static str, source: reqwest::Error) -> Error {
    Error::RemoteProviderHttp {
        provider: provider.to_string(),
        source: source.without_url(),
    }
}

pub(super) async fn body(
    response: reqwest::Response,
    provider: &'static str,
    limit: u64,
    limits: RemoteIoLimits,
) -> Result<Vec<u8>> {
    check_size(
        response.content_length().unwrap_or(0),
        limit,
        provider,
        "response bytes",
    )?;
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = tokio::time::timeout(limits.stall_timeout, stream.next())
        .await
        .map_err(|_| stalled(provider, limits))?
    {
        let chunk = chunk.map_err(|source| http_error(provider, source))?;
        let size = (bytes.len() as u64).saturating_add(chunk.len() as u64);
        check_size(size, limit, provider, "response bytes")?;
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub(super) async fn decode<T: DeserializeOwned>(
    response: reqwest::Response,
    provider: &'static str,
    limit: u64,
    limits: RemoteIoLimits,
) -> Result<T> {
    let bytes = body(checked(response, provider)?, provider, limit, limits).await?;
    serde_json::from_slice(&bytes).map_err(|source| Error::RemoteProviderApi {
        provider: provider.to_string(),
        message: format!(
            "invalid protocol response at line {} column {}",
            source.line(),
            source.column()
        ),
    })
}

pub(super) async fn download(
    response: reqwest::Response,
    destination: &Path,
    provider: &'static str,
    limits: RemoteIoLimits,
    lease: Option<&ExecutionPermit>,
    copies: u64,
) -> Result<PhasePermit> {
    let response = checked(response, provider)?;
    check_size(
        response.content_length().unwrap_or(0),
        limits.max_archive_bytes,
        provider,
        "archive bytes",
    )?;
    let reservation = response
        .content_length()
        .unwrap_or(limits.max_archive_bytes)
        .saturating_mul(copies);
    let phase = phase_permit(lease, reservation).await?;
    let mut file = tokio::fs::File::create(destination)
        .await
        .map_err(file_error)?;
    let mut stream = response.bytes_stream();
    let mut size = 0_u64;
    while let Some(chunk) = tokio::time::timeout(limits.stall_timeout, stream.next())
        .await
        .map_err(|_| stalled(provider, limits))?
    {
        let chunk = chunk.map_err(|source| http_error(provider, source))?;
        size = size.saturating_add(chunk.len() as u64);
        check_size(size, limits.max_archive_bytes, provider, "archive bytes")?;
        file.write_all(&chunk).await.map_err(file_error)?;
    }
    file.flush().await.map_err(file_error)?;
    Ok(phase)
}

pub(super) fn check_size(
    size: u64,
    limit: u64,
    provider: &'static str,
    resource: &'static str,
) -> Result<()> {
    if size > limit {
        Err(Error::ExecutorResourceLimit {
            executor: provider.to_string(),
            resource,
            limit,
        })
    } else {
        Ok(())
    }
}

fn stalled(provider: &'static str, limits: RemoteIoLimits) -> Error {
    Error::ExecutorStalled {
        executor: provider.to_string(),
        phase: "response body",
        timeout: limits.stall_timeout,
    }
}

fn file_error(source: std::io::Error) -> Error {
    Error::FileAction {
        action: "write_remote_output_archive",
        path: ".struo/tmp".to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::AsyncReadExt;

    async fn response(headers: &str, payload: &[u8], hold: bool) -> reqwest::Response {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let headers = headers.to_string();
        let payload = payload.to_vec();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).await;
            let _ = stream.write_all(headers.as_bytes()).await;
            let _ = stream.write_all(&payload).await;
            if hold {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        });
        reqwest::get(format!("http://{address}")).await.unwrap()
    }

    #[tokio::test]
    async fn rejects_oversized_declared_and_chunked_bodies() {
        for (headers, payload) in [
            ("HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n", &b""[..]),
            (
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
                &b"5\r\nhello\r\n0\r\n\r\n"[..],
            ),
        ] {
            let response = response(headers, payload, false).await;
            let error = body(response, "test", 4, RemoteIoLimits::default())
                .await
                .unwrap_err();
            assert!(matches!(
                error,
                Error::ExecutorResourceLimit { limit: 4, .. }
            ));
        }
    }

    #[tokio::test]
    async fn stalled_body_has_a_distinct_failure() {
        let response = response("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n", b"", true).await;
        let limits = RemoteIoLimits {
            stall_timeout: Duration::from_millis(20),
            ..RemoteIoLimits::default()
        };
        assert!(matches!(
            body(response, "test", 5, limits).await,
            Err(Error::ExecutorStalled { .. })
        ));
    }

    #[tokio::test]
    async fn oversized_archive_never_replaces_a_destination() {
        let response = response(
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n",
            b"hello",
            false,
        )
        .await;
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("output");
        std::fs::write(&destination, b"original").unwrap();
        let limits = RemoteIoLimits {
            max_archive_bytes: 4,
            ..RemoteIoLimits::default()
        };
        assert!(matches!(
            download(response, &destination, "test", limits, None, 1).await,
            Err(Error::ExecutorResourceLimit { .. })
        ));
        assert_eq!(std::fs::read(destination).unwrap(), b"original");
    }

    #[tokio::test]
    async fn error_bodies_are_not_buffered_or_exposed() {
        let response = response(
            "HTTP/1.1 503 Unavailable\r\nContent-Length: 1000000000\r\n\r\n",
            b"secret-token",
            false,
        )
        .await;
        let error = checked(response, "test").unwrap_err();
        assert!(matches!(
            error,
            Error::RemoteProviderStatus { status: 503, .. }
        ));
        assert!(!error.to_string().contains("secret-token"));
    }
}
