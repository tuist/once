use super::super::{
    restore_directory_blob_from_reader, DIRECTORY_BLOB_MAGIC, DIRECTORY_BLOB_V1_MAGIC,
};
use std::io::{self, Cursor, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

type Entry<'a> = (&'a str, u8, u32, &'a [u8]);

fn blob(version_one: bool, entries: &[Entry<'_>]) -> Vec<u8> {
    let mut bytes = if version_one {
        DIRECTORY_BLOB_V1_MAGIC
    } else {
        DIRECTORY_BLOB_MAGIC
    }
    .to_vec();
    for (path, kind, mode, content) in entries {
        if !version_one {
            bytes.push(*kind);
        }
        bytes.extend(u32::try_from(path.len()).unwrap().to_le_bytes());
        bytes.extend(mode.to_le_bytes());
        bytes.extend(u64::try_from(content.len()).unwrap().to_le_bytes());
        bytes.extend(path.as_bytes());
        bytes.extend(*content);
    }
    bytes
}

fn assert_clean(parent: &std::path::Path) {
    assert!(std::fs::read_dir(parent).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".struo-tree-")
    }));
}

#[test]
fn private_files_preserve_entry_boundaries_modes_and_root_umask() {
    for version_one in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let expected_root = temp.path().join("normal-root");
        std::fs::create_dir(&expected_root).unwrap();
        let destination = temp.path().join("output");
        let payload = vec![19; 512 * 1024 + 5];
        let bytes = blob(
            version_one,
            &[
                ("one", 1, 0o751, &payload),
                ("nested/two", 1, 0o640, b"second"),
            ],
        );
        restore_directory_blob_from_reader("output", &destination, Cursor::new(bytes)).unwrap();
        assert_eq!(std::fs::read(destination.join("one")).unwrap(), payload);
        assert_eq!(
            std::fs::read(destination.join("nested/two")).unwrap(),
            b"second"
        );
        assert_eq!(
            std::fs::metadata(destination.join("one"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o751
        );
        assert_eq!(
            std::fs::metadata(&destination)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            std::fs::metadata(expected_root)
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        );
        assert_clean(temp.path());
    }
}

#[test]
fn truncation_and_read_errors_leave_no_visible_output_or_scratch() {
    struct Failed;
    impl Read for Failed {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("reader failed"))
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("output");
    let mut bytes = blob(
        false,
        &[
            ("one", 1, 0o644, b"complete"),
            ("two", 1, 0o644, b"incomplete"),
        ],
    );
    bytes.truncate(bytes.len() - 3);
    assert!(
        restore_directory_blob_from_reader("output", &destination, Cursor::new(bytes)).is_err()
    );
    assert!(!destination.exists());
    assert_clean(temp.path());
    let bytes = blob(false, &[("one", 1, 0o644, b"complete")]);
    assert!(restore_directory_blob_from_reader(
        "output",
        &destination,
        Cursor::new(bytes).chain(Failed)
    )
    .is_err());
    assert!(!destination.exists());
    assert_clean(temp.path());
}

#[test]
fn symlinks_roundtrip_but_entry_parents_cannot_follow_them() {
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("output");
    let bytes = blob(
        false,
        &[("data", 1, 0o644, b"payload"), ("link", 2, 0, b"data")],
    );
    restore_directory_blob_from_reader("output", &destination, Cursor::new(bytes)).unwrap();
    assert_eq!(
        std::fs::read_link(destination.join("link")).unwrap(),
        PathBuf::from("data")
    );
    let external = temp.path().join("external");
    std::fs::create_dir(&external).unwrap();
    let target = external.to_str().unwrap();
    let bytes = blob(
        false,
        &[
            ("link", 3, 0, target.as_bytes()),
            ("link/escape", 1, 0o644, b"must not escape"),
        ],
    );
    let fresh = temp.path().join("fresh");
    assert!(restore_directory_blob_from_reader("fresh", &fresh, Cursor::new(bytes)).is_err());
    assert!(!fresh.exists());
    assert!(!external.join("escape").exists());
    assert_clean(temp.path());
}

#[test]
fn publication_does_not_clobber_a_destination_created_during_decoding() {
    struct Appeared {
        bytes: Cursor<Vec<u8>>,
        destination: PathBuf,
        created: bool,
    }
    impl Read for Appeared {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            let count = self.bytes.read(bytes)?;
            if count == 0 && !self.created {
                std::fs::create_dir(&self.destination)?;
                std::fs::write(self.destination.join("keep"), b"concurrent output")?;
                self.created = true;
            }
            Ok(count)
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("output");
    let reader = Appeared {
        bytes: Cursor::new(blob(false, &[("one", 1, 0o644, b"new")])),
        destination: destination.clone(),
        created: false,
    };
    assert!(restore_directory_blob_from_reader("output", &destination, reader).is_err());
    assert_eq!(
        std::fs::read(destination.join("keep")).unwrap(),
        b"concurrent output"
    );
    assert!(!destination.join("one").exists());
    assert_clean(temp.path());
}

#[test]
fn partially_decoded_tree_stays_under_private_container_until_publication() {
    struct Watched {
        bytes: Cursor<Vec<u8>>,
        parent: PathBuf,
        destination: PathBuf,
        observed: bool,
    }
    impl Read for Watched {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            assert!(!self.destination.exists());
            for entry in std::fs::read_dir(&self.parent)? {
                let entry = entry?;
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".struo-tree-")
                {
                    assert_eq!(entry.metadata()?.permissions().mode() & 0o777, 0o700);
                    self.observed = true;
                }
            }
            self.bytes.read(bytes)
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("output");
    let mut reader = Watched {
        bytes: Cursor::new(blob(false, &[("one", 1, 0o777, b"payload")])),
        parent: temp.path().to_path_buf(),
        destination: destination.clone(),
        observed: false,
    };
    restore_directory_blob_from_reader("output", &destination, &mut reader).unwrap();
    assert!(reader.observed);
    assert_eq!(std::fs::read(destination.join("one")).unwrap(), b"payload");
    assert_clean(temp.path());
}

#[test]
fn unsupported_rename_is_detected_before_any_visible_tree_is_created() {
    for error in [
        rustix::io::Errno::NOSYS,
        rustix::io::Errno::NOTSUP,
        rustix::io::Errno::INVAL,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("output");
        let tree = super::Tree::begin_with("output", &destination, |_, _| Err(error)).unwrap();
        assert!(tree.is_none());
        assert!(!destination.exists());
        assert_clean(temp.path());
    }
    let temp = tempfile::tempdir().unwrap();
    assert!(
        super::Tree::begin_with("output", &temp.path().join("output"), |_, _| Err(
            rustix::io::Errno::ACCESS
        ))
        .is_err()
    );
    assert_clean(temp.path());
}

#[test]
fn conflicting_entries_cannot_replace_private_files_or_directories() {
    for entries in [
        vec![
            ("same", 1, 0o644, b"first".as_slice()),
            ("same", 1, 0o644, b"second".as_slice()),
        ],
        vec![
            ("same", 1, 0o644, b"first".as_slice()),
            ("same/child", 1, 0o644, b"second".as_slice()),
        ],
    ] {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("output");
        assert!(restore_directory_blob_from_reader(
            "output",
            &destination,
            Cursor::new(blob(false, &entries))
        )
        .is_err());
        assert!(!destination.exists());
        assert_clean(temp.path());
    }
}

#[test]
fn publication_does_not_replace_an_empty_directory() {
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("output");
    let tree = super::Tree::begin("output", &destination).unwrap().unwrap();
    std::fs::write(tree.root().join("new"), b"private bytes").unwrap();
    std::fs::create_dir(&destination).unwrap();
    assert!(tree.publish("output", &destination).is_err());
    assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 0);
    assert_clean(temp.path());
}

#[test]
fn existing_roots_keep_legacy_replacement_and_old_open_readers() {
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("output");
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(destination.join("old"), b"old bytes").unwrap();
    let mut old = std::fs::File::open(destination.join("old")).unwrap();
    let bytes = blob(false, &[("new", 1, 0o644, b"new bytes")]);
    restore_directory_blob_from_reader("output", &destination, Cursor::new(bytes)).unwrap();
    let mut bytes = Vec::new();
    old.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"old bytes");
    assert!(!destination.join("old").exists());
    assert_eq!(
        std::fs::read(destination.join("new")).unwrap(),
        b"new bytes"
    );
    assert_clean(temp.path());
}

