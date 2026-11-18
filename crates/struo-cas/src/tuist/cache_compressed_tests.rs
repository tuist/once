use super::*;

async fn fixture(
    temp: &tempfile::TempDir,
    bytes: &[u8],
    encoded: Vec<u8>,
    stalls: bool,
    failures: Vec<Status>,
) -> (
    TuistCache,
    Digest,
    tokio::task::JoinHandle<()>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    let digest = Digest::of_bytes(bytes);
    let chunks = |bytes: &[u8]| {
        bytes
            .chunks(1024)
            .map(|chunk| {
                Ok(bytestream::ReadResponse {
                    data: chunk.to_vec(),
                })
            })
            .collect()
    };
    let (cache, server, reads, _) = cache_for_configured_stream(
        temp,
        digest,
        chunks(bytes),
        StreamOptions {
            compressed: Some(chunks(&encoded)),
            stalls,
            failures,
            ..StreamOptions::default()
        },
    )
    .await;
    cache
        .known_remote_blobs
        .lock()
        .await
        .insert(digest, sha256_digest(bytes).unwrap());
    (cache, digest, server, reads)
}

#[tokio::test]
async fn compressed_and_concatenated_frames_restore_exact_bytes() {
    let bytes = vec![7; 512 * 1024];
    let middle = bytes.len() / 2;
    let mut concatenated = zstd::stream::encode_all(&bytes[..middle], 0).unwrap();
    concatenated.extend(zstd::stream::encode_all(&bytes[middle..], 0).unwrap());
    for encoded in [
        zstd::stream::encode_all(bytes.as_slice(), 0).unwrap(),
        concatenated,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let (cache, digest, server, reads) =
            fixture(&temp, &bytes, encoded, false, Vec::new()).await;
        cache.ensure_blob_local(&digest).await.unwrap();
        assert_eq!(cache.local.get_blob(&digest).await.unwrap(), bytes);
        assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
        server.abort();
    }
}

#[tokio::test]
async fn oversized_windows_fall_back_to_identity_with_one_logical_transfer() {
    use std::io::Write;
    let bytes = vec![7; 16 * 1024 * 1024];
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 0).unwrap();
    encoder.window_log(24).unwrap();
    encoder
        .set_pledged_src_size(Some(u64::try_from(bytes.len()).unwrap()))
        .unwrap();
    encoder.write_all(&bytes).unwrap();
    let body = encoder.finish().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (cache, digest, server, reads) = fixture(&temp, &bytes, body, false, Vec::new()).await;
    let recorder = Arc::new(crate::transfer::Recorder::default());
    crate::transfer::observe(recorder.clone(), cache.ensure_blob_local(&digest))
        .await
        .unwrap();
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(cache.local.get_blob(&digest).await.unwrap(), bytes);
    let reports = recorder.transfers();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].size_bytes, u64::try_from(bytes.len()).unwrap());
    assert_eq!(
        std::fs::read_dir(cache.local.scratch_dir())
            .unwrap()
            .count(),
        0
    );
    server.abort();
}

