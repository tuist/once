use super::*;
use std::collections::BTreeMap;
use struo_cas::{Cas, Digest};
use struo_core::{Action, CopyPathMode, ResourceLimits, ResourceRequest, WorkspacePath};

pub(super) fn random_bytes(size: usize) -> Vec<u8> {
    let mut state = 0x1234_5678_u64;
    (0..size)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect()
}

fn resources() -> Arc<ResourcePool> {
    Arc::new(ResourcePool::new(ResourceLimits::new(
        4,
        1024 * 1024 * 1024,
    )))
}

pub(super) async fn capture(cache: &CacheProvider, source: &Path, action: &Action) -> ActionResult {
    let resources = resources();
    let _permit = resources.acquire(action.resource_request().clone()).await;
    struo_core::run_uncached(action, source, cache, false)
        .await
        .unwrap()
}

async fn measure(
    results: &[ActionResult],
    cache: &CacheProvider,
    files: &BTreeMap<String, Digest>,
    contended: bool,
) -> f64 {
    let resources = resources();
    let _held = if contended {
        Some(resources.acquire(ResourceRequest::default()).await)
    } else {
        None
    };
    let mut samples = Vec::new();
    for _ in 0..5 {
        let workspace = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        restore(results, workspace.path(), cache, None, &resources)
            .await
            .unwrap();
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        for (path, expected) in files {
            let restored = std::fs::read(workspace.path().join(path)).unwrap();
            assert_eq!(Digest::of_bytes(&restored), *expected);
        }
    }
    samples.sort_by(f64::total_cmp);
    samples[2]
}

async fn directory_materialization(contended: bool) -> f64 {
    let cache_root = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(Cas::open(cache_root.path()));
    let mut bytes = random_bytes(32 * 1024 * 1024);
    let source = tempfile::tempdir().unwrap();
    std::fs::create_dir(source.path().join("source")).unwrap();
    let mut results = Vec::new();
    let mut expected = BTreeMap::new();
    for index in 0..8_u8 {
        bytes[0] = index;
        std::fs::write(source.path().join("source/payload.bin"), &bytes).unwrap();
        let path = format!("output-{index}");
        expected.insert(format!("{path}/payload.bin"), Digest::of_bytes(&bytes));
        let action = Action::CopyPath {
            sources: vec![WorkspacePath::try_from("source").unwrap()],
            destination: WorkspacePath::try_from(path).unwrap(),
            mode: CopyPathMode::Tree,
            input_digest: None,
        };
        results.push(capture(&cache, source.path(), &action).await);
    }
    measure(&results, &cache, &expected, contended).await
}

#[tokio::test]
#[ignore = "manual cached-output performance measurement"]
async fn benchmark_cached_directory_materialization() {
    println!(
        "METRIC materialization_ms={}",
        directory_materialization(false).await
    );
    println!("METRIC materialized_mib=256");
}

#[tokio::test]
#[ignore = "manual cached-output performance measurement"]
async fn benchmark_contended_directory_materialization() {
    println!(
        "METRIC contended_materialization_ms={}",
        directory_materialization(true).await
    );
}

#[tokio::test]
#[ignore = "manual cached-output performance measurement"]
async fn benchmark_heterogeneous_directory_materialization() {
    let cache_root = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(Cas::open(cache_root.path()));
    let source = tempfile::tempdir().unwrap();
    std::fs::create_dir(source.path().join("source")).unwrap();
    let mut results = Vec::new();
    let mut expected = BTreeMap::new();
    for index in 0..8_u8 {
        let size = if index == 0 { 128 } else { 4 } * 1024 * 1024;
        let mut bytes = random_bytes(size);
        bytes[0] = index;
        std::fs::write(source.path().join("source/payload.bin"), &bytes).unwrap();
        let path = format!("output-{index}");
        expected.insert(format!("{path}/payload.bin"), Digest::of_bytes(&bytes));
        let action = Action::CopyPath {
            sources: vec![WorkspacePath::try_from("source").unwrap()],
            destination: WorkspacePath::try_from(path).unwrap(),
            mode: CopyPathMode::Tree,
            input_digest: None,
        };
        results.push(capture(&cache, source.path(), &action).await);
    }
    println!(
        "METRIC heterogeneous_materialization_ms={}",
        measure(&results, &cache, &expected, true).await
    );
    println!("METRIC heterogeneous_materialized_mib=156");
}

#[tokio::test]
#[ignore = "manual cached-output performance measurement"]
async fn benchmark_cached_small_directory_materialization() {
    let cache_root = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(Cas::open(cache_root.path()));
    let source = tempfile::tempdir().unwrap();
    std::fs::create_dir(source.path().join("source")).unwrap();
    let mut bytes = random_bytes(4096);
    let mut expected = BTreeMap::new();
    for index in 0..1024_u16 {
        bytes[..2].copy_from_slice(&index.to_le_bytes());
        let path = format!("piece-{index}.bin");
        std::fs::write(source.path().join("source").join(&path), &bytes).unwrap();
        expected.insert(format!("snapshot/{path}"), Digest::of_bytes(&bytes));
    }
    let action = Action::CopyPath {
        sources: vec![WorkspacePath::try_from("source").unwrap()],
        destination: WorkspacePath::try_from("snapshot").unwrap(),
        mode: CopyPathMode::Tree,
        input_digest: None,
    };
    let result = capture(&cache, source.path(), &action).await;
    println!(
        "METRIC small_directory_ms={}",
        measure(&[result], &cache, &expected, false).await
    );
    println!("METRIC small_directory_files=1024");
}

#[tokio::test]
#[ignore = "manual cached-output performance measurement"]
async fn benchmark_cached_modern_file_materialization() {
    let cache_root = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(Cas::open(cache_root.path()));
    let source = tempfile::tempdir().unwrap();
    let bytes = random_bytes(32 * 1024 * 1024);
    std::fs::write(source.path().join("source.bin"), &bytes).unwrap();
    let action = Action::CopyPath {
        sources: vec![WorkspacePath::try_from("source.bin").unwrap()],
        destination: WorkspacePath::try_from("output.bin").unwrap(),
        mode: CopyPathMode::File,
        input_digest: None,
    };
    let result = capture(&cache, source.path(), &action).await;
    let expected = BTreeMap::from([("output.bin".into(), Digest::of_bytes(&bytes))]);
    println!(
        "METRIC modern_file_ms={}",
        measure(&[result], &cache, &expected, false).await
    );
}
