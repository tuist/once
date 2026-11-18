use super::*;

mod poison;

fn cached_graph(clean: bool) -> AnalysisResult {
    let mut analysis = graph(
        &format!("printf original > .struo/out/source.txt; ln -s source.txt {PRODUCED}"),
        clean,
        &format!("cat {PRODUCED} > .struo/out/consumed.txt; printf run >> runs.log"),
    );
    analysis.actions[0].clean_paths = vec![PRODUCED.to_string()];
    analysis.actions.last_mut().unwrap().cacheable = true;
    analysis
}

#[tokio::test]
async fn cached_consumer_cannot_bypass_fingerprint_validation() {
    let workspace = Workspace::new();
    let first = workspace
        .build_with(cached_graph(false), &Producers::default(), None)
        .await
        .unwrap();
    assert_eq!(workspace.runs(), 1);
    assert_eq!(
        first.per_action_outcomes.last().unwrap().cache_state,
        EvidenceCacheState::Miss
    );
    let hit = workspace
        .build_with(cached_graph(false), &Producers::default(), None)
        .await
        .unwrap();
    assert_eq!(workspace.runs(), 2);
    assert_eq!(
        hit.per_action_outcomes.last().unwrap().cache_state,
        EvidenceCacheState::Miss
    );
    let error = workspace
        .build_with(cached_graph(true), &Producers::default(), None)
        .await
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("fingerprint-only output"),
        "{error:#}"
    );
    assert_eq!(workspace.runs(), 2);
    assert!(!workspace.path().join(PRODUCED).exists());
}

#[tokio::test]
async fn refused_cached_consumer_is_never_executed_or_published() {
    let workspace = Workspace::new();
    let producers = Producers::default();
    let error = workspace
        .build_with(cached_graph(true), &producers, None)
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("fingerprint-only output"));
    assert_eq!(workspace.runs(), 0);
    assert!(lock(&producers.inner.by_output).is_empty());
    let outcome = workspace
        .build_with(cached_graph(false), &producers, None)
        .await
        .unwrap();
    assert_eq!(
        outcome.per_action_outcomes.last().unwrap().cache_state,
        EvidenceCacheState::Miss
    );
    assert_eq!(workspace.read(".struo/out/consumed.txt"), "original");
}
