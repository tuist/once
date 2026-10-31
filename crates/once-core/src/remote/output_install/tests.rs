use super::*;
use crate::{Executor, ExecutorResources, ResourceLimits, ResourcePool, ResourceRequest};
use std::sync::Arc;
use std::time::Duration;

#[test]
fn many_replaced_outputs_fit_a_small_descriptor_budget() {
    const CHILD: &str = "ONCE_OUTPUT_DESCRIPTOR_TEST";
    if std::env::var_os(CHILD).is_none() {
        let result = std::process::Command::new("/bin/sh")
            .args(["-c", "ulimit -n 256 && exec \"$@\"", "output-descriptors"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "remote::output_install::tests::many_replaced_outputs_fit_a_small_descriptor_budget",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            for separate_parents in [false, true, false] {
                let workspace = tempfile::tempdir().unwrap();
                let staging = OutputStaging::create(workspace.path()).unwrap();
                let outputs = (0..100)
                    .map(|index| {
                        let path = if separate_parents {
                            format!("dir{index}/out")
                        } else {
                            format!("shared/out{index}")
                        };
                        for root in [workspace.path(), staging.files()] {
                            std::fs::create_dir_all(root.join(&path).parent().unwrap()).unwrap();
                        }
                        std::fs::write(workspace.path().join(&path), b"old").unwrap();
                        std::fs::write(staging.files().join(&path), b"new").unwrap();
                        WorkspacePath::try_from(path).unwrap()
                    })
                    .collect::<Vec<_>>();
                let result = staging.install(&outputs, workspace.path()).await;
                if separate_parents {
                    assert!(matches!(
                        result,
                        Err(Error::ExecutorResourceLimit {
                            resource: "pinned output directories",
                            ..
                        })
                    ));
                } else {
                    result.unwrap();
                }
                for output in outputs {
                    assert_eq!(
                        std::fs::read(workspace.path().join(output.as_str())).unwrap(),
                        if separate_parents { b"old" } else { b"new" }
                    );
                }
            }
        });
}

#[tokio::test]
async fn installer_uses_the_existing_output_restore_lock() {
    let workspace = tempfile::tempdir().unwrap();
    let staging = OutputStaging::create(workspace.path()).unwrap();
    std::fs::write(staging.files().join("out"), b"new").unwrap();
    std::fs::create_dir_all(workspace.path().join(".once/locks")).unwrap();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(workspace.path().join(".once/locks/output-restore.lock"))
        .unwrap();
    let mut lock = fd_lock::RwLock::new(file);
    let guard = lock.write().unwrap();
    let root = workspace.path().to_path_buf();
    let install = tokio::spawn(async move {
        staging
            .install(&[WorkspacePath::try_from("out").unwrap()], &root)
            .await
    });
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(!workspace.path().join("out").exists());
    drop(guard);
    install.await.unwrap().unwrap();
    assert_eq!(std::fs::read(workspace.path().join("out")).unwrap(), b"new");
}

#[tokio::test]
async fn lock_acquisition_rejects_symlink_ancestors_and_leaves() {
    for swap_parent in [true, false] {
        let workspace = tempfile::tempdir().unwrap();
        let sibling = tempfile::tempdir().unwrap();
        std::fs::write(sibling.path().join("secret"), b"coordinator only").unwrap();
        let staging = OutputStaging::create(workspace.path()).unwrap();
        std::fs::write(staging.files().join("out"), b"new").unwrap();
        if swap_parent {
            std::fs::rename(
                workspace.path().join(".once"),
                workspace.path().join("saved-once"),
            )
            .unwrap();
            std::os::unix::fs::symlink(sibling.path(), workspace.path().join(".once")).unwrap();
        } else {
            std::fs::create_dir(workspace.path().join(".once/locks")).unwrap();
            std::os::unix::fs::symlink(
                sibling.path().join("secret"),
                workspace.path().join(".once/locks/output-restore.lock"),
            )
            .unwrap();
        }
        assert!(staging
            .install(&[WorkspacePath::try_from("out").unwrap()], workspace.path())
            .await
            .is_err());
        assert!(!workspace.path().join("out").exists());
        assert!(!sibling.path().join("locks").exists());
        assert_eq!(
            std::fs::read(sibling.path().join("secret")).unwrap(),
            b"coordinator only"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_later_unsafe_parent_rolls_back_an_already_published_tree() {
    let workspace = tempfile::tempdir().unwrap();
    let sibling = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join("first")).unwrap();
    std::fs::write(workspace.path().join("first/original"), b"original").unwrap();
    std::fs::write(sibling.path().join("secret"), b"coordinator only").unwrap();
    std::os::unix::fs::symlink(sibling.path(), workspace.path().join("alias")).unwrap();
    let staging = OutputStaging::create(workspace.path()).unwrap();
    std::fs::create_dir(staging.files().join("first")).unwrap();
    std::fs::write(staging.files().join("first/new"), b"new").unwrap();
    std::fs::create_dir(staging.files().join("alias")).unwrap();
    std::fs::write(staging.files().join("alias/secret"), b"overwrite").unwrap();
    assert!(staging
        .install(
            &[
                WorkspacePath::try_from("first").unwrap(),
                WorkspacePath::try_from("alias/secret").unwrap(),
            ],
            workspace.path()
        )
        .await
        .is_err());
    assert_eq!(
        std::fs::read(workspace.path().join("first/original")).unwrap(),
        b"original"
    );
    assert!(!workspace.path().join("first/new").exists());
    assert_eq!(
        std::fs::read(sibling.path().join("secret")).unwrap(),
        b"coordinator only"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_publication_beneath_a_host_symlink() {
    let workspace = tempfile::tempdir().unwrap();
    let sibling = tempfile::tempdir().unwrap();
    std::fs::write(sibling.path().join("secret"), b"coordinator only").unwrap();
    std::os::unix::fs::symlink(sibling.path(), workspace.path().join("alias")).unwrap();
    let staging = OutputStaging::create(workspace.path()).unwrap();
    std::fs::create_dir(staging.files().join("alias")).unwrap();
    std::fs::write(staging.files().join("alias/secret"), b"replacement").unwrap();
    assert!(staging
        .install(
            &[WorkspacePath::try_from("alias/secret").unwrap()],
            workspace.path(),
        )
        .await
        .is_err());
    assert_eq!(
        std::fs::read(sibling.path().join("secret")).unwrap(),
        b"coordinator only"
    );
}

#[test]
fn cancelled_installer_keeps_staging_and_admission_until_blocking_work_finishes() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let workspace = tempfile::tempdir().unwrap();
        let staging = OutputStaging::create(workspace.path()).unwrap();
        std::fs::write(staging.files().join("out"), b"complete").unwrap();
        let staging_path = staging.root.clone();
        let resources =
            ExecutorResources::new(Arc::new(ResourcePool::new(ResourceLimits::new(1, 0))));
        let lease = resources
            .acquire(Executor::Local, &ResourceRequest::default())
            .await;
        let phase = PhasePermit {
            _action: Some(lease),
            ..PhasePermit::default()
        };
        let (release, gate) = std::sync::mpsc::channel();
        let (arrived, arrival) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            arrived.send(()).unwrap();
            gate.recv().unwrap();
        });
        arrival.await.unwrap();
        let root = workspace.path().to_path_buf();
        let install = tokio::spawn(async move {
            staging
                .install_admitted(&[WorkspacePath::try_from("out").unwrap()], &root, phase)
                .await
        });
        tokio::task::yield_now().await;
        install.abort();
        assert!(install.await.unwrap_err().is_cancelled());
        assert!(
            staging_path.exists(),
            "cancelled observer must not remove staging from the queued installer"
        );
        assert!(tokio::time::timeout(
            Duration::from_millis(20),
            resources.acquire(Executor::Local, &ResourceRequest::default())
        )
        .await
        .is_err());
        release.send(()).unwrap();
        blocker.await.unwrap();
        let _next = tokio::time::timeout(
            Duration::from_secs(5),
            resources.acquire(Executor::Local, &ResourceRequest::default()),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read(workspace.path().join("out")).unwrap(),
            b"complete"
        );
        assert!(!staging_path.exists());
    });
}

#[tokio::test]
async fn failed_multi_output_install_rolls_back_every_published_output() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("first"), b"old first").unwrap();
    std::fs::write(workspace.path().join("second"), b"old second").unwrap();
    let staging = OutputStaging::create(workspace.path()).unwrap();
    std::fs::write(staging.files().join("first"), b"new first").unwrap();
    let outputs = [
        WorkspacePath::try_from("first").unwrap(),
        WorkspacePath::try_from("second").unwrap(),
    ];
    assert!(staging.install(&outputs, workspace.path()).await.is_err());
    assert_eq!(
        std::fs::read(workspace.path().join("first")).unwrap(),
        b"old first"
    );
    assert_eq!(
        std::fs::read(workspace.path().join("second")).unwrap(),
        b"old second"
    );
}
