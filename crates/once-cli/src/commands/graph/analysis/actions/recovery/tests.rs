use std::path::PathBuf;

use once_core::{EvidenceCacheState, SandboxMode};
use once_frontend::analysis::{AnalysisObservations, AnalysisResult, DeclaredAction};
use once_frontend::GraphTarget;

use super::super::runner::run_declared_actions;
use super::*;

#[cfg(unix)]
mod admission;
#[cfg(unix)]
mod arg_files;
#[cfg(unix)]
mod fingerprint_outputs;

#[test]
fn only_confirmed_absence_counts_as_a_missing_blob() {
    let digest = Digest::of_bytes(b"gone");
    let absent = anyhow::Error::from(once_cas::Error::BlobNotFound(digest)).context("restoring");
    assert_eq!(confirmed_missing_blob(&absent), Some(digest));
    let through_core =
        anyhow::Error::from(once_core::Error::Cas(once_cas::Error::BlobNotFound(digest)))
            .context("materializing");
    assert_eq!(confirmed_missing_blob(&through_core), Some(digest));

    let unavailable = anyhow::Error::from(once_cas::Error::Remote {
        provider: "tuist",
        operation: "get_blob",
        message: "UNAVAILABLE".to_string(),
    });
    assert_eq!(confirmed_missing_blob(&unavailable), None);
    let unreadable = anyhow::Error::from(once_core::Error::Cas(once_cas::Error::Io {
        path: PathBuf::from("blob"),
        source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
    }));
    assert_eq!(confirmed_missing_blob(&unreadable), None);
    let misconfigured = anyhow::Error::from(once_cas::Error::InvalidConfig {
        provider: "tuist",
        message: "token rejected".to_string(),
    });
    assert_eq!(confirmed_missing_blob(&misconfigured), None);
}

#[test]
fn only_absence_or_classified_unavailability_allows_output_recomputation() {
    for kind in [
        once_cas::ErrorKind::Unavailable,
        once_cas::ErrorKind::Refused,
        once_cas::ErrorKind::Corrupt,
        once_cas::ErrorKind::Protocol,
        once_cas::ErrorKind::Misconfigured,
        once_cas::ErrorKind::Exhausted,
        once_cas::ErrorKind::Io,
        once_cas::ErrorKind::Other,
    ] {
        for through_core in [false, true] {
            let error = once_cas::Error::RemoteFailure {
                provider: "tuist",
                operation: "get blob",
                kind,
                message: "test failure".into(),
            };
            let error = if through_core {
                anyhow::Error::from(once_core::Error::Cas(error))
            } else {
                anyhow::Error::from(error)
            }
            .context("restoring declared output");
            assert_eq!(
                can_recompute_output(&error),
                kind == once_cas::ErrorKind::Unavailable,
                "{kind}"
            );
            assert_eq!(
                confirmed_missing_blob(&error),
                None,
                "an outage never proves absence"
            );
        }
    }
}

#[test]
fn local_io_errors_do_not_inherit_unavailability_from_their_source() {
    let unavailable = || once_cas::Error::RemoteFailure {
        provider: "test",
        operation: "read",
        kind: once_cas::ErrorKind::Unavailable,
        message: "unavailable".into(),
    };
    let errors = [
        anyhow::Error::from(once_cas::Error::Io {
            path: "output".into(),
            source: std::io::Error::other(unavailable()),
        }),
        anyhow::Error::from(once_core::Error::ReadOutput {
            path: "output".into(),
            source: std::io::Error::other(unavailable()),
        }),
    ];
    for error in errors {
        assert!(!can_recompute_output(
            &error.context("local output operation failed")
        ));
    }
}

fn target() -> GraphTarget {
    GraphTarget {
        label: once_frontend::TargetLabel {
            package: "tools".to_string(),
            name: "recovering".to_string(),
            id: "tools/recovering".to_string(),
        },
        kind: "demo_kind".to_string(),
        deps: Vec::new(),
        dependency_edges: BTreeMap::new(),
        srcs: Vec::new(),
        visibility: Vec::new(),
        attrs: BTreeMap::new(),
        capabilities: Vec::new(),
        providers: Vec::new(),
        tools: Vec::new(),
        diagnostics: Vec::new(),
    }
}

fn action(value: serde_json::Value) -> DeclaredAction {
    let mut value = value;
    let defaults = serde_json::json!({
        "success_exit_codes": [0],
        "depends_on_prior_actions": false,
    });
    for (key, default) in defaults.as_object().unwrap() {
        value
            .as_object_mut()
            .unwrap()
            .entry(key.clone())
            .or_insert_with(|| default.clone());
    }
    serde_json::from_value(value).unwrap()
}

const PRODUCED: &str = ".once/out/produced.txt";

