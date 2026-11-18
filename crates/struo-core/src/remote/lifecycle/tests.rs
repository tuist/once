use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::remote::test_server;
use crate::{Executor, ExecutorResources, ResourceRequest};

/// Parsed lifecycle records in a workspace, for assertions.
pub(in crate::remote) fn records(workspace: &Path) -> Vec<serde_json::Value> {
    let Ok(entries) = std::fs::read_dir(workspace.join(".struo/executors/lifecycle")) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension()? == "json")
                .then(|| serde_json::from_slice(&std::fs::read(path).ok()?).ok())?
        })
        .collect()
}

/// Journals an intent the way a create whose response was lost leaves it.
pub(in crate::remote) fn intent(journal: &Journal) -> String {
    let nonce = journal
        .begin("pending", "pending", "pending", Duration::ZERO)
        .unwrap()
        .nonce()
        .to_string();
    exited(journal);
    nonce
}

/// Simulates every earlier owner having exited by unlinking the record locks.
///
/// Dropping a lock is not enough in a test binary: any process spawned
/// concurrently by another test inherits open descriptors between fork and
/// exec, and an inherited descriptor keeps the flock held for that window, so
/// a reconciliation pass would see the record as active. That transient
/// "active" is correct and harmless in production, where the record is simply
/// retried by a later pass, but it makes exact report assertions racy. A fresh
/// lock file cannot be held through a descriptor that predates it.
pub(in crate::remote) fn exited(journal: &Journal) {
    let Ok(entries) = std::fs::read_dir(journal.root()) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_some_and(|extension| extension == "lock")
        {
            std::fs::remove_file(path).unwrap();
        }
    }
}

/// Assigns every record to a provider scope, for tests whose endpoint is only
/// known after the record exists.
pub(in crate::remote) fn rescope(workspace: &Path, provider: &str, scope: &str, credential: &str) {
    rewrite(workspace, |record| {
        record["provider"] = provider.into();
        record["scope"] = scope.into();
        record["credential"] = credential.into();
    });
}

/// Marks every record as having created `id`, as a journal surviving a
/// restart would after the provider returned it.
pub(in crate::remote) fn mark_created(workspace: &Path, id: &str) {
    rewrite(workspace, |record| {
        record["phase"] = "created".into();
        record["sandbox_ids"] = serde_json::json!([id]);
    });
}

fn rewrite(workspace: &Path, change: impl Fn(&mut serde_json::Value)) {
    for entry in std::fs::read_dir(workspace.join(".struo/executors/lifecycle")).unwrap() {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            let mut record: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            change(&mut record);
            std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        }
    }
}

/// A request seen by [`capture`], with lowercase header names.
pub(in crate::remote) struct Captured {
    pub method: String,
    pub path: String,
    pub headers: std::collections::BTreeMap<String, String>,
}

/// Serves `(status, json body)` responses in order and records each request's
/// headers, so tests can assert that control calls are authenticated.
pub(in crate::remote) async fn capture(
    responses: Vec<(u16, String)>,
) -> (String, tokio::task::JoinHandle<Vec<Captured>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut captured = Vec::new();
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                let mut chunk = [0_u8; 4096];
                let read = stream.read(&mut chunk).await.unwrap();
                assert!(read > 0, "request closed before its headers arrived");
                bytes.extend_from_slice(&chunk[..read]);
            }
            let head = String::from_utf8_lossy(&bytes).to_string();
            let mut lines = head.split("\r\n");
            let mut first = lines.next().unwrap().split_whitespace();
            captured.push(Captured {
                method: first.next().unwrap().to_string(),
                path: first.next().unwrap().to_string(),
                headers: lines
                    .filter_map(|line| line.split_once(':'))
                    .map(|(name, value)| {
                        (name.trim().to_ascii_lowercase(), value.trim().to_string())
                    })
                    .collect(),
            });
            let response = format!(
                "HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
        captured
    });
    (url, task)
}

struct State {
    credential: &'static str,
    ignores_filter: bool,
    sandboxes: Vec<(String, Option<String>)>,
    looked_up: Vec<String>,
    lookups: usize,
    deleted: Vec<String>,
    delete_failures: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            credential: "key",
            ignores_filter: false,
            sandboxes: Vec::new(),
            looked_up: Vec::new(),
            lookups: 0,
            deleted: Vec::new(),
            delete_failures: 0,
        }
    }
}

