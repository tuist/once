use super::download::extract_zip_archive;
use super::fs::{copy_tree_contents_blocking, file_sha256_hex, tree_digest_bytes};

enum ZipFixtureEntry<'a> {
    File(&'a str, &'a str),
    Symlink(&'a str, &'a str),
}

fn write_zip(path: &std::path::Path, entries: &[ZipFixtureEntry<'_>]) {
    let mut archive = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    for entry in entries {
        match entry {
            ZipFixtureEntry::File(name, contents) => {
                archive.start_file(*name, options).unwrap();
                std::io::Write::write_all(&mut archive, contents.as_bytes()).unwrap();
            }
            ZipFixtureEntry::Symlink(name, target) => {
                archive.add_symlink(*name, *target, options).unwrap();
            }
        }
    }
    archive.finish().unwrap();
}

fn extract_fixture(entries: &[ZipFixtureEntry<'_>]) -> (tempfile::TempDir, crate::Result<()>) {
    extract_fixture_within(entries, crate::RemoteIoLimits::default())
}

fn extract_fixture_within(
    entries: &[ZipFixtureEntry<'_>],
    limits: crate::RemoteIoLimits,
) -> (tempfile::TempDir, crate::Result<()>) {
    let tmp = tempfile::TempDir::new().unwrap();
    let archive = tmp.path().join("fixture.zip");
    write_zip(&archive, entries);
    let result = extract_zip_archive(
        &archive,
        &tmp.path().join("output"),
        limits,
        limits.max_archive_bytes,
    );
    (tmp, result)
}

#[test]
fn extraction_stops_at_the_extracted_byte_limit() {
    use ZipFixtureEntry::File;
    let limits = crate::RemoteIoLimits {
        max_archive_bytes: 8,
        ..crate::RemoteIoLimits::default()
    };
    let (_tmp, within) = extract_fixture_within(&[File("a", "1234"), File("b", "5678")], limits);
    within.unwrap();
    let (_tmp, over) = extract_fixture_within(&[File("a", "1234"), File("b", "56789")], limits);
    assert!(matches!(
        over,
        Err(crate::Error::ExecutorResourceLimit {
            resource: "extracted bytes",
            ..
        })
    ));
}

#[test]
fn extraction_refuses_archives_with_too_many_entries() {
    use ZipFixtureEntry::File;
    let limits = crate::RemoteIoLimits {
        max_archive_entries: 1,
        ..crate::RemoteIoLimits::default()
    };
    let (_tmp, result) = extract_fixture_within(&[File("a", "1"), File("b", "2")], limits);
    assert!(matches!(
        result,
        Err(crate::Error::ExecutorResourceLimit {
            resource: "archive entries",
            ..
        })
    ));
}

#[cfg(unix)]
#[test]
fn extract_zip_archive_restores_framework_symlinks() {
    use ZipFixtureEntry::{File, Symlink};
    let (tmp, result) = extract_fixture(&[
        File("Vendor.framework/Versions/A/Vendor", "binary"),
        File("Vendor.framework/Versions/A/Resources/Info.plist", "plist"),
        Symlink("Vendor.framework/Versions/Current", "A"),
        Symlink("Vendor.framework/Vendor", "Versions/Current/Vendor"),
        Symlink("Vendor.framework/Resources", "Versions/Current/Resources"),
        Symlink("Vendor.framework/Sibling", "../Other"),
    ]);
    result.unwrap();
    let framework = tmp.path().join("output/Vendor.framework");
    assert_eq!(
        std::fs::read_link(framework.join("Resources")).unwrap(),
        std::path::Path::new("Versions/Current/Resources")
    );
    assert_eq!(
        std::fs::read_to_string(framework.join("Resources/Info.plist")).unwrap(),
        "plist"
    );
    assert_eq!(
        std::fs::read_to_string(framework.join("Vendor")).unwrap(),
        "binary"
    );
}

#[test]
fn extract_zip_archive_rejects_symlinks_that_leave_the_destination() {
    use ZipFixtureEntry::{File, Symlink};
    for entries in [
        vec![Symlink("Vendor/escape", "/etc")],
        vec![Symlink("Vendor/escape", "../../outside")],
        vec![Symlink("escape", "..")],
        vec![Symlink("root", "."), Symlink("Vendor/escape", "../root/..")],
        vec![Symlink("root", "."), Symlink("root/escape", "..")],
        vec![Symlink("root", "."), File("root/file", "contents")],
    ] {
        let (tmp, result) = extract_fixture(&entries);
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains("outside the destination") || error.contains("through a symbolic link"),
            "{error}"
        );
        assert!(!tmp.path().join("outside").exists());
    }
}

fn tree_digest_string(root: &std::path::Path, suffixes: &[String]) -> String {
    String::from_utf8(tree_digest_bytes(root, suffixes).unwrap()).unwrap()
}

#[test]
fn file_sha256_hex_matches_known_vector() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("hello.txt");
    std::fs::write(&path, b"hello").unwrap();
    assert_eq!(
        file_sha256_hex(&path).unwrap(),
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
}

#[test]
fn tree_digest_is_empty_for_missing_or_non_directory_roots() {
    let tmp = tempfile::TempDir::new().unwrap();
    assert!(tree_digest_bytes(&tmp.path().join("absent"), &[])
        .unwrap()
        .is_empty());
    let file = tmp.path().join("file");
    std::fs::write(&file, b"x").unwrap();
    assert!(tree_digest_bytes(&file, &[]).unwrap().is_empty());
}

#[test]
fn tree_digest_lists_files_sorted_and_relative() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
    std::fs::write(tmp.path().join("b.txt"), b"b").unwrap();
    std::fs::write(tmp.path().join("a.txt"), b"a").unwrap();
    std::fs::write(tmp.path().join("sub").join("c.txt"), b"c").unwrap();

    let rendered = tree_digest_string(tmp.path(), &[]);
    let paths: Vec<&str> = rendered
        .lines()
        .map(|line| line.rsplit("  ").next().unwrap())
        .collect();
    assert_eq!(paths, ["a.txt", "b.txt", "sub/c.txt"]);
}

#[test]
fn tree_digest_honors_suffix_filter() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("keep.txt"), b"1").unwrap();
    std::fs::write(tmp.path().join("skip.rs"), b"2").unwrap();

    let rendered = tree_digest_string(tmp.path(), &[".txt".to_string()]);
    assert!(rendered.contains("keep.txt"));
    assert!(!rendered.contains("skip.rs"));
}

