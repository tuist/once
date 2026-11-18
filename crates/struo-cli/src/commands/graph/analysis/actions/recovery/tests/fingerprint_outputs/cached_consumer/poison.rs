use super::*;

#[tokio::test]
async fn historical_poison_is_never_reused_even_with_intact_fingerprint_inputs() {
    for implicit in [false, true] {
        let workspace = Workspace::new();
        let mut graph = cached_graph(false);
        if implicit {
            let consumer = graph.actions.last_mut().unwrap();
            consumer.inputs.clear();
            consumer.depends_on_prior_actions = true;
        }
        let first = workspace
            .build_with(graph.clone(), &Producers::default(), None)
            .await
            .unwrap();
        let key = first.per_action_outcomes.last().unwrap().action_digest;
        let poison = workspace
            .cache
            .put_blob(b"struo.symlink_output.v1\nsource.txt\n")
            .await
            .unwrap();
        let poisoned = ActionResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            outputs: BTreeMap::from([(".struo/out/consumed.txt".to_string(), poison)]),
        };
        workspace
            .cache
            .put_action_result(&key, &poisoned)
            .await
            .unwrap();
        assert_eq!(
            workspace
                .cache
                .get_action_result(&key)
                .await
                .unwrap()
                .unwrap()
                .outputs[".struo/out/consumed.txt"],
            poison
        );
        let second = workspace
            .build_with(graph, &Producers::default(), None)
            .await
            .unwrap();
        assert_eq!(
            second.per_action_outcomes.last().unwrap().action_digest,
            key
        );
        assert_eq!(
            second.per_action_outcomes.last().unwrap().cache_state,
            EvidenceCacheState::Miss
        );
        assert_eq!(workspace.runs(), 2);
        assert_eq!(workspace.read(".struo/out/consumed.txt"), "original");
    }
}