#[derive(Clone, Default)]
struct Fake(Arc<Mutex<State>>);

impl Fake {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap()
    }
}

impl Provider for Fake {
    const NAME: &'static str = "fake";

    fn scope(&self) -> String {
        "scope".to_string()
    }

    fn credential(&self) -> String {
        self.state().credential.to_string()
    }

    fn find(&self, nonce: &str) -> impl Future<Output = Result<Lookup>> + Send {
        let mut state = self.state();
        state.lookups += 1;
        state.looked_up.push(nonce.to_string());
        let ignores_filter = state.ignores_filter;
        let lookup = classify(
            state
                .sandboxes
                .iter()
                .filter(|(_, marker)| ignores_filter || marker.as_deref() == Some(nonce))
                .map(|(id, marker)| (id.as_str(), marker.as_deref(), true)),
            nonce,
            false,
        );
        std::future::ready(Ok(lookup))
    }

    fn delete(&self, id: &str) -> impl Future<Output = Result<()>> + Send {
        let mut state = self.state();
        let result = if state.delete_failures > 0 {
            state.delete_failures -= 1;
            Err(Error::RemoteProviderStatus {
                provider: "fake".to_string(),
                status: 503,
            })
        } else {
            state.sandboxes.retain(|(existing, _)| existing != id);
            state.deleted.push(id.to_string());
            Ok(())
        };
        std::future::ready(result)
    }
}

struct Sandbox(String);

impl HostedSandbox for Sandbox {
    fn id(&self) -> &str {
        &self.0
    }
}

fn budget() -> Budget {
    Budget::default()
}

async fn admit(resources: &ExecutorResources) -> ExecutionPermit {
    resources
        .acquire(Executor::E2b, &ResourceRequest::default())
        .await
}

#[tokio::test]
async fn committed_create_with_a_lost_response_is_deleted_on_the_next_run() {
    let workspace = tempfile::tempdir().unwrap();
    let journal = Journal::new(workspace.path());
    let provider = Fake::default();
    let remote = provider.clone();
    let error = create_owned(
        provider.clone(),
        Some(journal.clone()),
        Duration::ZERO,
        None,
        move |_, marker| async move {
            remote
                .state()
                .sandboxes
                .push(("lost".to_string(), marker.map(|marker| marker.nonce)));
            Err::<Sandbox, _>(Error::RemoteProviderHttp {
                provider: "fake".to_string(),
                source: reqwest::get("http://127.0.0.1:1").await.unwrap_err(),
            })
        },
    )
    .await
    .err()
    .unwrap();
    assert!(matches!(error, Error::RemoteProviderHttp { .. }));
    assert_eq!(records(workspace.path())[0]["phase"], "intent");

    exited(&journal);
    let report = reconcile(&journal, &provider, budget()).await;

    assert_eq!(report.deleted, 1);
    assert_eq!(provider.state().deleted, ["lost"]);
    assert!(records(workspace.path()).is_empty());
}

#[tokio::test]
async fn failed_deletion_stays_journaled_until_a_later_run_repairs_it() {
    let workspace = tempfile::tempdir().unwrap();
    let journal = Journal::new(workspace.path());
    let provider = Fake::default();
    provider.state().delete_failures = 3;
    let mut entry = journal
        .begin(Fake::NAME, "scope", "key", Duration::ZERO)
        .unwrap();
    entry.mark_created(&["kept".to_string()]).unwrap();
    let marker = Some(entry.nonce().to_string());
    provider
        .state()
        .sandboxes
        .push(("kept".to_string(), marker));

    let error = Cleanup::journaled(provider.clone(), "kept".to_string(), None, Some(entry))
        .run()
        .await
        .unwrap_err();
    let retained = records(workspace.path());
    exited(&journal);
    let report = reconcile(&journal, &provider, budget()).await;

    assert!(error.to_string().contains("503"));
    assert_eq!(retained[0]["phase"], "created");
    assert_eq!(retained[0]["attempts"], 1);
    assert_eq!(report.deleted, 1);
    assert_eq!(provider.state().deleted, ["kept"]);
    assert!(records(workspace.path()).is_empty());
}

