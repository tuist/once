use super::*;
use std::io::Read;

#[test]
fn an_open_input_cannot_be_redirected_by_a_parent_or_leaf_swap() {
    let workspace = tempfile::tempdir().unwrap();
    let sibling = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join("input")).unwrap();
    std::fs::write(workspace.path().join("input/file"), b"declared").unwrap();
    std::fs::write(sibling.path().join("file"), b"coordinator only").unwrap();
    let root = Directory::open(workspace.path()).unwrap();
    let mut file = root.open_file(Path::new("input/file")).unwrap();
    std::fs::rename(
        workspace.path().join("input"),
        workspace.path().join("saved"),
    )
    .unwrap();
    std::os::unix::fs::symlink(sibling.path(), workspace.path().join("input")).unwrap();
    let mut contents = String::new();
    file.read_to_string(&mut contents).unwrap();
    assert_eq!(contents, "declared");
    assert!(root.open_file(Path::new("input/file")).is_err());
    std::fs::remove_file(workspace.path().join("saved/file")).unwrap();
    std::os::unix::fs::symlink(
        sibling.path().join("file"),
        workspace.path().join("saved/file"),
    )
    .unwrap();
    assert!(root.open_file(Path::new("saved/file")).is_err());
}

#[test]
fn publication_and_cleanup_keep_their_anchored_parent_after_a_symlink_swap() {
    let workspace = tempfile::tempdir().unwrap();
    let sibling = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join("out")).unwrap();
    std::fs::write(workspace.path().join("new"), b"new").unwrap();
    std::fs::write(sibling.path().join("file"), b"coordinator only").unwrap();
    let root = Directory::open(workspace.path()).unwrap();
    let destination = root.parent(Path::new("out/file"), false).unwrap();
    let source = root.parent(Path::new("new"), false).unwrap();
    std::fs::rename(workspace.path().join("out"), workspace.path().join("saved")).unwrap();
    std::os::unix::fs::symlink(sibling.path(), workspace.path().join("out")).unwrap();
    source.rename_to(&destination).unwrap();
    assert_eq!(
        std::fs::read(workspace.path().join("saved/file")).unwrap(),
        b"new"
    );
    destination.remove().unwrap();
    assert!(!workspace.path().join("saved/file").exists());
    assert_eq!(
        std::fs::read(sibling.path().join("file")).unwrap(),
        b"coordinator only"
    );
}

#[test]
fn recursive_cleanup_does_not_follow_links() {
    let workspace = tempfile::tempdir().unwrap();
    let sibling = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join("out")).unwrap();
    std::fs::write(sibling.path().join("secret"), b"coordinator only").unwrap();
    std::os::unix::fs::symlink(sibling.path(), workspace.path().join("out/alias")).unwrap();
    Directory::open(workspace.path())
        .unwrap()
        .parent(Path::new("out"), false)
        .unwrap()
        .remove()
        .unwrap();
    assert_eq!(
        std::fs::read(sibling.path().join("secret")).unwrap(),
        b"coordinator only"
    );
    assert!(!workspace.path().join("out").exists());
}
