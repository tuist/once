use super::*;
use std::os::unix::fs::PermissionsExt;

fn filesystem_aliases(workspace: &Path, actual: &str, alias: &str) -> bool {
    let probe = tempfile::tempdir_in(workspace).unwrap();
    std::os::unix::fs::symlink(".", probe.path().join(actual)).unwrap();
    std::fs::symlink_metadata(probe.path().join(alias)).is_ok()
}

#[tokio::test]
async fn filesystem_aliases_cannot_hide_chained_output_escapes() {
    for (actual, alias) in [
        ("A", "A"),
        ("A", "a"),
        ("é", "e\u{301}"),
        ("Å", "å"),
        ("A", "different"),
    ] {
        let workspace = tempfile::tempdir().unwrap();
        let aliases = filesystem_aliases(workspace.path(), actual, alias);
        std::fs::write(workspace.path().join("secret"), b"coordinator only").unwrap();
        std::fs::create_dir(workspace.path().join("out")).unwrap();
        std::fs::write(workspace.path().join("out/original"), b"original").unwrap();
        let path = workspace.path().join("outputs.tar");
        archive(
            &path,
            &[
                (&format!("out/{actual}"), "."),
                ("out/b", &format!("{alias}/..")),
                ("out/leak", "b/secret"),
            ],
            &[],
        );
        let result = install_output_archive(
            &path,
            workspace.path(),
            &[WorkspacePath::try_from("out").unwrap()],
            "test",
        )
        .await;
        if aliases {
            assert!(
                result.is_err(),
                "filesystem alias {actual:?}/{alias:?} bypassed confinement"
            );
            assert_eq!(
                std::fs::read(workspace.path().join("out/original")).unwrap(),
                b"original"
            );
            assert!(std::fs::symlink_metadata(workspace.path().join("out/leak")).is_err());
        } else {
            result.unwrap();
            assert!(std::fs::symlink_metadata(workspace.path().join("out/leak"))
                .unwrap()
                .file_type()
                .is_symlink());
            assert!(
                std::fs::read(workspace.path().join("out/leak")).is_err(),
                "distinct names must leave a dangling link"
            );
        }
        assert_eq!(
            std::fs::read(workspace.path().join("secret")).unwrap(),
            b"coordinator only"
        );
    }
}

#[tokio::test]
async fn filesystem_aliases_preserve_safe_links_and_detect_cycles() {
    for (actual, alias) in [
        ("A", "A"),
        ("A", "a"),
        ("é", "e\u{301}"),
        ("Å", "å"),
        ("A", "different"),
    ] {
        let workspace = tempfile::tempdir().unwrap();
        let aliases = filesystem_aliases(workspace.path(), actual, alias);
        let path = workspace.path().join("outputs.tar");
        archive(
            &path,
            &[
                (&format!("out/{actual}"), "real"),
                ("out/tool", &format!("{alias}/tool")),
            ],
            &[("out/real/tool", b"tool")],
        );
        install_output_archive(
            &path,
            workspace.path(),
            &[WorkspacePath::try_from("out").unwrap()],
            "test",
        )
        .await
        .unwrap();
        if aliases {
            assert_eq!(
                std::fs::read(workspace.path().join("out/tool")).unwrap(),
                b"tool"
            );
        } else {
            assert!(std::fs::read(workspace.path().join("out/tool")).is_err());
        }
        archive(
            &path,
            &[(&format!("out/{actual}"), "b"), ("out/b", alias)],
            &[],
        );
        let result = install_output_archive(
            &path,
            workspace.path(),
            &[WorkspacePath::try_from("out").unwrap()],
            "test",
        )
        .await;
        if aliases {
            assert!(
                result.is_err(),
                "filesystem alias {actual:?}/{alias:?} hid a cycle"
            );
            assert_eq!(
                std::fs::read(workspace.path().join("out/real/tool")).unwrap(),
                b"tool"
            );
        } else {
            result.unwrap();
            assert!(std::fs::read(workspace.path().join("out/b")).is_err());
        }
    }
}

