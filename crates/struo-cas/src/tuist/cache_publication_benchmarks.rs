use super::*;

#[tokio::test]
#[ignore = "manual transfer performance measurement"]
async fn benchmark_shared_blob_publications() {
    let mut state = 0x1234_5678_u64;
    let bytes: Vec<u8> = (0..32 * 1024 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect();
    let mut samples = Vec::new();
    let mut uploaded_mib = 0.0;
    for _ in 0..7 {
        let temp = tempfile::TempDir::new().unwrap();
        let (cache, service, server) = fixture(&temp).await;
        let digest = cache.local.put_blob(&bytes).await.unwrap();
        let action = Digest::of_bytes(b"shared output action");
        let result = ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs: (0..4)
                .map(|index| (format!("output-{index}"), digest))
                .collect(),
        };
        let started = Instant::now();
        cache
            .put_action_result_remote(&action, &result)
            .await
            .unwrap();
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        let observed = service.0.lock().await;
        assert!(!observed.writes.is_empty());
        let mut total = 0_u32;
        for write in &observed.writes {
            let body: Vec<u8> = write
                .iter()
                .flat_map(|chunk| chunk.data.iter().copied())
                .collect();
            assert_eq!(body, bytes);
            total += u32::try_from(body.len()).unwrap();
        }
        let remote = observed
            .actions
            .last()
            .unwrap()
            .action_result
            .as_ref()
            .unwrap();
        assert_eq!(remote.output_files.len(), 4);
        for output in &remote.output_files {
            assert_eq!(
                output.digest.as_ref().unwrap(),
                &sha256_digest(&bytes).unwrap()
            );
        }
        uploaded_mib = f64::from(total) / (1024.0 * 1024.0);
        server.abort();
    }
    samples.sort_by(f64::total_cmp);
    println!("METRIC shared_publication_ms={}", samples[3]);
    println!("METRIC physical_upload_mib={uploaded_mib}");
}