#[tokio::test]
async fn foreign_markers_and_scopes_are_left_alone() {
    let workspace = tempfile::tempdir().unwrap();
    let journal = Journal::new(workspace.path());
    let provider = Fake::default();
    provider.state().ignores_filter = true;
    provider.state().sandboxes.push((
        "foreign".to_string(),
        Some("0123456789abcdef0123456789abcdef".to_string()),
    ));
    provider
        .state()
        .sandboxes
        .push(("unlabelled".to_string(), None));
    drop(
        journal
            .begin(Fake::NAME, "scope", "key", Duration::ZERO)
            .unwrap(),
    );
    drop(
        journal
            .begin(Fake::NAME, "another-endpoint", "key", Duration::ZERO)
            .unwrap(),
    );

    exited(&journal);
    let report = reconcile(&journal, &provider, budget()).await;

    assert_eq!(report.foreign, 1);
    assert_eq!(report.retained, 1);
    assert_eq!(provider.state().lookups, 1);
    assert!(provider.state().deleted.is_empty());
    assert_eq!(records(workspace.path()).len(), 2);
}

#[tokio::test]
async fn absence_is_accepted_only_after_the_record_settles() {
    let workspace = tempfile::tempdir().unwrap();
    let journal = Journal::new(workspace.path());
    let provider = Fake::default();
    drop(
        journal
            .begin(Fake::NAME, "scope", "key", Duration::from_hours(1))
            .unwrap(),
    );

    exited(&journal);
    let early = reconcile(&journal, &provider, budget()).await;
    assert_eq!(early.retained, 1);
    assert_eq!(records(workspace.path()).len(), 1);

    std::fs::remove_dir_all(workspace.path().join(".struo")).unwrap();
    drop(
        journal
            .begin(Fake::NAME, "scope", "key", Duration::ZERO)
            .unwrap(),
    );
    exited(&journal);
    let settled = reconcile(&journal, &provider, budget()).await;

    assert_eq!(settled.absent, 1);
    assert!(records(workspace.path()).is_empty());
}

#[tokio::test]
async fn records_held_by_a_live_owner_are_skipped() {
    let workspace = tempfile::tempdir().unwrap();
    let journal = Journal::new(workspace.path());
    let provider = Fake::default();
    let owned = journal
        .begin(Fake::NAME, "scope", "key", Duration::ZERO)
        .unwrap();

    let report = reconcile(&journal, &provider, budget()).await;

    assert_eq!(report.active, 1);
    assert_eq!(provider.state().lookups, 0);
    assert_eq!(records(workspace.path()).len(), 1);
    drop(owned);
}

#[tokio::test]
async fn cancelling_a_create_still_deletes_the_late_sandbox_and_holds_the_lease() {
    let workspace = tempfile::tempdir().unwrap();
    let journal = Journal::new(workspace.path());
    let provider = Fake::default();
    let resources = test_server::single_remote_quota();
    let lease = admit(&resources).await;
    let (started_tx, started) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel::<()>();
    let creation = create_owned(
        provider.clone(),
        Some(journal),
        Duration::ZERO,
        Some(lease),
        move |_, _| async move {
            let _ = started_tx.send(());
            let _ = released.await;
            Ok(Sandbox("late".to_string()))
        },
    );
    let mut creation = Box::pin(creation);
    test_server::assert_ready(async {
        tokio::select! {
            _ = &mut creation => panic!("create finished while held"),
            _ = started => {}
        }
    })
    .await;

    drop(creation);
    test_server::assert_pending(admit(&resources)).await;
    release.send(()).unwrap();
    test_server::assert_ready(admit(&resources)).await;

    assert_eq!(provider.state().deleted, ["late"]);
    assert!(records(workspace.path()).is_empty());
}

