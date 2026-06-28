use super::*;

use once_frontend::{AttrValue, Capability, TargetLabel};

static GRAPH_TEST_PRELUDE: &str = r#"
def target_kind(kind = None, impl = None):
    return {"_once_target_kind": True, "kind": kind, "impl": impl}

def _impl(ctx):
    out = declare_output(ctx["label"]["name"] + "-" + ctx["capability"] + ".txt")
    srcs = glob(ctx["srcs"])
    if "script" in ctx["attr"]:
        run_action(
            argv = ["/bin/sh", "-c", ctx["attr"]["script"], "sh", out],
            inputs = srcs,
            outputs = [out],
            cacheable = not ("uncacheable" in ctx["attr"]),
            identifier = ctx["label"]["name"] + "-" + ctx["capability"],
        )
        return {"target": ctx["label"]["name"], "out": out}

    if ctx["capability"] == "test":
        run_action(
            argv = ["/bin/sh", "-c", "printf test > \"$1\"", "sh", out],
            outputs = [out],
            identifier = ctx["label"]["name"] + "-test",
        )
    elif len(srcs) > 0:
        run_action(
            argv = ["/bin/sh", "-c", "cat \"$1\" > \"$2\"", "sh", srcs[0], out],
            inputs = srcs,
            outputs = [out],
            identifier = ctx["label"]["name"] + "-build",
        )
    else:
        run_action(
            argv = ["/bin/sh", "-c", "printf " + ctx["label"]["name"] + " > \"$1\"", "sh", out],
            outputs = [out],
            identifier = ctx["label"]["name"] + "-build",
        )
    return {"target": ctx["label"]["name"], "out": out}

test_kind = target_kind(impl = _impl)
metadata_kind = target_kind()
"#;

fn test_target(name: &str, deps: &[&str], script: impl Into<String>) -> GraphTarget {
    target_with_capabilities(
        name,
        deps,
        &[],
        &["build"],
        [("script".to_string(), AttrValue::String(script.into()))],
    )
}

fn target_of_kind(
    kind: &str,
    name: &str,
    deps: &[&str],
    srcs: &[&str],
    capabilities: &[&str],
    attrs: impl IntoIterator<Item = (String, AttrValue)>,
) -> GraphTarget {
    let mut target = target_with_capabilities(name, deps, srcs, capabilities, attrs);
    target.kind = kind.to_string();
    target
}

fn target_with_capabilities(
    name: &str,
    deps: &[&str],
    srcs: &[&str],
    capabilities: &[&str],
    attrs: impl IntoIterator<Item = (String, AttrValue)>,
) -> GraphTarget {
    GraphTarget {
        label: TargetLabel {
            package: String::new(),
            name: name.to_string(),
            id: name.to_string(),
        },
        kind: "test_kind".to_string(),
        deps: deps.iter().map(|dep| (*dep).to_string()).collect(),
        srcs: srcs.iter().map(|src| (*src).to_string()).collect(),
        attrs: attrs.into_iter().collect(),
        capabilities: capabilities
            .iter()
            .map(|capability| Capability {
                name: (*capability).to_string(),
                output_groups: Vec::new(),
                requires_outputs: Vec::new(),
            })
            .collect(),
        providers: Vec::new(),
        diagnostics: Vec::new(),
    }
}

#[test]
fn reachable_analysis_deps_walks_only_analysis_backed_direct_deps() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(workspace.path().join(".once/cache"));
    let graph = vec![
        target_with_capabilities(
            "Root",
            &["DirectAnalysis", "DirectMetadata"],
            &[],
            &["test"],
            [],
        ),
        target_with_capabilities(
            "DirectAnalysis",
            &["TransitiveAnalysis"],
            &[],
            &["build"],
            [],
        ),
        target_with_capabilities("TransitiveAnalysis", &[], &[], &["build"], []),
        target_of_kind(
            "metadata_kind",
            "DirectMetadata",
            &["HiddenAnalysis"],
            &[],
            &["build"],
            [],
        ),
        target_with_capabilities("HiddenAnalysis", &[], &[], &["build"], []),
    ];
    let analyzer = AnalysisEngine::from_source(GRAPH_TEST_PRELUDE).unwrap();
    let session =
        BuildSession::new_with_analyzer(workspace.path(), &cache, graph.clone(), analyzer);

    let reachable = session.reachable_analysis_deps(&graph[0]);

    assert!(reachable.contains("DirectAnalysis"));
    assert!(reachable.contains("TransitiveAnalysis"));
    assert!(!reachable.contains("DirectMetadata"));
    assert!(!reachable.contains("HiddenAnalysis"));
}

#[tokio::test]
async fn run_with_analysis_returns_none_for_target_kinds_without_implementation() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(workspace.path().join(".once/cache"));
    let graph = vec![
        target_of_kind("metadata_kind", "Root", &["Dep"], &[], &["test"], []),
        target_with_capabilities("Dep", &[], &[], &["build"], []),
    ];
    let analyzer = AnalysisEngine::from_source(GRAPH_TEST_PRELUDE).unwrap();
    let session =
        BuildSession::new_with_analyzer(workspace.path(), &cache, graph.clone(), analyzer);

    let outcome = session.run_with_analysis(&graph[0], "test").await.unwrap();

    assert!(outcome.is_none());
    assert!(!workspace.path().join(".once/out/Dep").exists());
}