#[test]
fn tree_digest_is_independent_of_creation_order() {
    let first = tempfile::TempDir::new().unwrap();
    std::fs::write(first.path().join("a"), b"one").unwrap();
    std::fs::write(first.path().join("b"), b"two").unwrap();

    let second = tempfile::TempDir::new().unwrap();
    std::fs::write(second.path().join("b"), b"two").unwrap();
    std::fs::write(second.path().join("a"), b"one").unwrap();

    assert_eq!(
        tree_digest_bytes(first.path(), &[]).unwrap(),
        tree_digest_bytes(second.path(), &[]).unwrap()
    );
}

#[test]
fn copy_tree_contents_preserves_nested_files() {
    let source = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(source.path().join("nested")).unwrap();
    std::fs::write(source.path().join("top.txt"), b"top").unwrap();
    std::fs::write(source.path().join("nested").join("deep.txt"), b"deep").unwrap();

    let dest = tempfile::TempDir::new().unwrap();
    copy_tree_contents_blocking(source.path(), dest.path()).unwrap();

    assert_eq!(std::fs::read(dest.path().join("top.txt")).unwrap(), b"top");
    assert_eq!(
        std::fs::read(dest.path().join("nested").join("deep.txt")).unwrap(),
        b"deep"
    );
}

#[cfg(unix)]
#[test]
fn copy_tree_contents_preserves_the_host_tree_digest() {
    use std::os::unix::fs::PermissionsExt;

    let source = tempfile::TempDir::new().unwrap();
    let nested = source.path().join("Framework.framework");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o775)).unwrap();
    let executable = nested.join("Framework");
    std::fs::write(&executable, b"binary").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();

    let destination = tempfile::TempDir::new().unwrap();
    copy_tree_contents_blocking(source.path(), destination.path()).unwrap();

    assert_eq!(
        struo_host_tree::host_tree_sha256_hex(source.path()).unwrap(),
        struo_host_tree::host_tree_sha256_hex(destination.path()).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn copy_tree_contents_preserves_symlinks() {
    let source = tempfile::TempDir::new().unwrap();
    std::fs::write(source.path().join("target.txt"), b"data").unwrap();
    std::os::unix::fs::symlink("target.txt", source.path().join("link.txt")).unwrap();

    let dest = tempfile::TempDir::new().unwrap();
    copy_tree_contents_blocking(source.path(), dest.path()).unwrap();

    let copied = dest.path().join("link.txt");
    assert!(std::fs::symlink_metadata(&copied)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        std::fs::read_link(&copied).unwrap(),
        std::path::Path::new("target.txt")
    );
}