/// A cacheable producer, an uncacheable action that cleans what it produced,
/// and an uncacheable consumer that reads it, so the consumer always needs
/// the producer's output restored after the cache probe said it was there.
fn analysis(producer_command: &str) -> AnalysisResult {
    AnalysisResult {
        actions: vec![
            action(serde_json::json!({
                "identifier": "produce",
                "argv": ["/bin/sh", "-c", producer_command],
                "outputs": [PRODUCED],
                "create_dirs": [".once/out"],
                "cacheable": true,
            })),
            action(serde_json::json!({
                "identifier": "clean",
                "argv": ["/bin/sh", "-c", "printf cleaned > .once/out/cleaned.txt"],
                "outputs": [".once/out/cleaned.txt"],
                "clean_paths": [PRODUCED],
                "cacheable": false,
            })),
            action(serde_json::json!({
                "identifier": "consume",
                "argv": ["/bin/sh", "-c", format!("cat {PRODUCED} > .once/out/consumed.txt")],
                "inputs": [PRODUCED],
                "outputs": [".once/out/consumed.txt"],
                "cacheable": false,
            })),
        ],
        provider: serde_json::json!({}),
        declared_outputs: Vec::new(),
        observations: AnalysisObservations::default(),
    }
}

fn counting(content: &str) -> String {
    format!("printf {content} > {PRODUCED}; printf run >> runs.log")
}

struct Workspace {
    directory: tempfile::TempDir,
    cache: CacheProvider,
    digests: SourceDigestCache,
    resources: Arc<ResourcePool>,
}

impl Workspace {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let cache = CacheProvider::Local(once_cas::Cas::open(directory.path().join("cas")));
        let digests = SourceDigestCache::open(directory.path());
        Self {
            directory,
            cache,
            digests,
            resources: Self::resources(),
        }
    }

    fn path(&self) -> &Path {
        self.directory.path()
    }

    fn resources() -> Arc<ResourcePool> {
        Arc::new(ResourcePool::new(once_core::ResourceLimits::new(4, 0)))
    }

    /// Build with a registry the test keeps, so it can repair afterwards.
    async fn build_with(
        &self,
        analysis: AnalysisResult,
        producers: &Producers,
        bus: Option<&RunEventBus>,
    ) -> Result<super::super::BuildOutcome> {
        let resources = Arc::clone(&self.resources);
        let workers = crate::bus_events::WorkerSlots::new(4);
        super::super::run_declared_actions_with(
            None,
            self.path(),
            &self.cache,
            Digest::of_bytes(b"modules"),
            &target(),
            "build",
            analysis,
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            Some(&self.digests),
            SandboxMode::default(),
            &resources,
            None,
            bus,
            &workers,
            producers,
        )
        .await
    }

    /// Lose the producer's output from both the disk and the cache.
    fn lose(&self, digest: Digest) {
        std::fs::remove_file(self.path().join(PRODUCED)).unwrap();
        std::fs::remove_file(self.blob_path(digest)).unwrap();
    }

    async fn restore(&self, producers: &Producers, digest: Digest) -> Result<()> {
        let resources = Arc::clone(&self.resources);
        Repair {
            workspace: self.path(),
            cache: &self.cache,
            source_digest_cache: Some(&self.digests),
            resources: &resources,
            producers,
        }
        .restore(&single_output(PRODUCED, digest))
        .await
    }

    async fn build(&self, analysis: AnalysisResult) -> Result<super::super::BuildOutcome> {
        let resources = Arc::clone(&self.resources);
        let workers = crate::bus_events::WorkerSlots::new(4);
        run_declared_actions(
            None,
            self.path(),
            &self.cache,
            Digest::of_bytes(b"modules"),
            &target(),
            "build",
            analysis,
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            Some(&self.digests),
            SandboxMode::default(),
            &resources,
            None,
            None,
            &workers,
        )
        .await
    }

    fn runs(&self) -> usize {
        std::fs::read_to_string(self.path().join("runs.log"))
            .unwrap_or_default()
            .matches("run")
            .count()
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.path().join(path)).unwrap()
    }

    fn blob_path(&self, digest: Digest) -> PathBuf {
        let hex = digest.to_hex();
        let (prefix, rest) = hex.split_at(2);
        self.path().join("cas/cas").join(prefix).join(rest)
    }
}

