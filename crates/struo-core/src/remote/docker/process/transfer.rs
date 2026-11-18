use std::path::Path;

use futures::TryStreamExt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::io::{ReaderStream, StreamReader};

use super::{bounded_bytes, error, io_error, pump, Client};
use crate::remote::http::Progress;
use crate::{Error, Result};

#[cfg(test)]
mod tests;

impl Client {
    pub async fn transfer(
        &self,
        args: &[String],
        input: Option<&Path>,
        output: Option<&Path>,
    ) -> Result<()> {
        let mut child = self.spawn(args, input.is_some())?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let progress = Progress::new();
        let work = async {
            let ((), (), _, status) = tokio::try_join!(
                async {
                    if let Some(path) = input {
                        let file = tokio::fs::File::open(path).await.map_err(io_error)?;
                        upload(file, stdin.expect("piped stdin"), &progress).await?;
                    }
                    Ok(())
                },
                async {
                    if let Some(path) = output {
                        let mut file = tokio::fs::File::create(path).await.map_err(io_error)?;
                        let stream = progress.track(ReaderStream::new(stdout));
                        let reader = StreamReader::new(Box::pin(stream));
                        pump(reader, &mut file, self.limits.max_archive_bytes, None, None).await?;
                        file.flush().await.map_err(io_error)?;
                    } else {
                        bounded_bytes(stdout, self.limits.control_bytes()).await?;
                    }
                    Ok(())
                },
                bounded_bytes(stderr, self.limits.control_bytes()),
                async { child.wait().await.map_err(io_error) },
            )?;
            if status.success() {
                Ok(())
            } else {
                Err(error(format!(
                    "Docker archive transfer failed with {status}"
                )))
            }
        };
        let watched = async {
            if input.is_some() {
                progress.watch_upload("docker", self.limits, work).await
            } else {
                progress
                    .watch("docker", "download", self.limits.stall_timeout, work)
                    .await
            }
        };
        tokio::time::timeout(self.limits.artifact_download_timeout, watched)
            .await
            .map_err(|_| Error::Timeout(self.limits.artifact_download_timeout))?
    }
}

async fn upload(
    reader: impl AsyncRead + Unpin,
    writer: impl AsyncWrite + Unpin,
    progress: &Progress,
) -> Result<()> {
    let state = (reader, writer, vec![0_u8; 32 * 1024], 0, 0);
    let stream = futures::stream::try_unfold(
        state,
        |(mut reader, mut writer, mut buffer, mut start, mut end)| async move {
            if start == end {
                end = reader.read(&mut buffer).await?;
                start = 0;
                if end == 0 {
                    writer.shutdown().await?;
                    return Ok::<_, std::io::Error>(None);
                }
            }
            let written = writer.write(&buffer[start..end]).await?;
            if written == 0 {
                return Err(std::io::ErrorKind::WriteZero.into());
            }
            start += written;
            Ok(Some(((), (reader, writer, buffer, start, end))))
        },
    );
    let mut stream = Box::pin(progress.track(stream));
    while stream.try_next().await.map_err(io_error)?.is_some() {}
    Ok(())
}