#[test]
fn private_file_creation_preserves_behavior_without_a_directory_handle() {
    for anchored in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("output");
        let mut tree = super::Tree::begin("output", &destination).unwrap().unwrap();
        assert!(tree.directory.is_some());
        if !anchored {
            tree.directory = None;
        }
        let entry = super::StreamedDirectoryEntry {
            kind: super::DirectoryEntryKind::File,
            mode: 0o751,
            content_len: 7,
            destination: tree.root().join("file"),
        };
        tree.restore_entry("output", &entry, &mut Cursor::new(b"payload"))
            .unwrap();
        assert!(tree
            .restore_entry("output", &entry, &mut Cursor::new(b"replace"))
            .is_err());
        tree.publish("output", &destination).unwrap();
        assert_eq!(std::fs::read(destination.join("file")).unwrap(), b"payload");
        assert_eq!(
            std::fs::metadata(destination.join("file"))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o751
        );
        assert_clean(temp.path());
    }
}

#[test]
fn private_created_modes_are_corrected_after_content_writes() {
    for mode in [0o400, 0o640, 0o666, 0o751, 0o1751, 0o4751] {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("output");
        restore_directory_blob_from_reader(
            "output",
            &destination,
            Cursor::new(blob(false, &[("file", 1, mode, b"payload")])),
        )
        .unwrap();
        assert_eq!(
            std::fs::metadata(destination.join("file"))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            mode
        );
    }
    let temp = tempfile::tempdir().unwrap();
    let mut file = std::fs::File::create(temp.path().join("private")).unwrap();
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .unwrap();
    crate::restore_file::write_private_content(
        &mut file,
        Cursor::new(b"corrected"),
        0o666,
        Some(9),
    )
    .unwrap();
    assert_eq!(
        file.metadata().unwrap().permissions().mode() & 0o7777,
        0o666
    );
}

#[test]
fn existing_root_symlinks_do_not_mutate_their_targets() {
    let temp = tempfile::tempdir().unwrap();
    let external = temp.path().join("external");
    std::fs::create_dir(&external).unwrap();
    std::fs::write(external.join("keep"), b"external").unwrap();
    let destination = temp.path().join("output");
    std::os::unix::fs::symlink(&external, &destination).unwrap();
    restore_directory_blob_from_reader(
        "output",
        &destination,
        Cursor::new(blob(false, &[("new", 1, 0o644, b"new")])),
    )
    .unwrap();
    assert_eq!(std::fs::read(external.join("keep")).unwrap(), b"external");
    assert!(!external.join("new").exists());
    assert!(!std::fs::symlink_metadata(&destination)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_clean(temp.path());
}
