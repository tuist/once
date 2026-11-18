use super::*;

#[tokio::test]
#[ignore = "manual transfer performance measurement"]
async fn benchmark_shared_blob_downloads() {
    let mut state = 0x1234_5678_u64;
    let bytes: Vec<u8> = (0..32 * 1024 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect();
    let digest = Digest::of_bytes(&bytes);
    let mut samples = Vec::new();
    let mut downloaded_mib = 0;
    for _ in 0..7 {
        let temp = tempfile::TempDir::new().unwrap();
        let chunks = bytes
            .chunks(256 * 1024)
            .map(|chunk| {
                Ok(bytestream::ReadResponse {
                    data: chunk.to_vec(),
                })
            })
            .collect();
        let (cache, server, reads) =
            cache_for_failing_stream(&temp, digest, chunks, Vec::new(), false).await;
        cache
            .known_remote_blobs
            .lock()
            .await
            .insert(digest, sha256_digest(&bytes).unwrap());
        let recorder = Arc::new(crate::transfer::Recorder::default());
        let started = Instant::now();
        crate::transfer::observe(recorder.clone(), async {
            futures::future::try_join_all((0..4).map(|_| cache.ensure_blob_local(&digest)))
                .await
                .unwrap();
        })
        .await;
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        let requests = reads.load(std::sync::atomic::Ordering::SeqCst);
        assert!((1..=4).contains(&requests));
        downloaded_mib = requests * 32;
        assert_eq!(recorder.transfers().len(), requests);
        for transfer in recorder.transfers() {
            assert_eq!(transfer.size_bytes, 32 * 1024 * 1024);
            assert_eq!(transfer.digest, digest);
            assert_eq!(transfer.direction, TransferDirection::Download);
        }
        assert_eq!(cache.local.get_blob(&digest).await.unwrap(), bytes);
        server.abort();
    }
    samples.sort_by(f64::total_cmp);
    println!("METRIC shared_download_ms={}", samples[3]);
    println!("METRIC physical_download_mib={downloaded_mib}");
}
