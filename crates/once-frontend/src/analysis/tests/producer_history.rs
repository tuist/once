use super::*;

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
