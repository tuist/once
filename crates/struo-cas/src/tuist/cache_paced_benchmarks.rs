use super::*;

fn corpus() -> Vec<u8> {
    let mut state = 0x1234_5678_u64;
    (0..32 * 1024 * 1024)
        .map(|index| {
            if index % (64 * 1024) < 32 * 1024 {
                0
            } else {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state.to_le_bytes()[0]
            }
        })
        .collect()
}

fn chunks(bytes: &[u8]) -> Vec<std::result::Result<bytestream::ReadResponse, Status>> {
    bytes
        .chunks(256 * 1024)
        .map(|chunk| {
            Ok(bytestream::ReadResponse {
                data: chunk.to_vec(),
            })
        })
        .collect()
}

async fn measure(delay: Duration, bytes: Vec<u8>) -> (f64, f64) {
    let compressed = zstd::stream::encode_all(bytes.as_slice(), 0).unwrap();
    let digest = Digest::of_bytes(&bytes);
    let mut samples = Vec::new();
    let mut physical = Vec::new();
    for _ in 0..7 {
        let temp = tempfile::TempDir::new().unwrap();
        let (cache, server, reads, wire_bytes) = cache_for_configured_stream(
            &temp,
            digest,
            chunks(&bytes),
            StreamOptions {
                compressed: Some(chunks(&compressed)),
                chunk_delay: delay,
                ..StreamOptions::default()
            },
        )
        .await;
        cache
            .known_remote_blobs
            .lock()
            .await
            .insert(digest, sha256_digest(&bytes).unwrap());
        let recorder = Arc::new(crate::transfer::Recorder::default());
        let started = Instant::now();
        crate::transfer::observe(recorder.clone(), cache.ensure_blob_local(&digest))
            .await
            .unwrap();
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        let transferred = wire_bytes.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            transferred == u64::try_from(bytes.len()).unwrap()
                || transferred == u64::try_from(compressed.len()).unwrap()
        );
        assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
        let reports = recorder.transfers();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].digest, digest);
        assert_eq!(
            reports[0].direction,
            crate::transfer::TransferDirection::Download
        );
        assert_eq!(reports[0].size_bytes, u64::try_from(bytes.len()).unwrap());
        physical.push(transferred);
        server.abort();
        assert_eq!(cache.local.get_blob(&digest).await.unwrap(), bytes);
    }
    samples.sort_by(f64::total_cmp);
    physical.sort_unstable();
    (
        samples[3],
        f64::from(u32::try_from(physical[3]).unwrap()) / (1024.0 * 1024.0),
    )
}

#[tokio::test]
#[ignore = "manual paced transfer performance measurement"]
async fn benchmark_paced_stream_downloads() {
    let (elapsed, physical) = measure(Duration::from_millis(4), corpus()).await;
    println!("METRIC paced_download_ms={elapsed}");
    println!("METRIC paced_physical_download_mib={physical}");
}

#[tokio::test]
#[ignore = "manual unpaced compressed transfer performance measurement"]
async fn benchmark_unpaced_compressed_downloads() {
    let (elapsed, physical) = measure(Duration::ZERO, corpus()).await;
    println!("METRIC compressed_download_ms={elapsed}");
    println!("METRIC compressed_physical_download_mib={physical}");
}

#[tokio::test]
#[ignore = "manual incompressible compressed transfer performance measurement"]
async fn benchmark_compressed_incompressible_downloads() {
    let mut state = 0x1234_5678_u64;
    let bytes = (0..32 * 1024 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect();
    let (elapsed, physical) = measure(Duration::ZERO, bytes).await;
    println!("METRIC compressed_incompressible_download_ms={elapsed}");
    println!("METRIC compressed_incompressible_physical_mib={physical}");
}