#[cfg(unix)]
#[tokio::test]
async fn an_output_replaced_by_a_later_action_is_restored_for_its_reader() {
    let workspace = Workspace::new();

    let outcome = workspace.build(analysis(&counting("first"))).await.unwrap();

    assert_eq!(outcome.cache_state, EvidenceCacheState::Bypass);
    assert_eq!(workspace.read(".once/out/consumed.txt"), "first");
    assert_eq!(workspace.runs(), 1);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actions_with_different_digests_for_the_same_output_never_run_at_once() {
    let workspace = Workspace::new();
    let writer = |content: &str| AnalysisResult {
        actions: vec![action(serde_json::json!({
            "identifier": "produce",
            "argv": ["/bin/sh", "-c", format!(
                "mkdir .once/out/running && sleep 0.5 && printf {content} > {PRODUCED} && rmdir .once/out/running"
            )],
            "outputs": [PRODUCED],
            "create_dirs": [".once/out"],
            "cacheable": true,
        }))],
        provider: serde_json::json!({}),
        declared_outputs: Vec::new(),
        observations: AnalysisObservations::default(),
    };

    let (first, second) = tokio::join!(
        workspace.build(writer("first")),
        workspace.build(writer("second")),
    );

    first.unwrap();
    second.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn a_blob_evicted_after_the_probe_reruns_its_producer_once() {
    let workspace = Workspace::new();
    let first = workspace.build(analysis(&counting("first"))).await.unwrap();
    let blob = workspace.blob_path(first.result.outputs[PRODUCED]);
    std::fs::remove_file(&blob).unwrap();

    let second = workspace.build(analysis(&counting("first"))).await.unwrap();

    assert_eq!(workspace.read(".once/out/consumed.txt"), "first");
    assert_eq!(workspace.runs(), 2, "the producer ran again exactly once");
    assert!(blob.exists(), "running the producer put its bytes back");
    assert_eq!(
        second.per_action_outcomes[0].cache_state,
        EvidenceCacheState::Hit,
        "the producer's own probe still reported the hit it was"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_corrupt_blob_is_not_mistaken_for_a_missing_one() {
    let workspace = Workspace::new();
    let first = workspace.build(analysis(&counting("first"))).await.unwrap();
    std::fs::write(
        workspace.blob_path(first.result.outputs[PRODUCED]),
        b"garbage",
    )
    .unwrap();

    let error = workspace
        .build(analysis(&counting("first")))
        .await
        .unwrap_err();

    assert_eq!(workspace.runs(), 1, "corruption must not run the producer");
    assert!(confirmed_missing_blob(&error).is_none(), "{error:#}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_producer_that_writes_different_bytes_fails_closed() {
    let workspace = Workspace::new();
    // Keyed identically on every run, but no longer reproduces its cached
    // bytes once it has run before: a consumer keyed on the cached bytes
    // must not be handed these.
    let drifting = format!(
        "if [ -f runs.log ]; then printf second > {PRODUCED}; else printf first > {PRODUCED}; fi; printf run >> runs.log"
    );
    let first = workspace.build(analysis(&drifting)).await.unwrap();
    std::fs::remove_file(workspace.blob_path(first.result.outputs[PRODUCED])).unwrap();

    let error = workspace.build(analysis(&drifting)).await.unwrap_err();

    assert_eq!(workspace.runs(), 2);
    assert!(
        format!("{error:#}").contains("different bytes than its cached result"),
        "{error:#}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_cancelled_waiter_leaves_the_repair_running_and_it_is_not_run_twice() {
    let workspace = Workspace::new();
    let producers = Producers::default();
    let command = format!("sleep 1; printf first > {PRODUCED}; printf run >> runs.log");
    let first = workspace
        .build_with(analysis(&command), &producers, None)
        .await
        .unwrap();
    let digest = first.result.outputs[PRODUCED];
    workspace.lose(digest);

    let cancelled = tokio::time::timeout(
        Duration::from_millis(100),
        workspace.restore(&producers, digest),
    )
    .await;
    assert!(cancelled.is_err(), "the first waiter gave up");
    workspace.restore(&producers, digest).await.unwrap();

    assert_eq!(workspace.runs(), 2, "the repair ran once, for both waiters");
    assert_eq!(workspace.read(PRODUCED), "first");
}

#[cfg(unix)]
#[tokio::test]
async fn a_repair_that_runs_out_of_time_fails_and_is_not_attempted_again() {
    let workspace = Workspace::new();
    let producers = Producers::with_limits(RepairLimits {
        deadline: Some(Duration::from_millis(300)),
        ..RepairLimits::default()
    });
    let command = format!(
        "printf run >> runs.log; if [ -f slow ]; then sleep 30; fi; printf first > {PRODUCED}"
    );
    let first = workspace
        .build_with(analysis(&command), &producers, None)
        .await
        .unwrap();
    let digest = first.result.outputs[PRODUCED];
    workspace.lose(digest);
    std::fs::write(workspace.path().join("slow"), b"").unwrap();

    let started = Instant::now();
    let error = workspace.restore(&producers, digest).await.unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(
        format!("{error:#}").contains("did not finish within"),
        "{error:#}"
    );
    std::fs::remove_file(workspace.path().join("slow")).unwrap();
    let again = workspace.restore(&producers, digest).await.unwrap_err();

    assert!(
        format!("{again:#}").contains("did not finish within"),
        "{again:#}"
    );
    assert_eq!(
        workspace.runs(),
        2,
        "the timed-out repair is never run again"
    );
    assert!(!workspace.path().join(PRODUCED).exists());
}

#[cfg(unix)]
#[tokio::test]
async fn an_exhausted_repair_budget_fails_without_running_the_producer() {
    let workspace = Workspace::new();
    let producers = Producers::with_limits(RepairLimits {
        executions: 0,
        ..RepairLimits::default()
    });
    let first = workspace
        .build_with(analysis(&counting("first")), &producers, None)
        .await
        .unwrap();
    let digest = first.result.outputs[PRODUCED];
    workspace.lose(digest);

    let error = workspace.restore(&producers, digest).await.unwrap_err();

    assert!(
        format!("{error:#}").contains("faster than it can be repaired"),
        "{error:#}"
    );
    assert_eq!(workspace.runs(), 1);
}

#[tokio::test]
async fn producers_that_need_each_other_fail_instead_of_waiting_forever() {
    let workspace = Workspace::new();
    let producers = Producers::default();
    let (left, right) = (".once/out/left", ".once/out/right");
    let (left_digest, right_digest) = (Digest::of_bytes(b"left"), Digest::of_bytes(b"right"));
    let producer = |name: &str, output: (&str, Digest), input: (&str, Digest)| {
        let declared = action(serde_json::json!({
            "identifier": name,
            "argv": ["/bin/sh", "-c", format!("printf {name} > {}", output.0)],
            "outputs": [output.0],
            "cacheable": true,
        }));
        Producer {
            target_id: "tools/recovering".to_string(),
            capability: "build".to_string(),
            index: 0,
            identifier: name.to_string(),
            report: ProducerReport::default(),
            action: super::super::declared_to_action(
                workspace.path(),
                &declared,
                Digest::of_bytes(b"modules"),
                &[],
                SandboxMode::Off,
            )
            .unwrap(),
            setup: None,
            inputs: BTreeMap::from([(input.0.to_string(), input.1)]),
            outputs: BTreeMap::from([(output.0.to_string(), output.1)]),
        }
    };
    producers.record(producer("left", (left, left_digest), (right, right_digest)));
    producers.record(producer(
        "right",
        (right, right_digest),
        (left, left_digest),
    ));
    let resources = Arc::clone(&workspace.resources);
    let repair = Repair {
        workspace: workspace.path(),
        cache: &workspace.cache,
        source_digest_cache: None,
        resources: &resources,
        producers: &producers,
    };

    let error = tokio::time::timeout(
        Duration::from_secs(10),
        repair.restore(&single_output(left, left_digest)),
    )
    .await
    .expect("a cycle is refused rather than awaited")
    .unwrap_err();

    assert!(format!("{error:#}").contains("form a cycle"), "{error:#}");
    assert!(!workspace.path().join(left).exists());
}

#[cfg(unix)]
#[tokio::test]
async fn a_repair_is_reported_as_another_attempt_of_the_same_action() {
    let workspace = Workspace::new();
    let producers = Producers::default();
    let bus = RunEventBus::new(1024);
    let mut events = bus.subscribe();
    let mut reported = analysis(&counting("first"));
    reported.actions[0].display_name = Some("Produce the file".to_string());
    reported.actions[0].source_files = vec!["src/produced.in".to_string()];
    let first = workspace
        .build_with(reported, &producers, Some(&bus))
        .await
        .unwrap();
    let digest = first.result.outputs[PRODUCED];
    workspace.lose(digest);
    while events.try_recv().is_ok() {}

    workspace.restore(&producers, digest).await.unwrap();

    let mut started = None;
    let mut completed = None;
    while let Ok(event) = events.try_recv() {
        match event {
            RunEvent::ActionAttemptStarted {
                attempt,
                action_index,
                ..
            } => {
                started = Some((attempt, action_index));
            }
            RunEvent::ActionCompleted {
                identifier,
                display_name,
                source_files,
                selected_attempt,
                was_cached,
                cache_key,
                ..
            } => {
                completed = Some((
                    identifier,
                    display_name,
                    source_files,
                    selected_attempt,
                    was_cached,
                    cache_key,
                ));
            }
            _ => {}
        }
    }
    assert_eq!(started, Some((REPAIR_ATTEMPT, 0)));
    let (identifier, display_name, source_files, selected_attempt, was_cached, cache_key) =
        completed.expect("the repair completed as an action");
    assert_eq!(identifier.as_deref(), Some("produce"));
    assert_eq!(display_name.as_deref(), Some("Produce the file"));
    assert_eq!(source_files, ["src/produced.in"]);
    assert_eq!(selected_attempt, REPAIR_ATTEMPT);
    assert!(!was_cached);
    assert_eq!(
        cache_key,
        first.per_action_outcomes[0].action_digest.to_string()
    );
}
