use std::time::Duration;

use super::*;
use crate::remote::lifecycle::{Lookup, Provider};

async fn client() -> process::Client {
    process::Client::new(
        std::env::var_os("ONCE_DOCKER_BIN").unwrap_or_else(|| "docker".into()),
        RemoteIoLimits::default(),
    )
    .connect()
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires Docker and ONCE_DOCKER_TEST_IMAGE"]
async fn real_docker_daemon_admission_and_container_limits_match() {
    let workspace = tempfile::tempdir().unwrap();
    let client = client().await;
    let workers = capacity::pool(&client).await.unwrap();
    let host = ExecutorResources::new(Arc::new(ResourcePool::new(ResourceLimits::new(
        128,
        64 * 1024 * 1024 * 1024,
    ))));
    let lease = host
        .acquire(Executor::Docker, &ResourceRequest::new(128, u64::MAX))
        .await;
    let (lease, worker) = capacity::admit(&client, lease).await.unwrap();
    let expected = worker.request().clone();
    assert!(expected.memory_bytes <= workers.limits().memory_bytes);
    assert!(expected.cpu_slots <= workers.limits().cpu_slots);
    let (container, cleanup) = lifecycle::create(
        client.clone(),
        Journal::new(workspace.path()),
        real_image(),
        lease,
        worker,
        None,
    )
    .await
    .unwrap();
    let configured = client
        .control(&[
            "inspect".into(),
            "--format".into(),
            "{{.HostConfig.Memory}} {{.HostConfig.NanoCpus}}".into(),
            container.0.clone(),
        ])
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8(configured).unwrap().trim(),
        format!(
            "{} {}",
            expected.memory_bytes,
            expected.cpu_slots as u64 * 1_000_000_000
        )
    );
    client
        .control(&["start".into(), container.0])
        .await
        .unwrap();
    assert!(tokio::time::timeout(
        Duration::from_millis(20),
        workers.acquire(ResourceRequest::new(128, u64::MAX))
    )
    .await
    .is_err());
    cleanup.run().await.unwrap();
    let _next = tokio::time::timeout(
        Duration::from_secs(5),
        workers.acquire(ResourceRequest::new(128, u64::MAX)),
    )
    .await
    .unwrap();
    assert_no_journal_records(workspace.path());
}

#[tokio::test]
#[ignore = "requires Docker and ONCE_DOCKER_TEST_IMAGE"]
async fn real_docker_cancellation_holds_admission_until_container_deletion() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(workspace.path().join("cache"));
    let pool = resources();
    let action = action(
        &["/bin/sh", "-c", "touch /tmp/once-ready; sleep 60"],
        real_image(),
    );
    let lease = pool
        .acquire(Executor::Docker, action.resource_request())
        .await;
    let mut execution = Box::pin(crate::run_uncached_admitted(
        &action,
        workspace.path(),
        &cache,
        false,
        None,
        lease,
    ));
    let journal = Journal::new(workspace.path());
    let client = client().await;
    let ready = async {
        loop {
            for nonce in journal.scan(100).unwrap() {
                let path = workspace
                    .path()
                    .join(format!(".once/executors/lifecycle/{nonce}.json"));
                if let Ok(bytes) = std::fs::read(path) {
                    let record: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                    if let Some(id) = record["sandbox_ids"][0].as_str() {
                        if client
                            .control(&[
                                "exec".into(),
                                id.into(),
                                "test".into(),
                                "-f".into(),
                                "/tmp/once-ready".into(),
                            ])
                            .await
                            .is_ok()
                        {
                            return nonce;
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    };
    let nonce = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::select! {
            result = &mut execution => panic!("action completed before cancellation: {result:?}"),
            nonce = ready => nonce,
        }
    })
    .await
    .unwrap();
    assert!(tokio::time::timeout(
        Duration::from_millis(20),
        pool.acquire(Executor::Docker, action.resource_request())
    )
    .await
    .is_err());
    drop(execution);
    let _next = tokio::time::timeout(
        Duration::from_secs(15),
        pool.acquire(Executor::Docker, action.resource_request()),
    )
    .await
    .unwrap();
    assert_no_journal_records(workspace.path());
    assert_eq!(client.find(&nonce).await.unwrap(), Lookup::Absent);
}

#[tokio::test]
#[ignore = "requires Docker and ONCE_DOCKER_TEST_IMAGE"]
async fn real_docker_timeout_and_capture_limits_still_delete_containers() {
    for capture_limit in [None, Some(8192)] {
        let workspace = tempfile::tempdir().unwrap();
        let cache = CacheProvider::open_local(workspace.path().join("cache"));
        let script = if capture_limit.is_some() {
            "head -c 16384 /dev/zero"
        } else {
            "sleep 60"
        };
        let mut action = action(&["/bin/sh", "-c", script], real_image());
        let limits = RemoteIoLimits {
            max_capture_bytes: capture_limit.unwrap_or(1024 * 1024),
            ..RemoteIoLimits::default()
        };
        let pool = resources().with_remote_io_limits(limits);
        if capture_limit.is_none() {
            if let Action::RunCommand { timeout_ms, .. } = &mut action {
                *timeout_ms = Some(2000);
            }
        }
        let lease = pool
            .acquire(Executor::Docker, action.resource_request())
            .await;
        let result =
            crate::run_uncached_admitted(&action, workspace.path(), &cache, false, None, lease)
                .await;
        if capture_limit.is_some() {
            assert!(
                matches!(result, Err(Error::ExecutorResourceLimit { .. })),
                "{result:?}"
            );
        } else {
            assert!(matches!(result, Err(Error::Timeout(_))), "{result:?}");
        }
        let _next = tokio::time::timeout(
            Duration::from_secs(15),
            pool.acquire(Executor::Docker, action.resource_request()),
        )
        .await
        .unwrap();
        assert_no_journal_records(workspace.path());
    }
}