#[tokio::test]
async fn dropping_a_created_sandbox_deletes_it() {
    let workspace = tempfile::tempdir().unwrap();
    let provider = Fake::default();
    let created = create_owned(
        provider.clone(),
        Some(Journal::new(workspace.path())),
        Duration::ZERO,
        None,
        |_, _| async { Ok(Sandbox("dropped".to_string())) },
    )
    .await
    .unwrap();

    drop(created);
    test_server::assert_ready(async {
        while !records(workspace.path()).is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;

    assert_eq!(provider.state().deleted, ["dropped"]);
}

#[tokio::test]
async fn corrupt_records_are_quarantined_within_a_bound() {
    let workspace = tempfile::tempdir().unwrap();
    let journal = Journal::new(workspace.path());
    let provider = Fake::default();
    let directory = workspace.path().join(".struo/executors/lifecycle");
    std::fs::create_dir_all(&directory).unwrap();
    for index in 0..24 {
        let contents = if index == 0 {
            vec![b' '; 64 * 1024]
        } else {
            b"{not json".to_vec()
        };
        std::fs::write(directory.join(format!("{index:032x}.json")), contents).unwrap();
    }
    std::fs::write(directory.join("not-a-nonce.json"), b"{}").unwrap();

    exited(&journal);
    let report = reconcile(&journal, &provider, budget()).await;

    assert_eq!(report.quarantined, 24);
    assert_eq!(provider.state().lookups, 0);
    assert!(
        std::fs::read_dir(directory.join("quarantine"))
            .unwrap()
            .count()
            <= 16
    );
    assert!(directory.join("not-a-nonce.json").exists());
}

#[test]
fn only_a_local_pre_send_refusal_proves_a_create_was_rejected() {
    let status = |status| Error::RemoteProviderStatus {
        provider: "fake".to_string(),
        status,
    };
    for code in [400, 401, 403, 404, 409, 422, 408, 429, 500, 503] {
        assert!(!rejected(&status(code)), "status {code}");
    }
    assert!(rejected(&Error::ExecutorResourceLimit {
        executor: "fake".to_string(),
        resource: "request metadata bytes",
        limit: 1,
    }));
    assert!(!rejected(&Error::ExecutorResourceLimit {
        executor: "fake".to_string(),
        resource: "response bytes",
        limit: 1,
    }));
}

#[tokio::test]
async fn a_create_refused_after_it_committed_is_found_by_marker_and_deleted() {
    for code in [400, 403] {
        let workspace = tempfile::tempdir().unwrap();
        let journal = Journal::new(workspace.path());
        let provider = Fake::default();
        let remote = provider.clone();
        let error = create_owned(
            provider.clone(),
            Some(journal.clone()),
            Duration::from_hours(1),
            None,
            move |_, marker| async move {
                remote
                    .state()
                    .sandboxes
                    .push(("committed".to_string(), marker.map(|marker| marker.nonce)));
                Err::<Sandbox, _>(Error::RemoteProviderStatus {
                    provider: "fake".to_string(),
                    status: code,
                })
            },
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(error, Error::RemoteProviderStatus { status, .. } if status == code));
        assert_eq!(
            records(workspace.path())[0]["phase"],
            "intent",
            "status {code}"
        );

        exited(&journal);
        let report = reconcile(&journal, &provider, budget()).await;

        assert_eq!(report.deleted, 1, "status {code}");
        assert_eq!(provider.state().deleted, ["committed"]);
        assert!(records(workspace.path()).is_empty());
    }
}

#[tokio::test]
async fn a_refused_create_without_a_sandbox_keeps_its_intent_until_it_settles() {
    let workspace = tempfile::tempdir().unwrap();
    let journal = Journal::new(workspace.path());
    let provider = Fake::default();
    create_owned(
        provider.clone(),
        Some(journal.clone()),
        Duration::from_hours(1),
        None,
        |_, _| async {
            Err::<Sandbox, _>(Error::RemoteProviderStatus {
                provider: "fake".to_string(),
                status: 400,
            })
        },
    )
    .await
    .err()
    .unwrap();

    exited(&journal);
    let report = reconcile(&journal, &provider, budget()).await;

    assert_eq!(report.retained, 1);
    assert!(provider.state().deleted.is_empty());
    assert_eq!(records(workspace.path())[0]["phase"], "intent");
}

#[tokio::test]
async fn an_oversized_marker_is_refused_before_sending_and_leaves_no_record() {
    let workspace = tempfile::tempdir().unwrap();
    create_owned(
        Fake::default(),
        Some(Journal::new(workspace.path())),
        Duration::ZERO,
        None,
        |_, _| async {
            Err::<Sandbox, _>(Error::ExecutorResourceLimit {
                executor: "fake".to_string(),
                resource: "request metadata bytes",
                limit: 1,
            })
        },
    )
    .await
    .err()
    .unwrap();

    assert!(records(workspace.path()).is_empty());
}

#[test]
fn listings_prove_absence_only_when_complete_and_filtered() {
    let nonce = "0123456789abcdef0123456789abcdef";
    assert_eq!(classify([], nonce, false), Lookup::Absent);
    assert_eq!(classify([], nonce, true), Lookup::Inconclusive);
    assert_eq!(
        classify([("other", None, true)], nonce, false),
        Lookup::Inconclusive
    );
    assert_eq!(
        classify(
            [("ours", Some(nonce), true), ("other", None, true)],
            nonce,
            false
        ),
        Lookup::Found(vec!["ours".to_string()])
    );
    assert_eq!(
        classify([("gone", Some(nonce), false)], nonce, false),
        Lookup::Absent
    );
}

#[tokio::test]
async fn a_created_record_naming_a_foreign_id_never_deletes_it() {
    let workspace = tempfile::tempdir().unwrap();
    let journal = Journal::new(workspace.path());
    let provider = Fake::default();
    provider.state().sandboxes.push((
        "reused".to_string(),
        Some("0123456789abcdef0123456789abcdef".to_string()),
    ));
    provider
        .state()
        .sandboxes
        .push(("unlabelled".to_string(), None));
    for (id, settle) in [
        ("reused", Duration::from_hours(1)),
        ("unlabelled", Duration::ZERO),
    ] {
        let mut entry = journal.begin(Fake::NAME, "scope", "key", settle).unwrap();
        entry.mark_created(&[id.to_string()]).unwrap();
    }

    exited(&journal);
    let report = reconcile(&journal, &provider, budget()).await;

    assert!(provider.state().deleted.is_empty());
    assert_eq!(provider.state().lookups, 2);
    assert_eq!(report.retained, 1);
    assert_eq!(report.absent, 1);
    assert_eq!(records(workspace.path())[0]["sandbox_ids"][0], "reused");
}

#[tokio::test]
async fn a_rotated_credential_repairs_verified_sandboxes_but_does_not_trust_absence() {
    let workspace = tempfile::tempdir().unwrap();
    let journal = Journal::new(workspace.path());
    let provider = Fake::default();
    let found = journal
        .begin(Fake::NAME, "scope", "old-key", Duration::ZERO)
        .unwrap();
    provider
        .state()
        .sandboxes
        .push(("kept".to_string(), Some(found.nonce().to_string())));
    drop(found);
    drop(
        journal
            .begin(Fake::NAME, "scope", "old-key", Duration::ZERO)
            .unwrap(),
    );

    exited(&journal);
    let report = reconcile(&journal, &provider, budget()).await;

    assert_eq!(report.deleted, 1);
    assert_eq!(report.retained, 1);
    assert_eq!(provider.state().deleted, ["kept"]);
    assert_eq!(records(workspace.path()).len(), 1);
}

#[tokio::test]
async fn absence_under_another_account_is_never_trusted_however_old_the_record() {
    let workspace = tempfile::tempdir().unwrap();
    let journal = Journal::new(workspace.path());
    let provider = Fake::default();
    drop(
        journal
            .begin(Fake::NAME, "scope", "other-account-key", Duration::ZERO)
            .unwrap(),
    );
    let long_ago = journal::now() - 30 * 24 * 60 * 60;
    rewrite(workspace.path(), |record| {
        record["created_at"] = long_ago.into();
        record["settles_at"] = long_ago.into();
    });

    exited(&journal);
    let report = reconcile(&journal, &provider, budget()).await;

    assert_eq!(provider.state().lookups, 1);
    assert_eq!(report.absent, 0);
    assert_eq!(report.retained, 1);
    assert!(provider.state().deleted.is_empty());
    assert_eq!(records(workspace.path())[0]["phase"], "intent");
}

#[tokio::test]
async fn the_intent_is_flushed_before_the_create_request_is_sent() {
    let workspace = tempfile::tempdir().unwrap();
    let journal = Journal::durable(workspace.path());
    let probe = journal.clone();
    let directory = workspace.path().to_path_buf();

    let (_, cleanup) = create_owned(
        Fake::default(),
        Some(journal.clone()),
        Duration::ZERO,
        None,
        move |_, marker| async move {
            let journaled = records(&directory);
            assert_eq!(journaled.len(), 1);
            assert_eq!(journaled[0]["phase"], "intent");
            assert_eq!(journaled[0]["nonce"], marker.unwrap().nonce.as_str());
            assert_eq!(probe.syncs(), 2, "record contents and directory entry");
            Ok(Sandbox("synced".to_string()))
        },
    )
    .await
    .unwrap();

    assert_eq!(journal.syncs(), 3, "the created id flushes its contents");
    assert_eq!(records(workspace.path())[0]["phase"], "created");
    cleanup.run().await.unwrap();
    assert!(records(workspace.path()).is_empty());
}

const CRASH_CHILD: &str = "STRUO_LIFECYCLE_CRASH_CHILD";

/// Journals an intent through the production flush path in a child process
/// that then exits without running destructors, as a crash after the intent
/// was recorded and before any response arrived would.
#[test]
fn a_durable_intent_survives_an_exited_process_and_is_repaired() {
    if let Some(workspace) = std::env::var_os(CRASH_CHILD) {
        let journal = Journal::durable(Path::new(&workspace));
        let entry = journal
            .begin(Fake::NAME, "scope", "key", Duration::ZERO)
            .unwrap();
        assert_eq!(journal.syncs(), 2);
        std::mem::forget(entry);
        std::process::exit(17);
    }
    let workspace = tempfile::tempdir().unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "remote::lifecycle::tests::a_durable_intent_survives_an_exited_process_and_is_repaired",
            "--test-threads=1",
        ])
        .env(CRASH_CHILD, workspace.path())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(17));

    let journaled = records(workspace.path());
    assert_eq!(journaled.len(), 1);
    assert_eq!(journaled[0]["phase"], "intent");
    let nonce = journaled[0]["nonce"].as_str().unwrap().to_string();
    let provider = Fake::default();
    provider
        .state()
        .sandboxes
        .push(("crashed".to_string(), Some(nonce)));

    let report = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(reconcile(
            &Journal::durable(workspace.path()),
            &provider,
            budget(),
        ));

    assert_eq!(report.active, 0, "the exited owner's lock was released");
    assert_eq!(report.deleted, 1);
    assert_eq!(provider.state().deleted, ["crashed"]);
    assert!(records(workspace.path()).is_empty());
}