#[tokio::test]
async fn remote_encodings_preserve_raw_fallback_and_literal_magic() {
    let mut state = 0x1234_5678_u64;
    let random: Vec<u8> = (0..128 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect();
    for bytes in [random, crate::blob::ZSTD_BLOB_MAGIC.to_vec(), Vec::new()] {
        let temp = tempfile::tempdir().unwrap();
        let encoded = zstd::stream::encode_all(bytes.as_slice(), 0).unwrap();
        let (cache, digest, server, _) = fixture(&temp, &bytes, encoded, false, Vec::new()).await;
        cache.ensure_blob_local(&digest).await.unwrap();
        assert_eq!(cache.local.get_blob(&digest).await.unwrap(), bytes);
        server.abort();
    }
}

#[tokio::test]
async fn malformed_or_truncated_frames_never_publish_requested_content() {
    let bytes = vec![7; 512 * 1024];
    let encoded = zstd::stream::encode_all(bytes.as_slice(), 0).unwrap();
    let mut trailing = encoded.clone();
    trailing.extend_from_slice(b"not a frame");
    for invalid in [
        encoded[..encoded.len() - 1].to_vec(),
        trailing,
        b"not zstd".to_vec(),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let (cache, digest, server, _) = fixture(&temp, &bytes, invalid, false, Vec::new()).await;
        assert!(cache.ensure_blob_local(&digest).await.is_err());
        assert!(!cache.local.has_blob(&digest).await.unwrap());
        assert_eq!(
            std::fs::read_dir(cache.local.scratch_dir())
                .unwrap()
                .count(),
            0
        );
        server.abort();
    }
}

#[tokio::test]
async fn declared_size_mismatches_never_publish_requested_content() {
    let bytes = vec![7; 512 * 1024];
    for expected in [bytes.len() - 1, bytes.len() + 1] {
        let temp = tempfile::tempdir().unwrap();
        let (cache, digest, server, _) = fixture(
            &temp,
            &bytes,
            zstd::stream::encode_all(bytes.as_slice(), 0).unwrap(),
            false,
            Vec::new(),
        )
        .await;
        cache
            .known_remote_blobs
            .lock()
            .await
            .get_mut(&digest)
            .unwrap()
            .size_bytes = i64::try_from(expected).unwrap();
        assert!(cache.ensure_blob_local(&digest).await.is_err());
        assert!(!cache.local.has_blob(&digest).await.unwrap());
        assert_eq!(
            std::fs::read_dir(cache.local.scratch_dir())
                .unwrap()
                .count(),
            0
        );
        server.abort();
    }
}

#[tokio::test]
async fn compressed_stream_errors_retry_and_preserve_single_flight() {
    let bytes = vec![7; 512 * 1024];
    let temp = tempfile::tempdir().unwrap();
    let (cache, digest, server, reads) = fixture(
        &temp,
        &bytes,
        zstd::stream::encode_all(bytes.as_slice(), 0).unwrap(),
        false,
        vec![Status::unavailable("interrupted")],
    )
    .await;
    let (first, second) = tokio::join!(
        cache.ensure_blob_local(&digest),
        cache.ensure_blob_local(&digest)
    );
    first.unwrap();
    second.unwrap();
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(cache.local.get_blob(&digest).await.unwrap(), bytes);
    server.abort();
}

#[tokio::test]
async fn a_complete_frame_without_transport_eof_does_not_publish() {
    let bytes = vec![7; 512 * 1024];
    let temp = tempfile::tempdir().unwrap();
    let (mut cache, digest, server, _) = fixture(
        &temp,
        &bytes,
        zstd::stream::encode_all(bytes.as_slice(), 0).unwrap(),
        true,
        Vec::new(),
    )
    .await;
    cache.stream_stall_timeout = Duration::from_millis(20);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), cache.ensure_blob_local(&digest))
            .await
            .unwrap()
            .is_err()
    );
    assert!(!cache.local.has_blob(&digest).await.unwrap());
    assert_eq!(
        std::fs::read_dir(cache.local.scratch_dir())
            .unwrap()
            .count(),
        0
    );
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guarded_staging_retains_remote_decoder_resources_after_cancellation() {
    struct Guard {
        entered: Option<tokio::sync::oneshot::Sender<()>>,
        release: std::sync::mpsc::Receiver<()>,
        _permit: tokio::sync::OwnedSemaphorePermit,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            self.entered.take().unwrap().send(()).unwrap();
            self.release.recv_timeout(Duration::from_secs(5)).unwrap();
        }
    }
    let bytes = vec![7; 512 * 1024];
    let temp = tempfile::tempdir().unwrap();
    let (cache, digest, server, _) = fixture(
        &temp,
        &bytes,
        zstd::stream::encode_all(bytes.as_slice(), 0).unwrap(),
        true,
        Vec::new(),
    )
    .await;
    let resources = Arc::new(tokio::sync::Semaphore::new(1));
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let guard = Guard {
        entered: Some(entered),
        release: wait,
        _permit: Arc::clone(&resources).acquire_owned().await.unwrap(),
    };
    let provider = crate::CacheProvider::Tuist(cache.clone());
    let parent = temp.path().to_path_buf();
    let task =
        tokio::spawn(async move { provider.stage_blob_guarded(&digest, &parent, guard).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let written = std::fs::read_dir(cache.local.scratch_dir()).is_ok_and(|entries| {
                entries.filter_map(std::result::Result::ok).any(|entry| {
                    std::fs::File::open(entry.path())
                        .and_then(|file| file.metadata())
                        .is_ok_and(|metadata| metadata.len() >= 128 * 1024)
                })
            });
            if written {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    tokio::time::timeout(Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    assert!(resources.try_acquire().is_err());
    assert_eq!(
        std::fs::read_dir(cache.local.scratch_dir())
            .unwrap()
            .count(),
        0
    );
    release.send(()).unwrap();
    assert!(task.await.unwrap_err().is_cancelled());
    let permit = tokio::time::timeout(Duration::from_secs(5), resources.acquire())
        .await
        .unwrap()
        .unwrap();
    drop(permit);
    assert!(!cache.local.has_blob(&digest).await.unwrap());
    assert_eq!(
        std::fs::read_dir(cache.local.scratch_dir())
            .unwrap()
            .count(),
        0
    );
    server.abort();
}

#[tokio::test]
async fn cancelling_compressed_streams_cleans_scratch_and_releases_waiters() {
    let bytes = vec![7; 512 * 1024];
    let temp = tempfile::tempdir().unwrap();
    let (cache, digest, server, reads) = fixture(
        &temp,
        &bytes,
        zstd::stream::encode_all(bytes.as_slice(), 0).unwrap(),
        true,
        Vec::new(),
    )
    .await;
    for target_reads in [1, 2] {
        let producer = cache.clone();
        let task = tokio::spawn(async move { producer.ensure_blob_local(&digest).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let started = reads.load(std::sync::atomic::Ordering::SeqCst) >= target_reads;
                let written = std::fs::read_dir(cache.local.scratch_dir()).is_ok_and(|entries| {
                    entries.filter_map(std::result::Result::ok).any(|entry| {
                        std::fs::File::open(entry.path())
                            .and_then(|file| file.metadata())
                            .is_ok_and(|metadata| metadata.len() >= 128 * 1024)
                    })
                });
                if started && written {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(!cache.local.has_blob(&digest).await.unwrap());
        if cache.local.scratch_dir().exists() {
            assert_eq!(
                std::fs::read_dir(cache.local.scratch_dir())
                    .unwrap()
                    .count(),
                0
            );
        }
    }
    server.abort();
}
