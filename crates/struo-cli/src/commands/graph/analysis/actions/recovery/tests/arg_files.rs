use super::*;

#[tokio::test]
async fn repairing_a_cached_producer_recreates_its_deferred_argument_files() {
    let workspace = Workspace::new();
    let mut draft = analysis(&format!(
        "cat .struo/args/produce.txt > {PRODUCED}; printf run >> runs.log"
    ));
    draft.actions.truncate(1);
    draft.actions[0]
        .arg_files
        .push(struo_frontend::analysis::DeclaredArgFile {
            path: ".struo/args/produce.txt".into(),
            format: struo_frontend::analysis::DeclaredArgFileFormat::LineDelimited,
            args: vec!["from-args".into()],
        });
    workspace.build(draft.clone()).await.unwrap();
    std::fs::remove_file(workspace.path().join(".struo/args/produce.txt")).unwrap();
    let producers = Producers::default();
    let cached = workspace.build_with(draft, &producers, None).await.unwrap();
    assert_eq!(cached.cache_state, EvidenceCacheState::Hit);
    assert!(
        !workspace.path().join(".struo/args/produce.txt").exists(),
        "a cache hit does not write argument files"
    );
    let digest = cached.result.outputs[PRODUCED];
    workspace.lose(digest);
    workspace.restore(&producers, digest).await.unwrap();
    assert_eq!(workspace.read(PRODUCED), "from-args\n");
    assert_eq!(workspace.runs(), 2);
}