#[tokio::test]
async fn a_full_journal_fails_closed_before_any_create() {
    let workspace = tempfile::tempdir().unwrap();
    let journal = Journal::new(workspace.path());
    let directory = workspace.path().join(".struo/executors/lifecycle");
    std::fs::create_dir_all(&directory).unwrap();
    for index in 0..journal::MAX_RECORDS {
        std::fs::write(directory.join(format!("{index:032x}.json")), b"{}").unwrap();
    }
    let created = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = created.clone();

    let error = create_owned(
        Fake::default(),
        Some(journal.clone()),
        Duration::ZERO,
        None,
        move |_, _| async move {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(Sandbox("paid".to_string()))
        },
    )
    .await
    .err()
    .unwrap();

    assert!(matches!(error, Error::ExecutorResourceLimit { .. }));
    assert!(!created.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(records(workspace.path()).len(), journal::MAX_RECORDS);

    std::fs::remove_dir_all(&directory).unwrap();
    std::fs::create_dir_all(&directory).unwrap();
    for index in 0..=journal::MAX_ENTRIES {
        std::fs::write(directory.join(format!("junk-{index}")), b"").unwrap();
    }
    assert!(matches!(
        journal
            .begin(Fake::NAME, "scope", "key", Duration::ZERO)
            .err()
            .unwrap(),
        Error::ExecutorResourceLimit { .. }
    ));
}

#[tokio::test]
async fn passes_rotate_through_records_they_cannot_resolve() {
    let workspace = tempfile::tempdir().unwrap();
    let journal = Journal::new(workspace.path());
    let provider = Fake::default();
    provider.state().ignores_filter = true;
    provider
        .state()
        .sandboxes
        .push(("unlabelled".to_string(), None));
    for _ in 0..3 {
        drop(
            journal
                .begin(Fake::NAME, "scope", "key", Duration::ZERO)
                .unwrap(),
        );
    }
    let one = Budget {
        records: 1,
        ..budget()
    };

    for _ in 0..3 {
        exited(&journal);
        reconcile(&journal, &provider, one).await;
    }

    let mut looked_up = provider.state().looked_up.clone();
    looked_up.sort();
    looked_up.dedup();
    assert_eq!(looked_up.len(), 3);
}
