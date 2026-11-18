use super::*;

#[test]
fn rejects_deep_implicit_tree_before_creating_any_nodes() {
    let temp = tempfile::tempdir().unwrap();
    let archive = temp.path().join("outputs.tar");
    let staging = temp.path().join("staging");
    std::fs::create_dir(&staging).unwrap();
    let path = format!("out/{}file", "d/".repeat(600));
    super::tests::write_archive(&archive, &[(&path, b"x")]);
    let limits = RemoteIoLimits {
        max_archive_entries: 2,
        ..RemoteIoLimits::default()
    };
    validate_headers(&archive, limits, "test").unwrap();
    let error = extract(&archive, &staging, limits).unwrap_err();
    assert!(matches!(
        error,
        Error::ExecutorResourceLimit {
            resource: "archive entries",
            ..
        }
    ));
    assert_eq!(std::fs::read_dir(staging).unwrap().count(), 0);
}

fn extract(
    archive: &Path,
    staging: &Path,
    limits: RemoteIoLimits,
) -> Result<BTreeMap<PathBuf, u32>> {
    extract_output_archive(
        archive,
        &Directory::open(staging).unwrap(),
        &[WorkspacePath::try_from("out").unwrap()],
        "test",
        limits,
    )
}

#[test]
fn bounds_depth_even_with_large_entry_and_metadata_limits() {
    for depth in [64, 65] {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("outputs.tar");
        let staging = temp.path().join("staging");
        std::fs::create_dir(&staging).unwrap();
        let path = format!("out/{}file", "d/".repeat(depth - 2));
        super::tests::write_archive(&archive, &[(&path, b"x")]);
        let result = extract(&archive, &staging, RemoteIoLimits::default());
        if depth == 64 {
            result.unwrap();
            assert_eq!(std::fs::read(staging.join(path)).unwrap(), b"x");
        } else {
            assert!(matches!(
                result,
                Err(Error::ExecutorResourceLimit {
                    resource: "output path depth",
                    ..
                })
            ));
            assert_eq!(std::fs::read_dir(staging).unwrap().count(), 0);
        }
    }
}

#[test]
fn charges_cumulative_implicit_nodes_before_creating_them() {
    let metadata_cost = ["out", "out/a", "out/b", "out/b/c"]
        .iter()
        .map(|path| path.len() as u64 + 256)
        .sum::<u64>();
    for (entry_limit, metadata_limit, succeeds) in [
        (3, metadata_cost, false),
        (4, metadata_cost - 1, false),
        (4, metadata_cost, true),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("outputs.tar");
        let staging = temp.path().join("staging");
        std::fs::create_dir(&staging).unwrap();
        super::tests::write_archive(&archive, &[("out/a", b"a"), ("out/b/c", b"c")]);
        let result = extract(
            &archive,
            &staging,
            RemoteIoLimits {
                max_archive_entries: entry_limit,
                max_metadata_bytes: metadata_limit,
                ..RemoteIoLimits::default()
            },
        );
        assert_eq!(std::fs::read(staging.join("out/a")).unwrap(), b"a");
        if succeeds {
            result.unwrap();
            assert_eq!(std::fs::read(staging.join("out/b/c")).unwrap(), b"c");
        } else {
            let resource = if entry_limit == 3 {
                "archive entries"
            } else {
                "output metadata bytes"
            };
            assert!(
                matches!(result, Err(Error::ExecutorResourceLimit { resource: actual, .. }) if actual == resource)
            );
            assert!(!staging.join("out/b").exists());
        }
    }
}

#[test]
fn charges_all_ancestor_metadata_before_first_allocation() {
    let temp = tempfile::tempdir().unwrap();
    let archive = temp.path().join("outputs.tar");
    let staging = temp.path().join("staging");
    std::fs::create_dir(&staging).unwrap();
    super::tests::write_archive(&archive, &[("out/a/b/c/d/file", b"x")]);
    let error = extract(
        &archive,
        &staging,
        RemoteIoLimits {
            max_metadata_bytes: 1024,
            ..RemoteIoLimits::default()
        },
    )
    .unwrap_err();
    assert!(matches!(
        error,
        Error::ExecutorResourceLimit {
            resource: "output metadata bytes",
            ..
        }
    ));
    assert_eq!(std::fs::read_dir(staging).unwrap().count(), 0);
}

