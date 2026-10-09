use once_events_client::proto::SourceFileStatus;

use super::*;

fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn repository() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "-q"]);
    git(root.path(), &["config", "user.email", "test@example.com"]);
    git(root.path(), &["config", "user.name", "test"]);
    std::fs::create_dir_all(root.path().join("app/src")).unwrap();
    std::fs::create_dir_all(root.path().join("vendor")).unwrap();
    std::fs::write(root.path().join(".gitignore"), "vendor/\n").unwrap();
    std::fs::write(root.path().join("app/src/main.rs"), "source").unwrap();
    std::fs::write(root.path().join("vendor/tracked.rs"), "source").unwrap();
    std::fs::write(root.path().join("vendor/ignored.rs"), "source").unwrap();
    git(root.path(), &["add", ".gitignore", "app"]);
    git(root.path(), &["add", "-f", "vendor/tracked.rs"]);
    git(
        root.path(),
        &["-c", "commit.gpgsign=false", "commit", "-qm", "initial"],
    );
    root
}

#[tokio::test]
async fn commit_membership_keeps_ignored_and_untracked_sources_visible() {
    let root = repository();
    let sha = revision(root.path()).await;
    let snapshot = collect(root.path(), &sha).await;
    assert_eq!(
        snapshot.status("app/src/main.rs"),
        SourceFileStatus::Committed
    );
    assert_eq!(
        snapshot.status("vendor/tracked.rs"),
        SourceFileStatus::Committed
    );
    assert_eq!(
        snapshot.status("vendor/ignored.rs"),
        SourceFileStatus::NotCommitted
    );
    assert_eq!(
        snapshot.status("app/src/Main.rs"),
        SourceFileStatus::NotCommitted
    );
    std::fs::write(root.path().join("new.rs"), "new").unwrap();
    git(root.path(), &["add", "new.rs"]);
    assert_eq!(snapshot.status("new.rs"), SourceFileStatus::NotCommitted);
}

#[tokio::test]
async fn the_checked_out_branch_is_reported_and_a_detached_head_has_none() {
    let root = repository();
    git(root.path(), &["checkout", "-q", "-b", "feat/report-branch"]);
    assert_eq!(checked_out_branch(root.path()).await, "feat/report-branch");
    assert_eq!(
        checked_out_branch(&root.path().join("app")).await,
        "feat/report-branch"
    );

    git(root.path(), &["checkout", "-q", "--detach"]);
    assert_eq!(checked_out_branch(root.path()).await, "");
}

#[tokio::test]
async fn nested_workspace_paths_already_have_the_repository_prefix() {
    let root = repository();
    let workspace = root.path().join("app");
    let snapshot = collect(&workspace, &revision(&workspace).await).await;
    assert_eq!(
        snapshot.status("app/src/main.rs"),
        SourceFileStatus::Committed
    );
    assert_eq!(
        snapshot.status("src/main.rs"),
        SourceFileStatus::NotCommitted
    );
}

#[tokio::test]
async fn moving_head_and_deleting_a_file_does_not_change_the_snapshot() {
    let root = repository();
    let initial = revision(root.path()).await;
    git(root.path(), &["rm", "app/src/main.rs"]);
    git(
        root.path(),
        &["-c", "commit.gpgsign=false", "commit", "-qm", "delete"],
    );
    let old = collect(root.path(), &initial).await;
    let current = collect(root.path(), &revision(root.path()).await).await;
    assert_eq!(old.status("app/src/main.rs"), SourceFileStatus::Committed);
    assert_eq!(
        current.status("app/src/main.rs"),
        SourceFileStatus::NotCommitted
    );
}

#[tokio::test]
async fn missing_commits_and_nonrepositories_are_unknown() {
    let root = repository();
    for sha in ["", "HEAD", &"0".repeat(40)] {
        assert_eq!(
            collect(root.path(), sha).await.status("app/src/main.rs"),
            SourceFileStatus::Unknown
        );
    }
    let root = tempfile::tempdir().unwrap();
    assert_eq!(revision(root.path()).await, "");
    assert_eq!(
        collect(root.path(), &"0".repeat(40))
            .await
            .status("file.rs"),
        SourceFileStatus::Unknown
    );
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_are_blobs_and_gitlinks_are_not_repository_files() {
    let root = repository();
    std::os::unix::fs::symlink("app/src/main.rs", root.path().join("link.rs")).unwrap();
    git(root.path(), &["add", "link.rs"]);
    let initial = revision(root.path()).await;
    git(
        root.path(),
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{initial},module"),
        ],
    );
    git(
        root.path(),
        &["-c", "commit.gpgsign=false", "commit", "-qm", "links"],
    );
    let snapshot = collect(root.path(), &revision(root.path()).await).await;
    assert_eq!(snapshot.status("link.rs"), SourceFileStatus::Committed);
    assert_eq!(snapshot.status("module"), SourceFileStatus::NotCommitted);
    assert_eq!(
        snapshot.status("module/file.rs"),
        SourceFileStatus::NotCommitted
    );
}

#[tokio::test]
async fn a_submodule_checkout_is_unknown_instead_of_linking_the_parent_repository() {
    let parent = repository();
    let dependency = repository();
    git(
        parent.path(),
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            dependency.path().to_str().unwrap(),
            "module",
        ],
    );
    let workspace = parent.path().join("module");
    let snapshot = collect(&workspace, &revision(&workspace).await).await;
    assert_eq!(
        snapshot.status("app/src/main.rs"),
        SourceFileStatus::Unknown
    );
}

#[tokio::test]
async fn output_bounds_fail_without_partial_classification() {
    let root = repository();
    assert!(git_output(root.path(), &["ls-tree", "-r", "HEAD"], 1)
        .await
        .is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn timeouts_kill_and_reap_the_child() {
    let mut command = Command::new("sleep");
    command.arg("60").stdout(Stdio::piped()).kill_on_drop(true);
    let start = std::time::Instant::now();
    assert!(command_output(command, 128, Duration::from_millis(20))
        .await
        .is_none());
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn byte_budget_accommodates_the_entry_budget_at_typical_path_lengths() {
    let record = format!(
        "100644 blob {}\t{}\0",
        "a".repeat(64),
        "source.rs".repeat(8)
    );
    assert!(record.len() * MAX_TREE_ENTRIES <= MAX_TREE_BYTES);
}

#[test]
fn tree_parser_is_nul_delimited_and_rejects_partial_or_oversized_listings() {
    let tree = b"100644 blob abc\tfile with spaces.rs\0";
    assert_eq!(
        parse_tree(tree, 1).unwrap(),
        HashSet::from(["file with spaces.rs".into()])
    );
    assert!(parse_tree(tree, 0).is_none());
    assert!(parse_tree(&tree[..tree.len() - 1], 1).is_none());
    assert!(parse_tree(b"100644 tree abc\tfile.rs\0", 1).is_none());
}
