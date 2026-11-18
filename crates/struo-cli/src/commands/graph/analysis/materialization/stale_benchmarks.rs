use super::*;
use struo_cas::{Cas, Digest};
use struo_core::{Action, CopyPathMode, ResourceLimits, WorkspacePath};

#[tokio::test]
#[ignore = "manual cached-output performance measurement"]
async fn benchmark_stale_modern_file_materialization() {
    measure_file(true).await;
}

#[tokio::test]
#[ignore = "manual cached-output performance measurement"]
async fn benchmark_matching_modern_file_validation() {
    measure_file(false).await;
}

async fn measure_file(stale_output: bool) {
    let cache_root = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(Cas::open(cache_root.path()));
    let source = tempfile::tempdir().unwrap();
    let bytes = super::benchmarks::random_bytes(32 * 1024 * 1024);
    std::fs::write(source.path().join("source.bin"), &bytes).unwrap();
    let action = Action::CopyPath {
        sources: vec![WorkspacePath::try_from("source.bin").unwrap()],
        destination: WorkspacePath::try_from("output.bin").unwrap(),
        mode: CopyPathMode::File,
        input_digest: None,
    };
    let result = super::benchmarks::capture(&cache, source.path(), &action).await;
    let expected = Digest::of_bytes(&bytes);
    let mut stale = bytes;
    if stale_output {
        stale[0] ^= 0xff;
    }
    let resources = Arc::new(ResourcePool::new(ResourceLimits::new(
        4,
        1024 * 1024 * 1024,
    )));
    let mut samples = Vec::new();
    for _ in 0..5 {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("output.bin"), &stale).unwrap();
        let modified = std::fs::metadata(workspace.path().join("output.bin"))
            .unwrap()
            .modified()
            .unwrap();
        let started = std::time::Instant::now();
        restore(
            std::slice::from_ref(&result),
            workspace.path(),
            &cache,
            None,
            &resources,
        )
        .await
        .unwrap();
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        if !stale_output {
            assert_eq!(
                std::fs::metadata(workspace.path().join("output.bin"))
                    .unwrap()
                    .modified()
                    .unwrap(),
                modified
            );
            assert!(!workspace.path().join(".struo/tmp").exists());
        }
        assert_eq!(
            Digest::of_bytes(&std::fs::read(workspace.path().join("output.bin")).unwrap()),
            expected
        );
    }
    samples.sort_by(f64::total_cmp);
    if stale_output {
        println!(
            "METRIC stale_file_materialization_ms={}",
            samples[samples.len() / 2]
        );
        println!("METRIC stale_file_materialized_mib=32");
    } else {
        println!(
            "METRIC matching_file_validation_ms={}",
            samples[samples.len() / 2]
        );
    }
}
