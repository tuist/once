use super::*;

#[test]
fn zig_native_history_matches_unset_target_and_separates_hosts() {
    let tmp = TempDir::new().unwrap();
    let source = format!(
        "{}\n{}\n{}",
        include_str!("../../../prelude/common.star"),
        include_str!("../../../prelude/zig.star"),
        r#"
def host_os():
    return current_os
def host_arch():
    return current_arch
def emit(target):
    ctx = {"label": {"id": "pkg/library"}, "attr": {"target": target}}
    run_action(["zig"], history = _action_history(ctx, "zig", "compile", ["build-lib"] + _zig_history_platform(ctx)))
current_os = "macos"
current_arch = "aarch64"
emit("")
emit("native")
current_os = "linux"
current_arch = "x86_64"
emit("native")
"#
    );
    let (store, ()) = with_active_store(store_for(tmp.path(), "pkg"), || run(&source).unwrap());
    assert!(store.actions[0].history.is_some());
    assert_eq!(store.actions[0].history, store.actions[1].history);
    assert_ne!(store.actions[1].history, store.actions[2].history);
}

#[test]
fn first_party_history_preserves_logical_steps_without_content_or_toolchain_facts() {
    let tmp = TempDir::new().unwrap();
    let source = format!(
        "{}\n{}",
        include_str!("../../../prelude/common.star"),
        r#"
ctx = {"label": {"id": "pkg/library"}}
def emit():
    for ecosystem in ["go", "kotlin", "zig", "cmake"]:
        run_action(["tool"], history = _action_history(ctx, ecosystem, "compile", ["native-output"]))
        run_action(["new-toolchain"], history = _action_history(ctx, ecosystem, "compile", ["native-output"]))
        run_action(["tool"], history = _action_history(ctx, ecosystem, "test.compile", ["native-output"]))
        run_action(["tool"], history = _action_history(ctx, ecosystem, "compile", ["other-output"]))
emit()
"#
    );
    let (store, ()) = with_active_store(store_for(tmp.path(), "pkg"), || {
        run(&source).unwrap();
    });
    for actions in store.actions.chunks_exact(4) {
        assert!(actions[0].history.is_some());
        assert_eq!(actions[0].history, actions[1].history);
        assert_ne!(actions[0].history, actions[2].history);
        assert_ne!(actions[0].history, actions[3].history);
    }
    assert_ne!(store.actions[0].history, store.actions[4].history);
}
