use super::*;
use std::path::PathBuf;

#[cfg(unix)]
#[tokio::test]
async fn rejects_an_input_beneath_an_ancestor_symlink() {
    let workspace = tempfile::tempdir().unwrap();
    let sibling = tempfile::tempdir().unwrap();
    std::fs::write(sibling.path().join("secret"), b"coordinator only").unwrap();
    std::os::unix::fs::symlink(sibling.path(), workspace.path().join("alias")).unwrap();
    assert!(create_input_archive_limited(
        workspace.path(),
        &[WorkspacePath::try_from("alias/secret").unwrap()],
        "test",
        RemoteIoLimits::default(),
        None,
    )
    .await
    .is_err());
}

#[tokio::test]
async fn input_staging_rejects_byte_and_metadata_growth() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join("inputs")).unwrap();
    std::fs::write(workspace.path().join("inputs/a"), vec![0; 2048]).unwrap();
    let inputs = [WorkspacePath::try_from("inputs").unwrap()];
    for limits in [
        RemoteIoLimits {
            max_archive_bytes: 512,
            ..RemoteIoLimits::default()
        },
        RemoteIoLimits {
            max_archive_entries: 1,
            ..RemoteIoLimits::default()
        },
        RemoteIoLimits {
            max_metadata_bytes: 10,
            ..RemoteIoLimits::default()
        },
    ] {
        let error = create_input_archive_limited(workspace.path(), &inputs, "test", limits, None)
            .await
            .err()
            .unwrap();
        assert!(matches!(error, crate::Error::ExecutorResourceLimit { .. }));
        assert!(std::fs::read_dir(workspace.path().join(".once/tmp"))
            .map_or(true, |entries| entries.count() == 0));
    }
}

#[tokio::test]
async fn input_archive_preserves_a_dependency_tree() {
    let workspace = tempfile::tempdir().unwrap();
    let bin = workspace.path().join("node_modules/.bin");
    let package = workspace.path().join("node_modules/vitest");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("runner.mjs"), "console.log('ok')").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("../vitest/runner.mjs", bin.join("vitest")).unwrap();
    let (archive, _phase) = create_input_archive_limited(
        workspace.path(),
        &[WorkspacePath::try_from("node_modules").unwrap()],
        "test",
        RemoteIoLimits::default(),
        None,
    )
    .await
    .unwrap();
    let destination = tempfile::tempdir().unwrap();
    tar::Archive::new(std::fs::File::open(archive.path()).unwrap())
        .unpack(destination.path())
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(destination.path().join("node_modules/vitest/runner.mjs")).unwrap(),
        "console.log('ok')"
    );
    #[cfg(unix)]
    assert_eq!(
        std::fs::read_link(destination.path().join("node_modules/.bin/vitest")).unwrap(),
        PathBuf::from("../vitest/runner.mjs")
    );
}

#[tokio::test]
async fn fifty_thousand_short_paths_fit_the_default_metadata_budget() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join("inputs")).unwrap();
    for index in 0..50_000 {
        std::fs::write(workspace.path().join(format!("inputs/{index:05}")), b"").unwrap();
    }
    let (archive, _phase) = create_input_archive_limited(
        workspace.path(),
        &[WorkspacePath::try_from("inputs").unwrap()],
        "test",
        RemoteIoLimits::default(),
        None,
    )
    .await
    .unwrap();
    assert!(archive.len() > 50_000 * 512);
}
