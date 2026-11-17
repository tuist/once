use futures::TryStreamExt as _;

use super::*;

impl TuistCache {
    pub(in crate::tuist::cache) async fn mirror_reapi_blobs(
        &self,
        digests: &[reapi::Digest],
        operation: &'static str,
    ) -> Result<Vec<Digest>> {
        let limit = self.batch_download_limit().await.unwrap_or_default();
        let unique = digests
            .iter()
            .map(|digest| (digest_key(digest), digest.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut groups = Vec::new();
        let mut batch = Vec::new();
        let mut bytes = 0_u64;
        for digest in unique.into_values() {
            let size = u64::try_from(digest.size_bytes).map_err(|_| {
                status::remote(operation, ErrorKind::Protocol, "negative blob size")
            })?;
            if size == 0 || size > limit {
                groups.push((vec![digest], size));
                continue;
            }
            if batch.len() == MAX_PENDING || bytes + size > limit {
                groups.push((std::mem::take(&mut batch), bytes));
                bytes = 0;
            }
            bytes += size;
            batch.push(digest);
        }
        if !batch.is_empty() {
            groups.push((batch, bytes));
        }
        let mirrored = stream::iter(groups)
            .map(|(batch, bytes)| self.mirror_reapi_batch(batch, bytes, limit, operation))
            .buffer_unordered(MAX_PARALLEL_TRANSFERS)
            .try_fold(HashMap::new(), |mut mirrored, batch| async move {
                mirrored.extend(batch);
                Ok(mirrored)
            })
            .await?;
        Ok(digests
            .iter()
            .map(|digest| mirrored[&digest_key(digest)])
            .collect())
    }

    async fn mirror_reapi_batch(
        &self,
        digests: Vec<reapi::Digest>,
        bytes: u64,
        limit: u64,
        operation: &'static str,
    ) -> Result<Vec<(String, Digest)>> {
        if digests.len() == 1 {
            let digest = &digests[0];
            if bytes > limit {
                let started = Instant::now();
                let staged =
                    retry::grpc(operation, || self.stage_remote_blob(digest, operation)).await?;
                let local = self.local.commit_staged(staged).await?;
                self.known_remote_blobs
                    .lock()
                    .await
                    .insert(local, digest.clone());
                transfer::record(TransferDirection::Download, local, bytes, started.elapsed());
                return Ok(vec![(digest_key(digest), local)]);
            }
            return Ok(vec![(
                digest_key(digest),
                self.mirror_reapi_blob(digest, operation).await?,
            )]);
        }
        retry::grpc(operation, || async {
            let started = Instant::now();
            let Some(batch) = self.read_download_batch(digests.clone(), bytes).await? else {
                let mut mirrored = Vec::new();
                for digest in &digests {
                    let staged =
                        retry::grpc(operation, || self.stage_remote_blob(digest, operation))
                            .await?;
                    let local = self.local.commit_staged(staged).await?;
                    self.known_remote_blobs
                        .lock()
                        .await
                        .insert(local, digest.clone());
                    transfer::record(
                        TransferDirection::Download,
                        local,
                        digest.size_bytes as u64,
                        started.elapsed(),
                    );
                    mirrored.push((digest_key(digest), local));
                }
                return Ok(mirrored);
            };
            let BatchResponse {
                blobs,
                memory: _memory,
                staging: _staging,
                permit: _permit,
            } = batch;
            let mut mirrored = Vec::with_capacity(digests.len());
            for (digest, blob) in digests.iter().zip(blobs) {
                let body =
                    download::blob_read_outcome(blob, digest, operation)?.ok_or_else(|| {
                        status::remote(
                            operation,
                            ErrorKind::NotFound,
                            "missing execution output blob",
                        )
                    })?;
                let local = Digest::of_bytes(&body);
                self.local.mirror_blob(&local, &body).await?;
                self.known_remote_blobs
                    .lock()
                    .await
                    .insert(local, digest.clone());
                transfer::record(
                    TransferDirection::Download,
                    local,
                    body.len() as u64,
                    started.elapsed(),
                );
                mirrored.push((digest_key(digest), local));
            }
            Ok(mirrored)
        })
        .await
    }
}
