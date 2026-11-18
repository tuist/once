use super::*;
use struo_cas::Cas;
use struo_core::{ResourceLimits, RunOpts};

async fn measure(mixed: bool) -> (f64, f64) {
    let workspace = tempfile::tempdir().unwrap();
    let mut state = 0x1234_5678_u64;
    let mut bytes: Vec<u8> = (0..32 * 1024 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect();
    if mixed {
        bytes[16 * 1024 * 1024..].fill(0);
    }
    let expected = Digest::of_bytes(&bytes);
    std::fs::create_dir(workspace.path().join("seed")).unwrap();
    std::fs::write(workspace.path().join("seed/payload.bin"), &bytes).unwrap();
    let resources = Arc::new(ResourcePool::new(ResourceLimits::new(
        4,
        1024 * 1024 * 1024,
    )));
    let action = Action::CopyPath {
        sources: vec![WorkspacePath::try_from("seed").unwrap()],
        destination: WorkspacePath::try_from("snapshot").unwrap(),
        mode: CopyPathMode::Tree,
        input_digest: None,
    };
    let mut samples = Vec::new();
    let mut stored = Vec::new();
    for _ in 0..5 {
        let cache_root = tempfile::tempdir().unwrap();
        let cas = Cas::open(cache_root.path());
        let cache = CacheProvider::Local(cas.clone());
        let started = std::time::Instant::now();
        let outcome = {
            let permit = resources.acquire(action.resource_request().clone()).await;
            struo_core::with_resource_permit(
                permit,
                Box::pin(struo_core::run_with_cache(
                    &action,
                    workspace.path(),
                    &cache,
                    RunOpts::default(),
                )),
            )
            .await
            .unwrap()
        };
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(outcome.cache, struo_core::CacheState::Miss);
        assert_eq!(outcome.result.exit_code, 0);
        assert!(cas
            .get_action_result(&action.digest())
            .await
            .unwrap()
            .is_some());
        let inventory = cas.stats().await.unwrap();
        assert_eq!(inventory.blob_count, 1);
        stored.push(f64::from(u32::try_from(inventory.blob_bytes).unwrap()) / (1024.0 * 1024.0));
        assert_eq!(
            Digest::of_bytes(
                &std::fs::read(workspace.path().join("snapshot/payload.bin")).unwrap()
            ),
            expected
        );
        std::fs::remove_dir_all(workspace.path().join("snapshot")).unwrap();
        let restored = tempfile::tempdir().unwrap();
        let _verification = resources.acquire(ResourceRequest::default()).await;
        struo_core::materialize_outputs(&outcome.result, restored.path(), &cache)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(restored.path().join("snapshot/payload.bin")).unwrap(),
            bytes
        );
    }
    samples.sort_by(f64::total_cmp);
    stored.sort_by(f64::total_cmp);
    (samples[2], stored[2])
}

#[tokio::test]
#[ignore = "manual fresh modern snapshot capture measurement"]
async fn benchmark_fresh_snapshot_capture() {
    let (ms, mib) = measure(false).await;
    println!("METRIC snapshot_capture_ms={ms}");
    println!("METRIC snapshot_capture_stored_mib={mib}");
}

#[tokio::test]
#[ignore = "manual late-compressibility snapshot capture monitor"]
async fn benchmark_mixed_snapshot_capture() {
    let (ms, mib) = measure(true).await;
    println!("METRIC mixed_snapshot_capture_ms={ms}");
    println!("METRIC mixed_snapshot_capture_stored_mib={mib}");
}
