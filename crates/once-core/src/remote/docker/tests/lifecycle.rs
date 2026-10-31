use std::sync::Arc;

use super::*;
use crate::{Executor, ExecutorResources, ResourceLimits, ResourcePool};

#[cfg(unix)]
#[path = "identity.rs"]
mod identity;

#[tokio::test]
async fn container_limits_match_the_normalized_worker_not_the_host() {
    let workers = Arc::new(ResourcePool::new(ResourceLimits::new(2, 123_456_789)));
    let worker = workers.acquire(ResourceRequest::new(100, u64::MAX)).await;
    let args = create_args(
        "once-unique",
        "unique",
        "sha256:immutable",
        worker.request(),
        Some(1500),
    );
    assert!(args.windows(2).any(|a| a == ["--memory", "123456789"]));
    assert!(args.windows(2).any(|a| a == ["--memory-swap", "123456789"]));
    assert!(args.windows(2).any(|a| a == ["--cpus", "2"]));
    assert!(args.iter().any(|a| a == "--pull=never"));
    assert_eq!(args.last().unwrap(), "122");
    assert!(!args
        .iter()
        .any(|a| a.contains("mount") || a.contains("docker.sock")));
}

#[cfg(unix)]
async fn mock_client(root: &std::path::Path) -> Client {
    use std::os::unix::fs::PermissionsExt;

    let binary = root.join("docker");
    std::fs::write(&binary, format!(r#"#!/bin/sh
root=$(dirname "$0")
id={}
case "$1" in
info)
    daemon=$(cat "$root/daemon" 2>/dev/null || printf '%s' "$root" | tr '/' '_')
    printf '{{"ID":"%s","NCPU":2,"MemTotal":3000}}\n' "$daemon";;
create)
    for arg do
        case "$arg" in once-operation=*) printf '%s' "${{arg#once-operation=}}" > "$root/nonce";; esac
    done
    touch "$root/alive"
    if test -e "$root/lost-response"; then exit 1; fi
    printf '%s\n' "$id";;
ps)
    if test -e "$root/switch-on-list"; then cp "$root/switch-on-list" "$root/daemon"; fi
    if test -e "$root/alive"; then printf '%s %s\n' "$id" "$(cat "$root/nonce")"; fi;;
rm)
    touch "$root/deleting"
    while test ! -e "$root/release"; do sleep 0.01; done
    if test -e "$root/fail"; then exit 1; fi
    rm -f "$root/alive";;
*) exit 1;;
esac
"#, "a".repeat(64))).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    Client::new(binary.into(), crate::RemoteIoLimits::default())
        .identify()
        .await
        .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_and_failed_deletion_retain_all_resources_until_reconciled() {
    let root = tempfile::tempdir().unwrap();
    let client = mock_client(root.path()).await;
    let host = ExecutorResources::new(Arc::new(ResourcePool::new(ResourceLimits::new(8, 1000))));
    let workers = Arc::new(ResourcePool::new(ResourceLimits::new(1, 100)));
    let worker = workers.acquire(ResourceRequest::new(1, 100)).await;
    let lease = host
        .acquire(
            Executor::Docker,
            &ResourceRequest::new(1, 100).with_slot("exclusive", 1),
        )
        .await
        .with_lifetime_owner(worker.clone());
    let (_, cleanup) = create(
        client.clone(),
        lifecycle::Journal::new(root.path()),
        "image".into(),
        lease,
        worker,
        None,
    )
    .await
    .unwrap();
    let deletion = tokio::spawn(cleanup.run());
    tokio::time::timeout(Duration::from_secs(5), async {
        while !root.path().join("deleting").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    deletion.abort();
    assert!(deletion.await.unwrap_err().is_cancelled());
    assert!(tokio::time::timeout(
        Duration::from_millis(20),
        workers.acquire(ResourceRequest::default())
    )
    .await
    .is_err());
    std::fs::write(root.path().join("fail"), b"").unwrap();
    std::fs::write(root.path().join("release"), b"").unwrap();
    assert!(tokio::time::timeout(
        Duration::from_millis(50),
        host.acquire(Executor::Docker, &ResourceRequest::new(8, 1000)),
    )
    .await
    .is_err());
    assert!(tokio::time::timeout(
        Duration::from_millis(50),
        host.acquire(
            Executor::Local,
            &ResourceRequest::new(1, 1).with_slot("exclusive", 1),
        ),
    )
    .await
    .is_err());
    assert!(tokio::time::timeout(
        Duration::from_millis(20),
        workers.acquire(ResourceRequest::default())
    )
    .await
    .is_err());
    std::fs::remove_file(root.path().join("fail")).unwrap();
    let journal = lifecycle::Journal::new(root.path());
    let _host = tokio::time::timeout(
        Duration::from_secs(5),
        host.acquire(
            Executor::Local,
            &ResourceRequest::new(8, 1000).with_slot("exclusive", 1),
        ),
    )
    .await
    .unwrap();
    let _next = tokio::time::timeout(
        Duration::from_secs(5),
        workers.acquire(ResourceRequest::default()),
    )
    .await
    .unwrap();
    assert!(journal.scan(100).unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn lost_create_response_keeps_worker_until_marker_reconciliation() {
    let root = tempfile::tempdir().unwrap();
    let client = mock_client(root.path()).await;
    std::fs::write(root.path().join("lost-response"), b"").unwrap();
    std::fs::write(root.path().join("release"), b"").unwrap();
    let host = ExecutorResources::new(Arc::new(ResourcePool::new(ResourceLimits::new(8, 1000))));
    let workers = Arc::new(ResourcePool::new(ResourceLimits::new(1, 100)));
    let worker = workers.acquire(ResourceRequest::default()).await;
    let lease = host
        .acquire(
            Executor::Docker,
            &ResourceRequest::default().with_slot("exclusive", 1),
        )
        .await
        .with_lifetime_owner(worker.clone());
    let journal = lifecycle::Journal::new(root.path());
    assert!(create(
        client.clone(),
        journal.clone(),
        "image".into(),
        lease,
        worker,
        None
    )
    .await
    .is_err());
    assert!(tokio::time::timeout(
        Duration::from_millis(20),
        workers.acquire(ResourceRequest::default())
    )
    .await
    .is_err());
    assert!(tokio::time::timeout(
        Duration::from_millis(50),
        host.acquire(Executor::Local, &ResourceRequest::new(8, 1000)),
    )
    .await
    .is_err());
    assert!(tokio::time::timeout(
        Duration::from_millis(50),
        host.acquire(
            Executor::Local,
            &ResourceRequest::new(1, 1).with_slot("exclusive", 1),
        ),
    )
    .await
    .is_err());
    let nonce = std::fs::read_to_string(root.path().join("nonce")).unwrap();
    std::fs::remove_file(root.path().join("alive")).unwrap();
    assert_eq!(client.find(&nonce).await.unwrap(), Lookup::Inconclusive);
    assert!(tokio::time::timeout(
        Duration::from_millis(20),
        workers.acquire(ResourceRequest::default())
    )
    .await
    .is_err());
    std::fs::write(root.path().join("alive"), b"").unwrap();
    lifecycle::reconcile(&journal, &client, lifecycle::Budget::default()).await;
    let _next = tokio::time::timeout(
        Duration::from_secs(5),
        workers.acquire(ResourceRequest::default()),
    )
    .await
    .unwrap();
    assert!(journal.scan(100).unwrap().is_empty());
}
