use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

use super::{KnownChanges, SourceDigestCache};

#[tokio::test]
async fn restoring_a_tree_preserves_a_matching_newer_child() {
    use once_cas::{CacheProvider, Cas};
    use once_core::{Action, CopyPathMode, WorkspacePath};

    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::Local(Cas::open(workspace.path().join("cache")));
    let digests = SourceDigestCache::open(workspace.path());
    std::fs::create_dir(workspace.path().join("seed")).unwrap();
    std::fs::write(workspace.path().join("seed/file"), "old").unwrap();
    let tree = Action::CopyPath {
        sources: vec![WorkspacePath::try_from("seed").unwrap()],
        destination: WorkspacePath::try_from("out/tree").unwrap(),
        mode: CopyPathMode::Tree,
        input_digest: None,
    };
    let mut result = once_core::run_uncached(&tree, workspace.path(), &cache, false)
        .await
        .unwrap();
    let original_tree = result.clone();
    digests.record_outputs(&result, workspace.path());
    let child = Action::WriteFile {
        path: WorkspacePath::try_from("out/tree/file").unwrap(),
        bytes: b"new".to_vec(),
        input_digest: None,
    };
    let updated = once_core::run_uncached(&child, workspace.path(), &cache, false)
        .await
        .unwrap();
    digests.record_outputs(&updated, workspace.path());
    result.outputs.extend(updated.outputs.clone());
    digests
        .materialize_outputs(&result, workspace.path(), &cache)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("out/tree/file")).unwrap(),
        "new"
    );
    digests
        .materialize_outputs(&original_tree, workspace.path(), &cache)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("out/tree/file")).unwrap(),
        "old"
    );
    digests.with_known_changes(KnownChanges::Since {
        sources: BTreeSet::new(),
        outputs: BTreeSet::new(),
    });
    digests
        .materialize_outputs(&updated, workspace.path(), &cache)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("out/tree/file")).unwrap(),
        "new"
    );
}

#[test]
fn unchanged_files_reuse_the_persisted_digest() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("source.rs"), "one").unwrap();

    let first = SourceDigestCache::open(workspace.path());
    let digest = first.digest(workspace.path(), "source.rs").unwrap();
    drop(first);

    let second = SourceDigestCache::open(workspace.path());
    assert_eq!(
        second.inner.entries.read().unwrap().len(),
        1,
        "the reopened cache should load its persisted entry"
    );
    assert!(!second.inner.dirty.load(Ordering::Relaxed));
    assert_eq!(
        second.digest(workspace.path(), "source.rs").unwrap(),
        digest
    );
    assert!(!second.inner.dirty.load(Ordering::Relaxed));
}

#[test]
fn same_length_content_changes_invalidate_the_digest() {
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path().join("source.rs");
    std::fs::write(&path, "one").unwrap();
    let cache = SourceDigestCache::open(workspace.path());
    let first = cache.digest(workspace.path(), "source.rs").unwrap();

    std::fs::write(&path, "two").unwrap();

    assert_ne!(cache.digest(workspace.path(), "source.rs").unwrap(), first);
}

#[test]
fn malformed_cache_files_are_ignored() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join(".once")).unwrap();
    std::fs::write(
        workspace.path().join(".once/source-digests.json"),
        "not json",
    )
    .unwrap();
    std::fs::write(workspace.path().join("source.rs"), "content").unwrap();

    let cache = SourceDigestCache::open(workspace.path());

    assert!(cache.digest(workspace.path(), "source.rs").is_ok());
}

#[test]
fn changed_outputs_do_not_reuse_the_recorded_digest() {
    let workspace = tempfile::tempdir().unwrap();
    let relative = ".once/out/library.rlib";
    std::fs::create_dir_all(workspace.path().join(".once/out")).unwrap();
    std::fs::write(workspace.path().join(relative), "first").unwrap();
    let expected = once_cas::Digest::of_bytes(b"encoded output");
    let cache = SourceDigestCache::open(workspace.path());
    cache.record_output(workspace.path(), relative, expected);

    assert!(cache.output_matches(workspace.path(), relative, expected));

    std::fs::write(workspace.path().join(relative), "other").unwrap();

    assert!(!cache.output_matches(workspace.path(), relative, expected));
}

#[test]
fn removed_outputs_do_not_reuse_watcher_trusted_digests() {
    let workspace = tempfile::tempdir().unwrap();
    let relative = ".once/out/library.rlib";
    std::fs::create_dir_all(workspace.path().join(".once/out")).unwrap();
    std::fs::write(workspace.path().join(relative), "first").unwrap();
    let expected = once_cas::Digest::of_bytes(b"encoded output");
    let cache = SourceDigestCache::open(workspace.path());
    cache.record_output(workspace.path(), relative, expected);
    cache.with_known_changes(KnownChanges::Since {
        sources: BTreeSet::new(),
        outputs: BTreeSet::new(),
    });

    std::fs::remove_file(workspace.path().join(relative)).unwrap();

    assert!(!cache.output_matches(workspace.path(), relative, expected));
}

#[test]
fn receipt_observations_include_only_reported_changes() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("one.rs"), "one").unwrap();
    std::fs::write(workspace.path().join("two.rs"), "two").unwrap();
    let cache = SourceDigestCache::open(workspace.path());
    let one = cache.digest(workspace.path(), "one.rs").unwrap();
    cache.digest(workspace.path(), "two.rs").unwrap();

    assert_eq!(
        cache.observed_digests_for(Some(&["one.rs".to_string()])),
        std::collections::BTreeMap::from([("one.rs".to_string(), one)])
    );
    assert!(cache.observed_digests_for(None).is_empty());
}

#[test]
fn changes_match_ignores_disappeared_unobserved_temporary_files() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = SourceDigestCache::open(workspace.path());

    assert!(cache.changes_match(workspace.path(), Some(&["temporary.rmeta".to_string()])));

    std::fs::write(workspace.path().join("temporary.rmeta"), "metadata").unwrap();

    assert!(!cache.changes_match(workspace.path(), Some(&["temporary.rmeta".to_string()])));
}

#[test]
fn changes_match_requires_observed_inputs_to_retain_their_digest() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("source.rs"), "one").unwrap();
    let cache = SourceDigestCache::open(workspace.path());
    cache.digest(workspace.path(), "source.rs").unwrap();

    assert!(cache.changes_match(workspace.path(), Some(&["source.rs".to_string()])));

    std::fs::write(workspace.path().join("source.rs"), "two").unwrap();

    assert!(!cache.changes_match(workspace.path(), Some(&["source.rs".to_string()])));
}
