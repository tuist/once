use super::*;

#[test]
fn legacy_positional_arguments_remain_positional() {
    let tmp = TempDir::new().unwrap();
    let (store, ()) = with_active_store(store_for(tmp.path(), "pkg"), || {
        run(r#"
copy_path("a", "b", "file", [], "", "copy", True)
link_path("a", "b", "link")
prepare_path("dir", "directory", "prepare")
write_tree_digest("dir", "hash", [], [], "digest", True)
download_and_extract("https://example.invalid/archive", "a" * 64, "archive", "DOWNLOAD_TOKEN", "fetch", True)
"#)
        .unwrap();
    });
    assert_eq!(
        store
            .actions
            .iter()
            .map(|action| action.identifier.as_deref().unwrap())
            .collect::<Vec<_>>(),
        ["copy", "link", "prepare", "digest", "fetch"]
    );
}

#[test]
fn optional_metadata_never_turns_a_cosmetic_error_into_a_build_failure() {
    let tmp = TempDir::new().unwrap();
    let (store, ()) = with_active_store(store_for(tmp.path(), "pkg"), || {
        run(r#"
run_action(["tool"], presentation = {"package": {"name": None}})
run_action(["tool"], presentation = {"platforms": [{"scheme": "custom", "id": "target", "usage": "future"}], "context": [{"key": "custom.mode", "value": "test"}]})
"#).unwrap();
    });
    assert!(store.actions[0].presentation.is_none());
    let metadata = store.actions[1].presentation.as_ref().unwrap();
    assert_eq!(metadata.platforms[0].usage, "future");
    assert_eq!(metadata.context[0].value, "test");
}

#[test]
fn ecosystem_helpers_keep_native_identity_and_version_semantics() {
    let tmp = TempDir::new().unwrap();
    let source = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}",
        include_str!("../../../prelude/common.star"),
        include_str!("../../../prelude/rust.star"),
        include_str!("../../../prelude/elixir.star"),
        include_str!("../../../prelude/apple_modules.star"),
        include_str!("../../../prelude/apple_linux.star"),
        include_str!("../../../prelude/apple.star"),
        r#"
rust = {"attr": {"package_name": "serde", "version": "1.0", "source": "git+https://example.invalid/repo#abcdef"}, "label": {"name": "serde-1.0"}}
run_action(["tool"], presentation = _rust_action_metadata(rust, "aarch64-linux-android", "build-tool"))
run_action(["tool"], presentation = _rust_action_metadata(rust, "aarch64-apple-ios-macabi"))
mix = {"attr": {"_mix_source": "git:https://example.invalid/repo", "version": "abcdef", "_mix_revision": "abcdef"}, "label": {"id": "pkg", "name": "jason"}}
run_action(["tool"], presentation = _mix_action_metadata(mix, "test", "jason"))
run_action(["tool"], presentation = _apple_action_metadata("arm64-apple-ios17.0-simulator", "ios", "simulator", "arm64"))
"#
    );
    let (store, ()) = with_active_store(store_for(tmp.path(), "pkg"), || {
        run(&source).unwrap();
    });
    let rust = store.actions[0].presentation.as_ref().unwrap();
    assert_eq!(rust.platforms[0].label, "Android · aarch64");
    assert_eq!(rust.package.as_ref().unwrap().revision, "abcdef");
    assert_eq!(
        store.actions[1].presentation.as_ref().unwrap().platforms[0].label,
        "Mac Catalyst · aarch64"
    );
    let mix = store.actions[2].presentation.as_ref().unwrap();
    assert_eq!(mix.package.as_ref().unwrap().version, "");
    assert_eq!(mix.package.as_ref().unwrap().ecosystem, "mix");
    assert_eq!(mix.context[0].value, "test");
    assert_eq!(
        store.actions[3].presentation.as_ref().unwrap().platforms[0].label,
        "iOS Simulator · arm64"
    );
}
