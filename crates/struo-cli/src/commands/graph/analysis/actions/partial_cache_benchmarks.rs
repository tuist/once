use super::*;
use struo_cas::Cas;
use struo_core::ResourceLimits;

fn random_bytes(size: usize) -> Vec<u8> {
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

async fn capture(
    cache: &CacheProvider,
    source: &Path,
    action: &Action,
    resources: &Arc<ResourcePool>,
) -> ActionResult {
    let _permit = resources.acquire(action.resource_request().clone()).await;
    struo_core::run_uncached(action, source, cache, false)
        .await
        .unwrap()
}

fn extend_available(available: &mut BTreeMap<String, AvailableInput>, result: &ActionResult) {
    available.extend(result.outputs.iter().map(|(path, digest)| {
        (
            path.clone(),
            AvailableInput {
                blob_digest: *digest,
                same_target: true,
                materialized: false,
            },
        )
    }));
}

async fn measure(ordered: bool) -> f64 {
    let root = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(Cas::open(root.path()));
    let source = tempfile::tempdir().unwrap();
    let resources = Arc::new(ResourcePool::new(ResourceLimits::new(
        4,
        1024 * 1024 * 1024,
    )));
    let mut bytes = random_bytes(32 * 1024 * 1024);
    std::fs::create_dir(source.path().join("source")).unwrap();
    let mut prior = Vec::new();
    let mut expected = Vec::new();
    let mut available = BTreeMap::new();
    for index in 0..8_u8 {
        bytes[0] = index;
        std::fs::write(source.path().join("source/payload.bin"), &bytes).unwrap();
        let path = format!("snapshot-{index}");
        let action = Action::CopyPath {
            sources: vec![WorkspacePath::try_from("source").unwrap()],
            destination: WorkspacePath::try_from(path.clone()).unwrap(),
            mode: CopyPathMode::Tree,
            input_digest: None,
        };
        let result = capture(&cache, source.path(), &action, &resources).await;
        extend_available(&mut available, &result);
        prior.push(result);
        expected.push((path, Digest::of_bytes(&bytes)));
    }
    let needed = random_bytes(4096);
    std::fs::write(source.path().join("needed-source.bin"), &needed).unwrap();
    let action = Action::CopyPath {
        sources: vec![WorkspacePath::try_from("needed-source.bin").unwrap()],
        destination: WorkspacePath::try_from("needed.bin").unwrap(),
        mode: CopyPathMode::File,
        input_digest: None,
    };
    let needed_result = capture(&cache, source.path(), &action, &resources).await;
    extend_available(&mut available, &needed_result);
    prior.push(needed_result);
    let declared: DeclaredAction = serde_json::from_value(serde_json::json!({
        "inputs": ["needed.bin"], "outputs": ["result.bin"], "depends_on_prior_actions": ordered,
        "operation": { "kind": "copy_path", "sources": ["needed.bin"], "destination": "result.bin", "mode": "file" }
    })).unwrap();
    let mut samples = Vec::new();
    for _ in 0..5 {
        let workspace = tempfile::tempdir().unwrap();
        let prepared = prepare_declared_action_with_inputs(
            workspace.path(),
            &declared,
            Digest::of_bytes(b"partial-cache"),
            &[],
            &available,
            None,
            SandboxMode::Inputs,
        )
        .unwrap();
        let context = DeclaredActionContext {
            workspace: workspace.path(),
            cache: &cache,
            source_digest_cache: None,
            available_inputs: &available,
            prior_cached_results: PendingResults {
                restored_at_end: &prior,
                intermediate: &[],
            },
            target_id: "partial-cache",
            capability: "build",
            index: 9,
            identifier: "copy",
            argv: &[],
            arg_files: &[],
            input_fingerprint: prepared.input_fingerprint,
            record_success_evidence: false,
            resources: &resources,
            output_observer: None,
        };
        let started = std::time::Instant::now();
        let (outcome, _) = execute_declared_cache_miss(&context, &prepared.action, &declared)
            .await
            .unwrap();
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(outcome.result.exit_code, 0);
        assert_eq!(
            std::fs::read(workspace.path().join("result.bin")).unwrap(),
            needed
        );
        if !ordered {
            materialize_prior_cached_results(workspace.path(), &cache, &prior, None)
                .await
                .unwrap();
        }
        for (path, digest) in &expected {
            let file = workspace.path().join(path).join("payload.bin");
            assert_eq!(Digest::of_bytes(&std::fs::read(file).unwrap()), *digest);
        }
    }
    samples.sort_by(f64::total_cmp);
    samples[2]
}

#[tokio::test]
#[ignore = "manual partial-cache miss restoration measurement"]
async fn benchmark_partial_cache_independent_miss() {
    println!("METRIC partial_cache_miss_ms={}", measure(false).await);
}

#[tokio::test]
#[ignore = "manual ordered partial-cache miss restoration measurement"]
async fn benchmark_partial_cache_ordered_miss() {
    println!("METRIC partial_cache_ordered_ms={}", measure(true).await);
}
