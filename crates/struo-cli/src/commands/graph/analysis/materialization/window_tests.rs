use super::*;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;
use struo_cas::Digest;
use struo_core::ResourceLimits;

fn result(path: &str) -> ActionResult {
    ActionResult {
        exit_code: 0,
        stdout: None,
        stderr: None,
        outputs: BTreeMap::from([(path.to_string(), Digest::of_bytes(path.as_bytes()))]),
    }
}

#[cfg(not(windows))]
#[tokio::test]
async fn completed_members_refill_before_their_slow_peer_finishes() {
    let workspace = tempfile::tempdir().unwrap();
    let results = vec![result("large"), result("small-one"), result("small-two")];
    if !paths::disjoint(
        workspace.path(),
        &results[0].outputs.keys().cloned().collect(),
        &results[2].outputs.keys().cloned().collect(),
    ) {
        return;
    }
    let resources = Arc::new(ResourcePool::new(ResourceLimits::new(
        2,
        1024 * 1024 * 1024,
    )));
    let (release, wait) = tokio::sync::oneshot::channel();
    let wait = Arc::new(Mutex::new(Some(wait)));
    let (started, ready) = tokio::sync::oneshot::channel();
    let started = Arc::new(Mutex::new(Some(started)));
    let root = workspace.path().to_path_buf();
    let task = tokio::spawn(async move {
        run(&results, &root, &resources, move |index, permit| {
            let wait = Arc::clone(&wait);
            let started = Arc::clone(&started);
            async move {
                let _permit = permit;
                if index == 0 {
                    let gate = wait.lock().unwrap().take().unwrap();
                    gate.await.unwrap();
                } else if index == 2 {
                    started.lock().unwrap().take().unwrap().send(()).unwrap();
                }
                Ok(())
            }
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    release.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn first_failure_stops_refills_and_waits_for_admitted_peers() {
    let workspace = tempfile::tempdir().unwrap();
    let results = vec![result("a"), result("b"), result("c")];
    if !paths::disjoint(
        workspace.path(),
        &results[0].outputs.keys().cloned().collect(),
        &results[1].outputs.keys().cloned().collect(),
    ) {
        return;
    }
    let resources = Arc::new(ResourcePool::new(ResourceLimits::new(
        2,
        1024 * 1024 * 1024,
    )));
    let (release, wait) = tokio::sync::oneshot::channel();
    let wait = Arc::new(Mutex::new(Some(wait)));
    let (started, ready) = tokio::sync::oneshot::channel();
    let started = Arc::new(Mutex::new(Some(started)));
    let root = workspace.path().to_path_buf();
    let task = tokio::spawn(async move {
        run(&results, &root, &resources, move |index, permit| {
            assert!(index < 2, "no refill is permitted after failure");
            let wait = Arc::clone(&wait);
            let started = Arc::clone(&started);
            async move {
                let _permit = permit;
                if index == 0 {
                    anyhow::bail!("injected failure");
                }
                started.lock().unwrap().take().unwrap().send(()).unwrap();
                let gate = wait.lock().unwrap().take().unwrap();
                gate.await.unwrap();
                Ok(())
            }
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    assert!(!task.is_finished());
    release.send(()).unwrap();
    assert!(task
        .await
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("injected failure"));
}

#[tokio::test]
async fn cancelling_the_window_releases_all_admitted_future_reservations() {
    let workspace = tempfile::tempdir().unwrap();
    let results = vec![result("a"), result("b")];
    if !paths::disjoint(
        workspace.path(),
        &results[0].outputs.keys().cloned().collect(),
        &results[1].outputs.keys().cloned().collect(),
    ) {
        return;
    }
    let resources = Arc::new(ResourcePool::new(ResourceLimits::new(
        2,
        1024 * 1024 * 1024,
    )));
    let pool = Arc::clone(&resources);
    let (started, mut ready) = tokio::sync::mpsc::channel(2);
    let root = workspace.path().to_path_buf();
    let task = tokio::spawn(async move {
        run(&results, &root, &pool, move |index, permit| {
            let started = started.clone();
            async move {
                let _permit = permit;
                started.send(index).await.unwrap();
                std::future::pending::<()>().await;
                Ok(())
            }
        })
        .await
    });
    for _ in 0..2 {
        tokio::time::timeout(Duration::from_secs(5), ready.recv())
            .await
            .unwrap()
            .unwrap();
    }
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let permit = tokio::time::timeout(
        Duration::from_secs(5),
        resources.acquire(ResourceRequest::new(2, 1024 * 1024 * 1024)),
    )
    .await
    .unwrap();
    drop(permit);
}

#[cfg(unix)]
#[tokio::test]
async fn refills_recheck_aliases_created_while_other_members_are_active() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join("tree")).unwrap();
    let results = vec![result("tree"), result("other"), result("alias/child")];
    let resources = Arc::new(ResourcePool::new(ResourceLimits::new(
        2,
        1024 * 1024 * 1024,
    )));
    let (release, wait) = tokio::sync::oneshot::channel();
    let wait = Arc::new(Mutex::new(Some(wait)));
    let (started, ready) = tokio::sync::oneshot::channel();
    let started = Arc::new(Mutex::new(Some(started)));
    let (installed, installed_wait) = tokio::sync::oneshot::channel();
    let installed = Arc::new(Mutex::new(Some(installed)));
    let root = workspace.path().to_path_buf();
    let task = tokio::spawn(async move {
        run(&results, &root, &resources, |index, permit| {
            let wait = Arc::clone(&wait);
            let started = Arc::clone(&started);
            let installed = Arc::clone(&installed);
            let root = root.clone();
            async move {
                let _permit = permit;
                if index == 0 {
                    std::os::unix::fs::symlink("tree", root.join("alias")).unwrap();
                    installed.lock().unwrap().take().unwrap().send(()).unwrap();
                    let gate = wait.lock().unwrap().take().unwrap();
                    gate.await.unwrap();
                } else if index == 2 {
                    started.lock().unwrap().take().unwrap().send(()).unwrap();
                }
                Ok(())
            }
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), installed_wait)
        .await
        .unwrap()
        .unwrap();
    tokio::pin!(ready);
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut ready)
        .await
        .is_err());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    task.await.unwrap().unwrap();
}
