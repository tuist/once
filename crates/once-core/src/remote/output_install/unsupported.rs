use crate::remote::archive::{create_input_archive_limited, install_output_archive_limited};
use crate::remote::PhasePermit;
use crate::{Error, RemoteIoLimits, WorkspacePath};

fn assert_unsupported(error: Error) {
    let Error::FileAction { source, .. } = error else {
        panic!("expected an observable unsupported filesystem error, got {error}");
    };
    assert_eq!(source.kind(), std::io::ErrorKind::Unsupported);
    assert!(source
        .to_string()
        .contains("race-safe remote filesystem access is unavailable"));
}

#[tokio::test]
async fn archive_input_preparation_fails_closed_without_platform_support() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("input"), b"declared").unwrap();
    let error = create_input_archive_limited(
        workspace.path(),
        &[WorkspacePath::try_from("input").unwrap()],
        "test",
        RemoteIoLimits::default(),
        None,
    )
    .await
    .err()
    .unwrap();
    assert_unsupported(error);
}

#[tokio::test]
async fn archive_output_installation_fails_closed_without_platform_support() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("out"), b"original").unwrap();
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut archive = tar::Builder::new(file.reopen().unwrap());
    let mut header = tar::Header::new_gnu();
    header.set_size(3);
    header.set_mode(0o755);
    header.set_cksum();
    archive
        .append_data(&mut header, "out", &b"new"[..])
        .unwrap();
    archive.finish().unwrap();
    let error = install_output_archive_limited(
        file.path(),
        workspace.path(),
        &[WorkspacePath::try_from("out").unwrap()],
        "test",
        RemoteIoLimits::default(),
        PhasePermit::default(),
    )
    .await
    .unwrap_err();
    assert_unsupported(error);
    assert_eq!(
        std::fs::read(workspace.path().join("out")).unwrap(),
        b"original"
    );
    assert!(!workspace.path().join(".once").exists());
}
