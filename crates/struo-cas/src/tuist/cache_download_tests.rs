use super::*;

async fn fixture_bytes(
    temp: &tempfile::TempDir,
    bytes: &[u8],
    failures: Vec<Status>,
    stalls: bool,
) -> (
    TuistCache,
    Digest,
    tokio::task::JoinHandle<()>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    let digest = Digest::of_bytes(bytes);
    let chunks = bytes
        .chunks(32 * 1024)
        .map(|chunk| {
            Ok(bytestream::ReadResponse {
                data: chunk.to_vec(),
            })
        })
        .collect();
    let (cache, server, reads) =
        cache_for_failing_stream(temp, digest, chunks, failures, stalls).await;
    cache
        .known_remote_blobs
        .lock()
        .await
        .insert(digest, sha256_digest(bytes).unwrap());
    (cache, digest, server, reads)
}

#[tokio::test]
async fn file_and_memory_readers_share_one_verified_download() {
    let temp = tempfile::TempDir::new().unwrap();
    let bytes = vec![7; 3 * 1024 * 1024];
    let (cache, digest, server, reads) = fixture_bytes(&temp, &bytes, Vec::new(), false).await;
    let recorder = Arc::new(crate::transfer::Recorder::default());
    let ((), memory) = Box::pin(crate::transfer::observe(recorder.clone(), async {
        tokio::try_join!(cache.ensure_blob_local(&digest), cache.get_blob(&digest)).unwrap()
    }))
    .await;
    assert_eq!(memory, bytes);
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(recorder.transfers().len(), 1);
    assert_eq!(recorder.transfers()[0].size_bytes, 3 * 1024 * 1024);
    server.abort();
}

#[tokio::test]
async fn transient_failure_is_retried_by_one_reader_while_the_other_waits() {
    let temp = tempfile::TempDir::new().unwrap();
    let bytes = vec![7; 3 * 1024 * 1024];
    let (cache, digest, server, reads) = fixture_bytes(
        &temp,
        &bytes,
        vec![Status::unavailable("interrupted")],
        false,
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
async fn a_failed_read_releases_waiters_without_memoizing_success() {
    let temp = tempfile::TempDir::new().unwrap();
    let bytes = vec![7; 3 * 1024 * 1024];
    let (cache, digest, server, reads) = fixture_bytes(
        &temp,
        &bytes,
        vec![Status::permission_denied("first read denied")],
        false,
    )
    .await;
    let (first, second) = tokio::join!(
        cache.ensure_blob_local(&digest),
        cache.ensure_blob_local(&digest)
    );
    assert_ne!(first.is_err(), second.is_err());
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(cache.local.get_blob(&digest).await.unwrap(), bytes);
    server.abort();
}

#[tokio::test]
async fn cancelling_a_stalled_reader_does_not_hold_the_next_reader() {
    let temp = tempfile::TempDir::new().unwrap();
    let bytes = vec![7; 100_000];
    let (cache, digest, server, reads) = fixture_bytes(&temp, &bytes, Vec::new(), true).await;
    let producer = cache.clone();
    let first = tokio::spawn(async move { producer.ensure_blob_local(&digest).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while reads.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let producer = cache.clone();
    let second = tokio::spawn(async move { producer.ensure_blob_local(&digest).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while reads.load(std::sync::atomic::Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    second.abort();
    assert!(second.await.unwrap_err().is_cancelled());
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(!cache.local.has_blob(&digest).await.unwrap());
    if cache.local.scratch_dir().exists() {
        assert_eq!(
            std::fs::read_dir(cache.local.scratch_dir())
                .unwrap()
                .count(),
            0
        );
    }
    server.abort();
}

#[tokio::test]
async fn corrupt_remote_mapping_never_satisfies_a_waiting_reader() {
    let temp = tempfile::TempDir::new().unwrap();
    let wrong = vec![8; 3 * 1024 * 1024];
    let (cache, _, server, reads) = fixture_bytes(&temp, &wrong, Vec::new(), false).await;
    let expected = Digest::of_bytes(&vec![7; 3 * 1024 * 1024]);
    cache
        .known_remote_blobs
        .lock()
        .await
        .insert(expected, sha256_digest(&wrong).unwrap());
    let (file, memory) = Box::pin(async {
        tokio::join!(
            cache.ensure_blob_local(&expected),
            cache.get_blob(&expected)
        )
    })
    .await;
    assert!(file.is_err());
    assert!(memory.is_err());
    assert!(!cache.local.has_blob(&expected).await.unwrap());
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 2);
    server.abort();
}

#[tokio::test]
async fn unrelated_download_and_publication_leases_do_not_block_a_download() {
    let temp = tempfile::TempDir::new().unwrap();
    let bytes = vec![7; 3 * 1024 * 1024];
    let (cache, digest, server, reads) = fixture_bytes(&temp, &bytes, Vec::new(), false).await;
    let _other = cache
        .download_flights
        .acquire(Digest::of_bytes(b"other blob"))
        .await;
    let _publication = cache.publication_flights.acquire(digest).await;
    tokio::time::timeout(Duration::from_secs(2), cache.ensure_blob_local(&digest))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cache.local.get_blob(&digest).await.unwrap(), bytes);
    assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
    server.abort();
}
