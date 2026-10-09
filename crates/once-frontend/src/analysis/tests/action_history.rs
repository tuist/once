use super::*;

#[test]
fn history_keys_are_canonical_bounded_optional_and_legacy_safe() {
    let tmp = TempDir::new().unwrap();
    let (store, ()) = with_active_store(store_for(tmp.path(), "pkg"), || {
        run(r#"
run_action(["tool"], history = action_history_key("producer.v1", ["a:b", "c"]))
run_action(["tool"], history = action_history_key("producer.v1", ["a", "b:c"]))
run_action(["tool"], history = action_history_key("producer.v1", ["a:b", "c"]))
run_action(["tool"], history = {"namespace": "invalid namespace", "key": "key"})
run_action(["tool"], history = action_history_key("producer.v1", ["secret\n"]))
run_action(["tool"], history = action_history_key("producer.v1", ["x"] * 9))
run_action(["tool"], history = action_history_key("producer.v1", ["x" * 2049]))
run_action(["tool"], history = {"namespace": "future.v9", "key": "opaque"})
run_action(["tool"])
"#)
        .unwrap();
    });
    assert_ne!(store.actions[0].history, store.actions[1].history);
    assert_eq!(store.actions[0].history, store.actions[2].history);
    assert_eq!(store.actions[0].history.as_ref().unwrap().key.len(), 64);
    for action in &store.actions[3..7] {
        assert!(action.history.is_none());
    }
    assert_eq!(
        store.actions[7].history.as_ref().unwrap().namespace,
        "future.v9"
    );
    assert!(store.actions[8].history.is_none());
    let mut legacy = serde_json::to_value(&store.actions[0]).unwrap();
    legacy.as_object_mut().unwrap().remove("history");
    let legacy: DeclaredAction = serde_json::from_value(legacy).unwrap();
    assert!(legacy.history.is_none());
}

#[test]
fn cargo_history_is_stable_across_versions_labels_revisions_and_registry_protocols() {
    let tmp = TempDir::new().unwrap();
    let source = format!(
        "{}\n{}\n{}",
        include_str!("../../../prelude/common.star"),
        include_str!("../../../prelude/rust.star"),
        r#"
def ctx(version, source, label, host = False):
    return {"attr": {"package_name": "serde", "crate_name": "serde", "version": version, "source": source, "_cargo_host_tool": host}, "label": {"id": label, "name": label}}
def emit(c, step = "crate.compile.rlib", target = "x86_64-unknown-linux-gnu"):
    run_action(["tool"], history = _rust_action_history(c, step, target))
emit(ctx("1.0.200", "registry+https://github.com/rust-lang/crates.io-index", "serde-1.0.200"))
emit(ctx("1.0.228", "sparse+https://index.crates.io/", "serde-1.0.228-other-label"))
emit(ctx("2.0.0", "sparse+https://index.crates.io/", "serde-2.0.0"))
emit(ctx("1.0.228", "sparse+https://index.crates.io/", "serde-host", True))
emit(ctx("1.0.228", "sparse+https://index.crates.io/", "serde"), target = "aarch64-apple-darwin")
emit(ctx("1.0.228", "sparse+https://index.crates.io/", "serde"), step = "build-script.compile")
emit(ctx("1.0.228", "git+https://example.invalid/repo?rev=old#old", "old"))
emit(ctx("1.0.229", "git+https://example.invalid/repo?branch=new#new", "new"))
emit(ctx("1.0.228", "git+https://user:secret@example.invalid/repo#rev", "private"))
emit(ctx("1.0.228", "", "source-less-generated"))
scoped = ctx("1.0.228", "sparse+https://index.crates.io/", "serde")
scoped["label"]["package"] = "second/workspace"
emit(scoped)
mobile = ctx("1.0.228", "sparse+https://index.crates.io/", "serde")
mobile["_action_suffix"] = "apple"
mobile["_history_consumer"] = "app"
emit(mobile)
mobile["_history_consumer"] = "extension"
emit(mobile)
emit(ctx("1.0.228", "git+ssh://git@example.invalid/repo#old", "ssh-old"))
emit(ctx("1.0.229", "git+ssh://example.invalid/repo#new", "ssh-new"))
bad = ctx("1.0.228", "sparse+https://index.crates.io/", "bad")
bad["attr"]["version"] = {"select": {"default": "1.0.228"}}
emit(bad)
"#
    );
    let (store, ()) = with_active_store(store_for(tmp.path(), "pkg"), || {
        run(&source).unwrap();
    });
    let keys = store
        .actions
        .iter()
        .map(|action| action.history.as_ref())
        .collect::<Vec<_>>();
    assert_eq!(keys[0], keys[1]);
    for key in &keys[2..6] {
        assert_ne!(keys[0], *key);
    }
    assert!(keys[6].is_some());
    assert_eq!(keys[6], keys[7]);
    assert_eq!(keys[6], keys[8]);
    assert!(keys[9].is_none());
    assert_ne!(keys[1], keys[10]);
    assert_ne!(keys[11], keys[12]);
    assert!(keys[13].is_some());
    assert_eq!(keys[13], keys[14]);
    assert!(keys[15].is_none());
}

#[test]
fn cargo_history_buckets_separate_incompatible_zero_and_prerelease_versions() {
    let source = format!("{}\n{}\nvalue = repr([_rust_history_bucket(v) for v in [\"1.2.3\", \"1.9.9+build\", \"0.3.6\", \"0.3.9\", \"0.0.5\", \"0.0.6\", \"1.0.0-beta.2\", \"0.11.0+wasi-snapshot-preview1\", \"1.0.0-rc.1+b\", \"opaque\"]])", include_str!("../../../prelude/common.star"), include_str!("../../../prelude/rust.star"));
    assert_eq!(
        eval_string(&source).unwrap(),
        "[\"1\", \"1\", \"0.3\", \"0.3\", \"0.0.5\", \"0.0.6\", \"1.0.0-beta.2\", \"0.11\", \"1.0.0-rc.1\", \"opaque\"]"
    );
}
