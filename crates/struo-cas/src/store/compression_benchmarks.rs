use super::*;

#[tokio::test]
#[ignore = "manual native structured compression tradeoff measurement"]
async fn benchmark_native_structured_compression() {
    let mut bytes = Vec::new();
    let mut state = 0x1234_5678_u64;
    let mut index = 0_u32;
    while bytes.len() < 4 * 1024 * 1024 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let row = format!("{{\"id\":{},\"path\":\"workspace/data/{}/item-{}.txt\",\"status\":\"ready\",\"value\":{},\"metadata\":{{\"group\":{},\"parent\":{}}}}}\n",
            state % 65536, index % 512, index % 65536, state % 1_000_000, index % 8, index / 64);
        bytes.extend_from_slice(row.as_bytes());
        index += 1;
    }
    bytes.truncate(4 * 1024 * 1024);
    let expected = Digest::of_bytes(&bytes);
    let level3 = zstd::stream::encode_all(bytes.as_slice(), 3).unwrap();
    let reference = u32::try_from(level3.len() + blob::ZSTD_BLOB_HEADER_LEN).unwrap();
    let mut samples = Vec::new();
    let mut sizes = Vec::new();
    for _ in 0..5 {
        let root = tempfile::tempdir().unwrap();
        let cas = Cas::open(root.path());
        let started = std::time::Instant::now();
        let digest = cas.put_stream(bytes.as_slice()).await.unwrap();
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(digest, expected);
        assert_eq!(cas.get_blob(&digest).await.unwrap(), bytes);
        sizes
            .push(u32::try_from(std::fs::metadata(cas.blob_path(&digest)).unwrap().len()).unwrap());
    }
    samples.sort_by(f64::total_cmp);
    sizes.sort_unstable();
    println!("METRIC native_structured_capture_ms={}", samples[2]);
    println!(
        "METRIC native_structured_stored_mib={}",
        f64::from(sizes[2]) / (1024.0 * 1024.0)
    );
    println!(
        "METRIC native_structured_level3_mib={}",
        f64::from(reference) / (1024.0 * 1024.0)
    );
}
