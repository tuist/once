use super::*;

pub(super) struct BatchResponse {
    pub(super) blobs: Vec<reapi::batch_read_blobs_response::Response>,
    pub(super) memory: crate::budget::Reservation,
    pub(super) staging: crate::budget::Reservation,
    pub(super) permit: OwnedSemaphorePermit,
}

impl TuistCache {
    pub(super) async fn read_download_batch(
        &self,
        digests: Vec<reapi::Digest>,
        bytes: u64,
    ) -> Result<Option<BatchResponse>> {
        let admitted = Instant::now();
        let largest = digests
            .iter()
            .map(|digest| u64::try_from(digest.size_bytes).unwrap_or_default())
            .max()
            .unwrap_or_default();
        // Publication writes one temporary CAS file at a time, not every blob at once.
        let staging = self
            .budget
            .reserve_staging(stream_limits::encoded_bytes(largest))
            .await?;
        let response_limit = download::batch_response_limit(bytes);
        let memory = self
            .budget
            .reserve_memory(
                response_limit as u64 + 2 * bytes + stream_limits::CODEC_MEMORY + FRAMING_BYTES,
            )
            .await?;
        let permit = Arc::clone(&self.transfer_limit)
            .acquire_owned()
            .await
            .expect("transfer semaphore remains open");
        let capabilities = self.remote_capabilities().await;
        let channel = self.grpc_channel().await?;
        let mut acceptable_compressors = vec![reapi::compressor::Value::Identity as i32];
        if capabilities.zstd_batches {
            acceptable_compressors.push(reapi::compressor::Value::Zstd as i32);
        }
        let request = reapi::BatchReadBlobsRequest {
            instance_name: self.instance_name(),
            digests: digests.clone(),
            acceptable_compressors,
            digest_function: SHA256_DIGEST_FUNCTION,
        };
        let rpc_started = Instant::now();
        let admission_seconds = admitted.elapsed().as_secs_f64();
        let response = retry::grpc("get blobs", || {
            let channel = channel.clone();
            let request = request.clone();
            async move {
                match batch_rpc::read(
                    channel,
                    self.authorized_grpc_request(request, "get blobs").await?,
                    response_limit,
                    self.stream_stall_timeout,
                )
                .await
                {
                    Ok(response) => Ok(Some(response.into_inner())),
                    Err(status) if status.code() == Code::Unimplemented => Ok(None),
                    Err(status) if status.code() == Code::NotFound => Err(super::status::remote(
                        "get blobs",
                        ErrorKind::Protocol,
                        "batch RPC returned NOT_FOUND without per-blob statuses",
                    )
                    .into()),
                    Err(status) => Err(status.into()),
                }
            }
        })
        .await?;
        let Some(response) = response else {
            self.batch_downloads
                .unsupported
                .store(true, Ordering::Relaxed);
            return Ok(None);
        };
        let blobs = ordered_responses(response, &digests)?;
        tracing::debug!(
            blobs = blobs.len(),
            decoded_bytes = bytes,
            payload_bytes = blobs.iter().map(|blob| blob.data.len()).sum::<usize>(),
            admission_seconds,
            rpc_seconds = rpc_started.elapsed().as_secs_f64(),
            "fetched remote blob batch"
        );
        Ok(Some(BatchResponse {
            blobs,
            memory,
            staging,
            permit,
        }))
    }
}

fn ordered_responses(
    response: reapi::BatchReadBlobsResponse,
    digests: &[reapi::Digest],
) -> Result<Vec<reapi::batch_read_blobs_response::Response>> {
    let count = response.responses.len();
    let mut indexed = response
        .responses
        .into_iter()
        .map(|blob| {
            let digest = blob.digest.as_ref().ok_or_else(|| {
                status::remote("get blobs", ErrorKind::Protocol, "missing response digest")
            })?;
            Ok(((digest.hash.clone(), digest.size_bytes), blob))
        })
        .collect::<Result<HashMap<_, _>>>()?;
    if count != digests.len() || indexed.len() != count {
        return Err(status::remote(
            "get blobs",
            ErrorKind::Protocol,
            "missing or duplicate batch response",
        ));
    }
    digests
        .iter()
        .map(|digest| {
            indexed
                .remove(&(digest.hash.clone(), digest.size_bytes))
                .ok_or_else(|| {
                    status::remote(
                        "get blobs",
                        ErrorKind::Protocol,
                        "unexpected batch response digest",
                    )
                })
        })
        .collect()
}