#[cfg(unix)]
fn parallel_leaf_script(marker: &str, peer: &str, output: &str) -> String {
    format!(
        r#"mkdir -p sync
: > sync/{marker}
i=0
while [ ! -f sync/{peer} ]; do
  i=$((i + 1))
  [ "$i" -le 50 ] || exit 42
  sleep 0.1
done
printf {output} > "$1"
"#
    )
}

#[cfg(unix)]
#[tokio::test]
async fn independent_dependencies_run_in_parallel() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(workspace.path().join(".once/cache"));
    let graph = vec![
        test_target("Root", &["LeafA", "LeafB"], "printf root > \"$1\""),
        test_target("LeafA", &[], parallel_leaf_script("LeafA", "LeafB", "a")),
        test_target("LeafB", &[], parallel_leaf_script("LeafB", "LeafA", "b")),
    ];
    let analyzer = AnalysisEngine::from_source(GRAPH_TEST_PRELUDE).unwrap();
    let session =
        BuildSession::new_with_analyzer(workspace.path(), &cache, graph.clone(), analyzer);

    let outcome = session
        .build_with_analysis(&graph[0])
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        outcome.outputs,
        vec![".once/out/Root/Root-build.txt".to_string()]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn uncacheable_declared_actions_bypass_action_cache() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(workspace.path().join(".once/cache"));
    let graph = vec![target_with_capabilities(
        "Root",
        &[],
        &[],
        &["build"],
        [
            (
                "script".to_string(),
                AttrValue::String("printf x >> side_effect; printf run > \"$1\"".to_string()),
            ),
            ("uncacheable".to_string(), AttrValue::Bool(true)),
        ],
    )];
    let analyzer = AnalysisEngine::from_source(GRAPH_TEST_PRELUDE).unwrap();
    let session =
        BuildSession::new_with_analyzer(workspace.path(), &cache, graph.clone(), analyzer);

    let first = session
        .build_with_analysis(&graph[0])
        .await
        .unwrap()
        .unwrap();
    let second = session
        .build_with_analysis(&graph[0])
        .await
        .unwrap()
        .unwrap();

    assert_eq!(first.cache_tag, "bypass");
    assert_eq!(second.cache_tag, "bypass");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("side_effect")).unwrap(),
        "xx"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn build_direct_analysis_deps_returns_only_direct_deps_in_declared_order() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(workspace.path().join(".once/cache"));
    let graph = vec![
        target_with_capabilities("Root", &["Second", "Metadata", "First"], &[], &["test"], []),
        target_with_capabilities("Second", &["Shared"], &[], &["build"], []),
        target_of_kind("metadata_kind", "Metadata", &[], &[], &["build"], []),
        target_with_capabilities("First", &["Shared"], &[], &["build"], []),
        target_with_capabilities("Shared", &[], &[], &["build"], []),
    ];
    let analyzer = AnalysisEngine::from_source(GRAPH_TEST_PRELUDE).unwrap();
    let session =
        BuildSession::new_with_analyzer(workspace.path(), &cache, graph.clone(), analyzer);

    let outcomes = session.build_direct_analysis_deps(&graph[0]).await.unwrap();
    let outcome_ids = outcomes
        .iter()
        .map(|(target_id, _)| target_id.as_str())
        .collect::<Vec<_>>();
    let provider_targets = outcomes
        .iter()
        .map(|(_, outcome)| outcome.provider["target"].as_str().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(outcome_ids, vec!["Second", "First"]);
    assert_eq!(provider_targets, vec!["Second", "First"]);
    assert_eq!(
        outcomes[0].1.outputs,
        vec![".once/out/Second/Second-build.txt".to_string()]
    );
    assert!(workspace
        .path()
        .join(".once/out/Shared/Shared-build.txt")
        .is_file());
    assert!(!workspace.path().join(".once/out/Metadata").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn capability_runs_are_salted_by_dependency_action_digests() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("dep.txt"), b"one").unwrap();
    let cache = CacheProvider::open_local(workspace.path().join(".once/cache"));
    let graph = vec![
        target_with_capabilities("Dep", &[], &["dep.txt"], &["build"], []),
        target_with_capabilities("Root", &["Dep"], &[], &["test"], []),
    ];
    let analyzer = AnalysisEngine::from_source(GRAPH_TEST_PRELUDE).unwrap();

    let session =
        BuildSession::new_with_analyzer(workspace.path(), &cache, graph.clone(), analyzer);
    let first = session
        .run_with_analysis(&graph[1], "test")
        .await
        .unwrap()
        .unwrap();

    std::fs::write(workspace.path().join("dep.txt"), b"two").unwrap();
    let analyzer = AnalysisEngine::from_source(GRAPH_TEST_PRELUDE).unwrap();
    let session =
        BuildSession::new_with_analyzer(workspace.path(), &cache, graph.clone(), analyzer);
    let second = session
        .run_with_analysis(&graph[1], "test")
        .await
        .unwrap()
        .unwrap();

    assert_ne!(first.action_digest, second.action_digest);
}
