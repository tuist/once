use super::*;
use crate::{Executor, ExecutorResources, ResourceLimits, ResourcePool, ResourceRequest};
use std::sync::Arc;
use std::time::Duration;

#[test]
fn many_replaced_outputs_fit_a_small_descriptor_budget() {
    const CHILD: &str = "STRUO_OUTPUT_DESCRIPTOR_TEST";
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
    std::fs::create_dir_all(workspace.path().join(".struo/locks")).unwrap();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(workspace.path().join(".struo/locks/output-restore.lock"))
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
                workspace.path().join(".struo"),
                workspace.path().join("saved-struo"),
            )
            .unwrap();
            std::os::unix::fs::symlink(sibling.path(), workspace.path().join(".struo")).unwrap();
        } else {
            std::fs::create_dir(workspace.path().join(".struo/locks")).unwrap();
            std::os::unix::fs::symlink(
                sibling.path().join("secret"),
                workspace.path().join(".struo/locks/output-restore.lock"),
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

#[tokio::test]
async fn remote_snapshots_preserve_links_and_recheck_coordinator_targets_on_restore() {
    use struo_cas::{ActionResult, CacheProvider};
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().unwrap();
    let cas = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(cas.path().to_path_buf());
    let roots = ["out", "other"].map(|path| WorkspacePath::try_from(path).unwrap());
    let mut staging = OutputStaging::create(workspace.path()).unwrap();
    std::fs::create_dir(staging.files().join("out")).unwrap();
    std::fs::create_dir(staging.files().join("other")).unwrap();
    std::fs::write(staging.files().join("other/file"), b"worker bytes").unwrap();
    symlink("../assets/missing", staging.files().join("out/dangling")).unwrap();
    symlink("../other/file", staging.files().join("out/cross-output")).unwrap();
    let mut outputs = staging.capture(&roots, &roots, &cache).await.unwrap();
    staging.install(&roots, workspace.path()).await.unwrap();
    std::fs::create_dir(workspace.path().join("local")).unwrap();
    std::fs::write(workspace.path().join("local/file"), b"local copy bytes").unwrap();
    std::fs::create_dir(workspace.path().join("local/child")).unwrap();
    std::fs::write(
        workspace.path().join("local/child/stale"),
        b"old child bytes",
    )
    .unwrap();
    outputs.extend(
        crate::outputs::capture(
            &[WorkspacePath::try_from("local").unwrap()],
            workspace.path(),
            &cache,
            crate::OutputSymlinkMode::Preserve,
        )
        .await
        .unwrap(),
    );
    std::fs::write(workspace.path().join("local/file"), b"updated copy bytes").unwrap();
    outputs.extend(
        crate::outputs::capture(
            &[WorkspacePath::try_from("local/file").unwrap()],
            workspace.path(),
            &cache,
            crate::OutputSymlinkMode::Preserve,
        )
        .await
        .unwrap(),
    );
    std::fs::remove_file(workspace.path().join("local/child/stale")).unwrap();
    std::fs::write(
        workspace.path().join("local/child/current"),
        b"new child bytes",
    )
    .unwrap();
    outputs.extend(
        crate::outputs::capture(
            &[WorkspacePath::try_from("local/child").unwrap()],
            workspace.path(),
            &cache,
            crate::OutputSymlinkMode::Preserve,
        )
        .await
        .unwrap(),
    );
    let result = ActionResult {
        exit_code: 0,
        stdout: None,
        stderr: None,
        outputs,
    };
    for _ in 0..2 {
        assert_eq!(
            std::fs::read_link(workspace.path().join("out/dangling")).unwrap(),
            Path::new("../assets/missing")
        );
        assert_eq!(
            std::fs::read(workspace.path().join("out/cross-output")).unwrap(),
            b"worker bytes"
        );
        assert_eq!(
            std::fs::read(workspace.path().join("local/file")).unwrap(),
            b"updated copy bytes"
        );
        assert_eq!(
            std::fs::read(workspace.path().join("local/child/current")).unwrap(),
            b"new child bytes"
        );
        assert!(!workspace.path().join("local/child/stale").exists());
        std::fs::remove_dir_all(workspace.path().join("local")).unwrap();
        std::fs::remove_dir_all(workspace.path().join("out")).unwrap();
        std::fs::remove_dir_all(workspace.path().join("other")).unwrap();
        crate::materialize_outputs(&result, workspace.path(), &cache)
            .await
            .unwrap();
    }
    std::fs::create_dir(workspace.path().join("assets")).unwrap();
    std::fs::write(workspace.path().join("assets/missing"), b"coordinator only").unwrap();
    assert!(crate::outputs::restore(&result, workspace.path(), &cache)
        .await
        .is_err());
    assert_eq!(
        std::fs::read(workspace.path().join("assets/missing")).unwrap(),
        b"coordinator only"
    );
}

#[tokio::test]
async fn unsafe_remote_links_are_rejected_before_capture_and_on_cached_restore() {
    use struo_cas::{ActionResult, CacheProvider};
    use std::os::unix::fs::symlink;

    for target in [
        "/etc/passwd",
        "../../escape",
        "cycle",
        "../assets/file",
        "../alias/absent",
        "../assets",
        "../alias/../missing",
    ] {
        let workspace = tempfile::tempdir().unwrap();
        let cas = tempfile::tempdir().unwrap();
        let cache = CacheProvider::open_local(cas.path().to_path_buf());
        std::fs::create_dir(workspace.path().join("assets")).unwrap();
        std::fs::write(workspace.path().join("assets/file"), b"private").unwrap();
        symlink("assets", workspace.path().join("alias")).unwrap();
        std::fs::write(workspace.path().join("out"), b"original").unwrap();
        let mut staging = OutputStaging::create(workspace.path()).unwrap();
        std::fs::create_dir(staging.files().join("out")).unwrap();
        symlink(target, staging.files().join("out/cycle")).unwrap();
        let roots = [WorkspacePath::try_from("out").unwrap()];
        assert!(
            staging.capture(&roots, &roots, &cache).await.is_err(),
            "{target}"
        );

        // Model a cache blob supplied without the producer's validation.
        let outputs = crate::outputs::capture_remote(&roots, staging.files(), &cache)
            .await
            .unwrap();
        let result = ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs,
        };
        assert!(
            crate::outputs::restore(&result, workspace.path(), &cache)
                .await
                .is_err(),
            "{target}"
        );
        assert_eq!(
            std::fs::read(workspace.path().join("out")).unwrap(),
            b"original"
        );
        assert_eq!(
            std::fs::read(workspace.path().join("assets/file")).unwrap(),
            b"private"
        );
    }
}

#[tokio::test]
async fn remote_snapshot_decoder_rejects_writes_through_links_before_publication() {
    use struo_cas::{ActionResult, CacheProvider};
    let workspace = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let cas = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(cas.path().to_path_buf());
    std::fs::write(workspace.path().join("out"), b"original").unwrap();
    std::fs::write(external.path().join("secret"), b"private").unwrap();
    let mut blob = crate::outputs::REMOTE_OUTPUT_MAGIC.to_vec();
    blob.extend_from_slice(crate::directory_blob::DIRECTORY_BLOB_MAGIC);
    let mut parent_blob = Vec::new();
    for (kind, path, content) in [
        (3_u8, "link", external.path().to_str().unwrap().as_bytes()),
        (1, "link/secret", b"overwrite".as_slice()),
    ] {
        blob.push(kind);
        blob.extend_from_slice(&(path.len() as u32).to_le_bytes());
        blob.extend_from_slice(&0o755_u32.to_le_bytes());
        blob.extend_from_slice(&(content.len() as u64).to_le_bytes());
        blob.extend_from_slice(path.as_bytes());
        blob.extend_from_slice(content);
        if kind == 3 {
            parent_blob = blob.clone();
        }
    }
    let mut marker = crate::outputs::REMOTE_OUTPUT_MAGIC.to_vec();
    marker.extend_from_slice(b"safe");
    let marker = cache.put_blob(&marker).await.unwrap();
    for (snapshot, separate_child) in [(&blob, false), (&parent_blob, true)] {
        for enveloped in [true, false] {
            let bytes = if enveloped {
                &snapshot[..]
            } else {
                &snapshot[crate::outputs::REMOTE_OUTPUT_MAGIC.len()..]
            };
            let digest = cache.put_blob(bytes).await.unwrap();
            let mut outputs = BTreeMap::from([("out".into(), digest), ("marker".into(), marker)]);
            if separate_child {
                outputs.insert("out/link/secret".into(), marker);
            }
            let result = ActionResult {
                exit_code: 0,
                stdout: None,
                stderr: None,
                outputs,
            };
            assert!(
                crate::materialize_outputs(&result, workspace.path(), &cache)
                    .await
                    .is_err()
            );
        }
    }
    assert_eq!(
        std::fs::read(external.path().join("secret")).unwrap(),
        b"private"
    );
    assert_eq!(
        std::fs::read(workspace.path().join("out")).unwrap(),
        b"original"
    );
}
