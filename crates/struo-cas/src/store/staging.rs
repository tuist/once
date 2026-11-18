use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt};

use super::Cas;
use crate::filesystem::{ensure_dir, rename_into_place};
use crate::{Digest, Error, Result};

pub(crate) struct PendingBlob {
    tmp: tempfile::TempPath,
    digest: Digest,
    len: u64,
    _owner: Arc<dyn Send + Sync>,
}

impl std::fmt::Debug for PendingBlob {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingBlob")
            .field("digest", &self.digest)
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

impl PendingBlob {
    #[cfg(test)]
    pub(crate) fn digest(&self) -> Digest {
        self.digest
    }
    pub(crate) fn len(&self) -> u64 {
        self.len
    }
}

impl Cas {
    #[cfg(test)]
    pub(crate) async fn stage_stream<R: AsyncRead + Unpin>(
        &self,
        reader: R,
        declared: u64,
    ) -> Result<PendingBlob> {
        self.stage_stream_guarded(reader, declared, ()).await
    }

    pub(crate) async fn stage_stream_guarded<R: AsyncRead + Unpin, G: Send + 'static>(
        &self,
        reader: R,
        declared: u64,
        guard: G,
    ) -> Result<PendingBlob> {
        let scratch = self.scratch_dir();
        ensure_dir(&scratch).await?;
        let owner = Arc::new(Mutex::new(guard));
        let (tmp, digest) = super::stream::ingest_remote(
            &scratch,
            reader.take(declared.saturating_add(1)),
            declared,
            false,
            Arc::clone(&owner),
        )
        .await
        .map_err(|error| match error {
            Error::Io { path, source }
                if matches!(
                    source.kind(),
                    std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof
                ) =>
            {
                Error::corrupt_blob(path, Digest::of_bytes(&[]), &source.to_string())
            }
            error => error,
        })?;
        Ok(PendingBlob {
            tmp,
            digest,
            len: declared,
            _owner: owner,
        })
    }

    pub(crate) async fn commit_staged(&self, staged: PendingBlob) -> Result<Digest> {
        let final_path = self.blob_path(&staged.digest);
        if !tokio::fs::try_exists(&final_path).await.unwrap_or(false) {
            rename_into_place(&staged.tmp, &final_path).await?;
        }
        Ok(staged.digest)
    }
}