#[tokio::test]
async fn installs_reasonably_nested_output() {
    let temp = tempfile::tempdir().unwrap();
    let archive = temp.path().join("outputs.tar");
    let path = format!("out/{}file", "nested/".repeat(12));
    super::tests::write_archive(&archive, &[(&path, b"x")]);
    install_output_archive_limited(
        &archive,
        temp.path(),
        &[WorkspacePath::try_from("out").unwrap()],
        "test",
        RemoteIoLimits {
            max_archive_entries: 14,
            max_metadata_bytes: 16 * 1024,
            ..RemoteIoLimits::default()
        },
        PhasePermit::default(),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(temp.path().join(path)).unwrap(), b"x");
}

#[test]
fn duplicate_directories_charge_nodes_and_modes_only_once() {
    let temp = tempfile::tempdir().unwrap();
    let archive = temp.path().join("outputs.tar");
    let staging = temp.path().join("staging");
    std::fs::create_dir(&staging).unwrap();
    let mut builder = tar::Builder::new(std::fs::File::create(&archive).unwrap());
    for (path, kind, mode) in [
        ("out/d/file", tar::EntryType::Regular, 0o644),
        ("out/d", tar::EntryType::Directory, 0o755),
        ("out/d", tar::EntryType::Directory, 0o750),
        ("out", tar::EntryType::Directory, 0o755),
        ("out", tar::EntryType::Directory, 0o750),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(kind);
        header.set_mode(mode);
        header.set_size(0);
        header.set_cksum();
        builder
            .append_data(&mut header, path, std::io::empty())
            .unwrap();
    }
    builder.finish().unwrap();
    let metadata_cost = ["out", "out/d", "out/d/file"]
        .iter()
        .map(|path| path.len() as u64 + 256)
        .sum::<u64>()
        + ["out", "out/d"]
            .iter()
            .map(|path| path.len() as u64 + 64)
            .sum::<u64>();
    let modes = extract(
        &archive,
        &staging,
        RemoteIoLimits {
            max_archive_entries: 5,
            max_metadata_bytes: metadata_cost,
            ..RemoteIoLimits::default()
        },
    )
    .unwrap();
    assert_eq!(modes.len(), 2);
    assert_eq!(modes[Path::new("out")], 0o750);
    assert_eq!(modes[Path::new("out/d")], 0o750);
    assert!(staging.join("out/d/file").is_file());
}

#[test]
fn charges_directory_modes_and_link_targets_before_allocation() {
    for kind in [tar::EntryType::Directory, tar::EntryType::Symlink] {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("outputs.tar");
        let staging = temp.path().join("staging");
        std::fs::create_dir(&staging).unwrap();
        let mut builder = tar::Builder::new(std::fs::File::create(&archive).unwrap());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(kind);
        header.set_mode(0o755);
        header.set_size(0);
        if kind.is_symlink() {
            header.set_link_name("target").unwrap();
        }
        header.set_cksum();
        builder
            .append_data(&mut header, "out/node", std::io::empty())
            .unwrap();
        builder.finish().unwrap();
        let nodes_cost = ("out".len() + "out/node".len()) as u64 + 2 * 256;
        let extra_cost = if kind.is_dir() {
            "out/node".len() as u64 + 64
        } else {
            "target".len() as u64
        };
        let error = extract(
            &archive,
            &staging,
            RemoteIoLimits {
                max_metadata_bytes: nodes_cost + extra_cost - 1,
                ..RemoteIoLimits::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            Error::ExecutorResourceLimit {
                resource: "output metadata bytes",
                ..
            }
        ));
        assert_eq!(std::fs::read_dir(staging).unwrap().count(), 0);
    }
}
