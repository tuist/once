use super::*;

async fn capture(workspace: &Workspace, producers: &Producers, path: &str) -> Digest {
    let lease = producers
        .executors(&workspace.resources)
        .unwrap()
        .acquire_materialization()
        .await;
    let output = struo_core::WorkspacePath::try_from(path).unwrap();
    super::super::super::super::capture_uncached_outputs_admitted(
        &[output],
        workspace.path(),
        &workspace.cache,
        Some(&lease),
        Some((producers, "produce")),
    )
    .await
    .unwrap()[path]
}

#[tokio::test]
async fn fingerprint_refusal_is_not_absence_and_never_repairs_or_installs_other_outputs() {
    let workspace = Workspace::new();
    let producers = Producers::default();
    std::fs::create_dir_all(workspace.path().join(PRODUCED)).unwrap();
    std::fs::write(workspace.path().join(PRODUCED).join("child"), b"contents").unwrap();
    let digest = capture(&workspace, &producers, PRODUCED).await;
    std::fs::remove_dir_all(workspace.path().join(PRODUCED)).unwrap();
    let raw = workspace.cache.put_blob(b"restorable").await.unwrap();
    let result = ActionResult {
        outputs: BTreeMap::from([(PRODUCED.to_string(), digest), ("a.txt".to_string(), raw)]),
        exit_code: 0,
        stdout: None,
        stderr: None,
    };
    let error = Repair {
        workspace: workspace.path(),
        cache: &workspace.cache,
        source_digest_cache: None,
        resources: &workspace.resources,
        producers: &producers,
    }
    .restore(&result)
    .await
    .unwrap_err();
    assert_eq!(confirmed_missing_blob(&error), None);
    assert!(format!("{error:#}").contains("uncached action `produce`"));
    assert!(lock(&producers.inner.repairs).is_empty());
    assert_eq!(
        producers.inner.remaining.load(Ordering::SeqCst),
        RepairLimits::default().executions
    );
    assert!(!workspace.path().join("a.txt").exists());
    assert!(!workspace.path().join(PRODUCED).exists());
}

#[tokio::test]
async fn present_fingerprints_verify_without_accessing_their_blobs() {
    let workspace = Workspace::new();
    let producers = Producers::default();
    std::fs::create_dir_all(workspace.path().join(PRODUCED)).unwrap();
    std::fs::write(workspace.path().join(PRODUCED).join("child"), b"contents").unwrap();
    let digest = capture(&workspace, &producers, PRODUCED).await;
    std::fs::remove_file(workspace.blob_path(digest)).unwrap();
    workspace.restore(&producers, digest).await.unwrap();
    assert_eq!(
        std::fs::read(workspace.path().join(PRODUCED).join("child")).unwrap(),
        b"contents"
    );
}

#[tokio::test]
async fn changed_fingerprint_and_symlink_to_an_identical_directory_are_refused() {
    let workspace = Workspace::new();
    let producers = Producers::default();
    std::fs::create_dir_all(workspace.path().join(PRODUCED)).unwrap();
    std::fs::write(workspace.path().join(PRODUCED).join("child"), b"original").unwrap();
    let digest = capture(&workspace, &producers, PRODUCED).await;
    std::fs::write(workspace.path().join(PRODUCED).join("child"), b"changed").unwrap();
    assert!(workspace
        .restore(&producers, digest)
        .await
        .unwrap_err()
        .to_string()
        .contains("verifying output"));
    std::fs::write(workspace.path().join(PRODUCED).join("child"), b"original").unwrap();
    std::fs::rename(
        workspace.path().join(PRODUCED),
        workspace.path().join("backing"),
    )
    .unwrap();
    std::os::unix::fs::symlink("../../backing", workspace.path().join(PRODUCED)).unwrap();
    let error = workspace.restore(&producers, digest).await.unwrap_err();
    assert!(format!("{error:#}").contains("fingerprint-only output"));
    assert!(workspace
        .path()
        .join(PRODUCED)
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
}

#[tokio::test]
async fn raw_manifest_bytes_at_unregistered_keys_restore_normally() {
    for bytes in [
        b"struo.directory_output.v1\n".as_slice(),
        b"struo.symlink_output.v1\nmissing\n".as_slice(),
    ] {
        let workspace = Workspace::new();
        let producers = Producers::default();
        std::fs::create_dir_all(workspace.path().join(".struo/out")).unwrap();
        std::fs::write(workspace.path().join(PRODUCED), bytes).unwrap();
        let digest = capture(&workspace, &producers, PRODUCED).await;
        std::fs::remove_file(workspace.path().join(PRODUCED)).unwrap();
        workspace.restore(&producers, digest).await.unwrap();
        assert_eq!(
            std::fs::read(workspace.path().join(PRODUCED)).unwrap(),
            bytes
        );
    }
}

#[tokio::test]
async fn later_raw_capture_never_clears_an_earlier_fingerprint_for_the_same_key() {
    let workspace = Workspace::new();
    let producers = Producers::default();
    std::fs::create_dir_all(workspace.path().join(PRODUCED)).unwrap();
    let digest = capture(&workspace, &producers, PRODUCED).await;
    std::fs::remove_dir(workspace.path().join(PRODUCED)).unwrap();
    std::fs::write(
        workspace.path().join(PRODUCED),
        b"struo.directory_output.v1\n",
    )
    .unwrap();
    let raw = capture(&workspace, &producers, PRODUCED).await;
    assert_eq!(digest, raw);
    let wrong_type = workspace.restore(&producers, digest).await.unwrap_err();
    assert!(format!("{wrong_type:#}").contains("fingerprint-only output"));
    std::fs::remove_file(workspace.path().join(PRODUCED)).unwrap();
    let absent = workspace.restore(&producers, raw).await.unwrap_err();
    assert!(format!("{absent:#}").contains("fingerprint-only output"));
    assert!(!workspace.path().join(PRODUCED).exists());
}

#[tokio::test]
async fn restorable_exact_child_does_not_require_its_fingerprint_only_parent() {
    let workspace = Workspace::new();
    let producers = Producers::default();
    std::fs::create_dir_all(workspace.path().join(PRODUCED)).unwrap();
    let parent = capture(&workspace, &producers, PRODUCED).await;
    std::fs::remove_dir(workspace.path().join(PRODUCED)).unwrap();
    let child = format!("{PRODUCED}/child");
    let digest = workspace.cache.put_blob(b"child").await.unwrap();
    let inputs = BTreeMap::from([
        (
            PRODUCED.to_string(),
            super::super::super::super::AvailableInput {
                blob_digest: parent,
                materialized: false,
                same_target: false,
            },
        ),
        (
            child.clone(),
            super::super::super::super::AvailableInput {
                blob_digest: digest,
                materialized: false,
                same_target: false,
            },
        ),
    ]);
    let declared =
        action(serde_json::json!({"argv": ["unused"], "inputs": [&child], "outputs": []}));
    let result = super::super::super::super::unmaterialized_inputs_for(&declared, &inputs).unwrap();
    assert_eq!(result.outputs.len(), 1);
    Repair {
        workspace: workspace.path(),
        cache: &workspace.cache,
        source_digest_cache: None,
        resources: &workspace.resources,
        producers: &producers,
    }
    .restore(&result)
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(workspace.path().join(child)).unwrap(),
        b"child"
    );
}
