use super::*;

#[path = "cache_download_body.rs"]
mod body;

impl TuistCache {
    pub(super) async fn stream_remote_blob<G: Clone + Send + 'static>(
        &self,
        digest: &Digest,
        remote_digest: &reapi::Digest,
        guard: G,
    ) -> std::result::Result<(), retry::Failure> {
        let compressed = self.remote_capabilities().await.zstd_streams;
        match self
            .stream_remote_representation(digest, remote_digest, compressed, guard.clone())
            .await
        {
            Err(retry::Failure::Local(error))
                if compressed && Cas::remote_window_limit_exceeded(&error) =>
            {
                tracing::debug!(blob_digest = %digest, "retrying oversized compressed frame as identity");
                self.stream_remote_representation(digest, remote_digest, false, guard)
                    .await
            }
            result => result,
        }
    }

    async fn stream_remote_representation<G: Send + 'static>(
        &self,
        digest: &Digest,
        remote_digest: &reapi::Digest,
        compressed: bool,
        guard: G,
    ) -> std::result::Result<(), retry::Failure> {
        let expected = u64::try_from(remote_digest.size_bytes).map_err(|_| {
            status::remote(
                "get blob",
                ErrorKind::Protocol,
                "remote blob has invalid declared size",
            )
        })?;
        let message_limit = stream_limits::message_bytes(expected, self.budget.memory_bytes());
        let staging = self
            .budget
            .reserve_staging(stream_limits::staging_bytes(expected))
            .await?;
        let memory = self
            .budget
            .reserve_memory(stream_limits::memory_bytes(message_limit))
            .await?;
        let permit = Arc::clone(&self.transfer_limit)
            .acquire_owned()
            .await
            .expect("transfer semaphore remains open");
        let compressor = if compressed {
            reapi::compressor::Value::Zstd
        } else {
            reapi::compressor::Value::Identity
        };
        let channel = self.grpc_channel().await?;
        let mut client = ByteStreamClient::new(channel).max_decoding_message_size(
            usize::try_from(message_limit).unwrap_or(BYTE_STREAM_MESSAGE_LIMIT),
        );
        let request = bytestream::ReadRequest {
            resource_name: byte_stream_read_resource(
                &self.instance_name(),
                remote_digest,
                compressor as i32,
            ),
            read_offset: 0,
            read_limit: 0,
        };
        let response = match client
            .read(self.authorized_grpc_request(request, "get blob").await?)
            .await
        {
            Ok(response) => response,
            Err(status) if status.code() == Code::NotFound => {
                return Err(Error::BlobNotFound(*digest).into())
            }
            Err(status) => return Err(status.into()),
        };
        let mirrored = body::store(
            &self.local,
            response.into_inner(),
            self.stream_stall_timeout,
            expected,
            compressed,
            (*digest, remote_digest.hash.clone()),
            (permit, memory, staging, guard),
        )
        .await?;
        if mirrored != *digest {
            return Err(status::remote(
                "get blob",
                ErrorKind::Corrupt,
                format!("remote blob {digest} did not match requested digest"),
            )
            .into());
        }
        Ok(())
    }
}
