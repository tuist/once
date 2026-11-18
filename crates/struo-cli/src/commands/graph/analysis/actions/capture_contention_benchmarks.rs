use super::*;
use struo_cas::Cas;
use struo_core::{ResourceLimits, RunOpts};

#[tokio::test]
#[ignore = "manual fully admitted fresh capture contention monitor"]
async fn benchmark_admitted_snapshot_capture() {
    let root = tempfile::tempdir().unwrap();
    let limits = ResourceLimits::new(4, 1024 * 1024 * 1024);
    let width = limits.max_parallel_actions();
    let resources = Arc::new(ResourcePool::new(limits));
    let mut state = 0x1234_5678_u64;
    let mut bytes: Vec<u8> = (0..32 * 1024 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect();
    let mut workspaces = Vec::new();
    let mut expected = Vec::new();
    for index in 0..width {
        bytes[0] = u8::try_from(index).unwrap();
        expected.push(Digest::of_bytes(&bytes));
        let workspace = root.path().join(index.to_string());
        std::fs::create_dir_all(workspace.join("seed")).unwrap();
        std::fs::write(workspace.join("seed/payload.bin"), &bytes).unwrap();
        workspaces.push(workspace);
    }
    let action = Action::CopyPath {
        sources: vec![WorkspacePath::try_from("seed").unwrap()],
        destination: WorkspacePath::try_from("snapshot").unwrap(),
        mode: CopyPathMode::Tree,
        input_digest: None,
    };
    let mut samples = Vec::new();
    let mut stored = Vec::new();
    for _ in 0..5 {
        let caches: Vec<_> = (0..width).map(|_| tempfile::tempdir().unwrap()).collect();
        let providers: Vec<_> = caches
            .iter()
            .map(|root| CacheProvider::open_local(root.path()))
            .collect();
        let barrier = tokio::sync::Barrier::new(width);
        let started = std::time::Instant::now();
        let outcomes = futures::future::join_all(workspaces.iter().zip(&providers).map(
            |(workspace, cache)| {
                let resources = &resources;
                let barrier = &barrier;
                let action = &action;
                async move {
                    let permit = resources.acquire(action.resource_request().clone()).await;
                    barrier.wait().await;
                    struo_core::with_resource_permit(
                        permit,
                        Box::pin(struo_core::run_with_cache(
                            action,
                            workspace,
                            cache,
                            RunOpts::default(),
                        )),
                    )
                    .await
                    .unwrap()
                }
            },
        ))
        .await;
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        let mut total = 0_u64;
        for (index, ((outcome, workspace), cache)) in
            outcomes.iter().zip(&workspaces).zip(&providers).enumerate()
        {
            assert_eq!(outcome.cache, struo_core::CacheState::Miss);
            assert_eq!(outcome.result.exit_code, 0);
            let inventory = Cas::open(cache.root()).stats().await.unwrap();
            assert_eq!(inventory.blob_count, 1);
            total += inventory.blob_bytes;
            assert_eq!(
                Digest::of_bytes(&std::fs::read(workspace.join("snapshot/payload.bin")).unwrap()),
                expected[index]
            );
            let restored = tempfile::tempdir().unwrap();
            let _verification = resources.acquire(ResourceRequest::default()).await;
            struo_core::materialize_outputs(&outcome.result, restored.path(), cache)
                .await
                .unwrap();
            assert_eq!(
                Digest::of_bytes(
                    &std::fs::read(restored.path().join("snapshot/payload.bin")).unwrap()
                ),
                expected[index]
            );
            assert!(cache
                .get_action_result(&action.digest())
                .await
                .unwrap()
                .is_some());
            std::fs::remove_dir_all(workspace.join("snapshot")).unwrap();
        }
        stored.push(f64::from(u32::try_from(total).unwrap()) / (1024.0 * 1024.0));
    }
    samples.sort_by(f64::total_cmp);
    stored.sort_by(f64::total_cmp);
    println!("METRIC admitted_capture_ms={}", samples[2]);
    println!("METRIC admitted_capture_stored_mib={}", stored[2]);
}