#[tokio::test]
async fn bounds_staged_symlink_expansion() {
    for count in [40, 41] {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join("outputs.tar");
        let links = (0..count)
            .map(|index| {
                (
                    format!("out/link{index}"),
                    if index + 1 == count {
                        "file".to_owned()
                    } else {
                        format!("link{}", index + 1)
                    },
                )
            })
            .collect::<Vec<_>>();
        let references = links
            .iter()
            .map(|(path, target)| (path.as_str(), target.as_str()))
            .collect::<Vec<_>>();
        archive(&path, &references, &[("out/file", b"tool")]);
        let result = install_output_archive(
            &path,
            workspace.path(),
            &[WorkspacePath::try_from("out").unwrap()],
            "test",
        )
        .await;
        if count == 40 {
            result.unwrap();
            assert!(
                std::fs::symlink_metadata(workspace.path().join("out/link0"))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        } else {
            assert!(result.is_err());
            assert!(!workspace.path().join("out").exists());
        }
    }
}

#[tokio::test]
async fn preserves_restrictive_directory_modes_after_extracting_executable_children() {
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path().join("outputs.tar");
    let mut builder = tar::Builder::new(std::fs::File::create(&path).unwrap());
    for (path, mode) in [("out", 0o700), ("out/private", 0o500)] {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(mode);
        header.set_cksum();
        builder
            .append_data(&mut header, path, std::io::empty())
            .unwrap();
    }
    let script = b"#!/bin/sh\nexit 0\n";
    let mut header = tar::Header::new_gnu();
    header.set_size(script.len() as u64);
    header.set_mode(0o751);
    header.set_cksum();
    builder
        .append_data(&mut header, "out/private/tool", &script[..])
        .unwrap();
    builder.finish().unwrap();
    install_output_archive(
        &path,
        workspace.path(),
        &[WorkspacePath::try_from("out").unwrap()],
        "test",
    )
    .await
    .unwrap();
    for (path, mode) in [
        ("out", 0o700),
        ("out/private", 0o500),
        ("out/private/tool", 0o751),
    ] {
        assert_eq!(
            std::fs::metadata(workspace.path().join(path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            mode
        );
    }
    assert!(
        std::process::Command::new(workspace.path().join("out/private/tool"))
            .status()
            .unwrap()
            .success()
    );
    std::fs::set_permissions(
        workspace.path().join("out/private"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
}

fn archive(path: &Path, links: &[(&str, &str)], files: &[(&str, &[u8])]) {
    let mut builder = tar::Builder::new(std::fs::File::create(path).unwrap());
    for (path, target) in links {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        builder.append_link(&mut header, path, target).unwrap();
    }
    for (path, contents) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append_data(&mut header, path, *contents).unwrap();
    }
    builder.finish().unwrap();
}

#[tokio::test]
async fn rejects_chained_parent_traversal_before_publication() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("secret"), b"coordinator only").unwrap();
    std::fs::create_dir(workspace.path().join("out")).unwrap();
    std::fs::write(workspace.path().join("out/original"), b"original").unwrap();
    let path = workspace.path().join("outputs.tar");
    archive(
        &path,
        &[("out/a", "."), ("out/b", "a/.."), ("out/leak", "b/secret")],
        &[],
    );
    assert!(install_output_archive(
        &path,
        workspace.path(),
        &[WorkspacePath::try_from("out").unwrap()],
        "test"
    )
    .await
    .is_err());
    assert_eq!(
        std::fs::read(workspace.path().join("out/original")).unwrap(),
        b"original"
    );
    assert!(!workspace.path().join("out/leak").exists());
}

#[tokio::test]
async fn preserves_framework_npm_and_dangling_links() {
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path().join("outputs.tar");
    archive(
        &path,
        &[
            ("out/Kit.framework/Versions/Current", "A"),
            ("out/Kit.framework/Kit", "Versions/Current/Kit"),
            ("out/node_modules/.bin/tool", "../tool/bin"),
            ("out/dangling", "not-built-yet"),
            ("out/shared", "../shared/file"),
        ],
        &[
            ("out/Kit.framework/Versions/A/Kit", b"framework"),
            ("out/node_modules/tool/bin", b"tool"),
            ("shared/file", b"shared"),
        ],
    );
    install_output_archive(
        &path,
        workspace.path(),
        &[
            WorkspacePath::try_from("out").unwrap(),
            WorkspacePath::try_from("shared").unwrap(),
        ],
        "test",
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(workspace.path().join("out/Kit.framework/Kit")).unwrap(),
        b"framework"
    );
    assert_eq!(
        std::fs::read(workspace.path().join("out/node_modules/.bin/tool")).unwrap(),
        b"tool"
    );
    assert_eq!(
        std::fs::read_link(workspace.path().join("out/dangling")).unwrap(),
        PathBuf::from("not-built-yet")
    );
    assert_eq!(
        std::fs::read(workspace.path().join("out/shared")).unwrap(),
        b"shared"
    );
    assert_ne!(
        std::fs::metadata(workspace.path().join("out/node_modules/tool/bin"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0
    );
}

#[tokio::test]
async fn rejects_cycles_and_undeclared_dangling_targets() {
    for links in [
        vec![("out/a", "b"), ("out/b", "a")],
        vec![("out/a", "a")],
        vec![("out/a", "../missing")],
        vec![("out/a", "/host/secret")],
        vec![("out/a", "../host-alias/../out/missing")],
    ] {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join("outputs.tar");
        archive(&path, &links, &[]);
        assert!(install_output_archive(
            &path,
            workspace.path(),
            &[WorkspacePath::try_from("out").unwrap()],
            "test"
        )
        .await
        .is_err());
        assert!(!workspace.path().join("out").exists());
    }
}

#[tokio::test]
async fn rejects_archive_entries_beneath_a_staged_symlink() {
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path().join("outputs.tar");
    archive(
        &path,
        &[("out/alias", ".")],
        &[("out/alias/file", b"payload")],
    );
    assert!(install_output_archive(
        &path,
        workspace.path(),
        &[WorkspacePath::try_from("out").unwrap()],
        "test"
    )
    .await
    .is_err());
    assert!(!workspace.path().join("out").exists());
}

#[tokio::test]
async fn archive_install_cannot_overwrite_a_host_symlink_ancestor() {
    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("workspace");
    let sibling = parent.path().join("sibling");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&sibling).unwrap();
    std::fs::write(sibling.join("secret"), b"coordinator only").unwrap();
    std::os::unix::fs::symlink("../sibling", workspace.join("alias")).unwrap();
    let path = parent.path().join("outputs.tar");
    archive(&path, &[], &[("alias/secret", b"overwrite")]);
    assert!(install_output_archive(
        &path,
        &workspace,
        &[WorkspacePath::try_from("alias/secret").unwrap()],
        "test"
    )
    .await
    .is_err());
    assert_eq!(
        std::fs::read(sibling.join("secret")).unwrap(),
        b"coordinator only"
    );
}

#[tokio::test]
async fn archive_install_replaces_a_leaf_symlink_without_following_it() {
    let workspace = tempfile::tempdir().unwrap();
    let sibling = tempfile::tempdir().unwrap();
    std::fs::write(sibling.path().join("secret"), b"coordinator only").unwrap();
    std::os::unix::fs::symlink(sibling.path().join("secret"), workspace.path().join("out"))
        .unwrap();
    let path = workspace.path().join("outputs.tar");
    archive(&path, &[], &[("out", b"replacement")]);
    install_output_archive(
        &path,
        workspace.path(),
        &[WorkspacePath::try_from("out").unwrap()],
        "test",
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(workspace.path().join("out")).unwrap(),
        b"replacement"
    );
    assert_eq!(
        std::fs::read(sibling.path().join("secret")).unwrap(),
        b"coordinator only"
    );
}
