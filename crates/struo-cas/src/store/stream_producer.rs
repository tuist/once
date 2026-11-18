use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::{output, temporary, STREAM_CHUNK};
use crate::{Cas, Digest, Error, Result};

struct Cancellation(Arc<AtomicBool>);
impl Drop for Cancellation {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

struct Sink {
    output: output::Writer,
    cancelled: Arc<AtomicBool>,
    pipelined: bool,
}

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        check(&self.cancelled)?;
        let bytes = if self.pipelined {
            &bytes[..bytes.len().min(super::hasher::PIPELINE_CHUNK)]
        } else {
            bytes
        };
        self.output.push(bytes)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        check(&self.cancelled)
    }
}

fn check(cancelled: &AtomicBool) -> io::Result<()> {
    if cancelled.load(Ordering::Acquire) {
        Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "blob producer cancelled",
        ))
    } else {
        Ok(())
    }
}

pub(in crate::store) async fn produce<W, G>(
    cas: &Cas,
    guard: G,
    produce: W,
    pipelined: bool,
) -> Result<Digest>
where
    W: FnOnce(&mut dyn Write) -> io::Result<()> + Send + 'static,
    G: Send + 'static,
{
    let scratch = cas.scratch_dir();
    crate::filesystem::ensure_dir(&scratch).await?;
    let (raw, raw_path) = temporary(&scratch, "stream-")?.into_parts();
    let (encoded, encoded_path) = temporary(&scratch, "zstd-")?.into_parts();
    let raw_name = raw_path.to_path_buf();
    let encoded_name = encoded_path.to_path_buf();
    let owned = cas.clone();
    let cancellation = Cancellation(Arc::new(AtomicBool::new(false)));
    let cancelled = Arc::clone(&cancellation.0);
    let worker = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let mut sink = Sink {
            output: output::Writer::new(raw, encoded, None)?,
            cancelled,
            pipelined,
        };
        if pipelined {
            sink.output.pipeline_hash()?;
        }
        {
            let mut writer = io::BufWriter::with_capacity(STREAM_CHUNK, &mut sink);
            produce(&mut writer)?;
            writer.flush()?;
        }
        check(&sink.cancelled)?;
        let (compressed, digest, file) = sink.output.prepare(None)?;
        let destination = owned.blob_path(&digest);
        if !destination.try_exists().unwrap_or(false) {
            file.sync_all()?;
            drop(file);
            check(&sink.cancelled)?;
            let parent = destination.parent().expect("blob shard has a parent");
            std::fs::create_dir_all(parent)?;
            std::fs::rename(
                if compressed { &encoded_name } else { &raw_name },
                &destination,
            )?;
            sync_directory(parent)?;
        }
        Ok::<_, io::Error>(digest)
    });
    let result = worker
        .await
        .map_err(|source| Error::Io {
            path: scratch.clone(),
            source: io::Error::other(source.to_string()),
        })?
        .map_err(|source| Error::Io {
            path: scratch,
            source,
        });
    drop((raw_path, encoded_path));
    result
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}
#[cfg(not(unix))]
fn sync_directory(_: &Path) -> io::Result<()> {
    Ok(())
}
