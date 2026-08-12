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

    if ctx["attr"].get("read_dependency_output"):
        run_action(
            argv = ["/bin/sh", "-c", "cat \"$1\" > \"$2\"", "sh", ctx["deps"][0]["out"], out],
            inputs = [ctx["deps"][0]["out"]],
            outputs = [out],
            identifier = ctx["label"]["name"] + "-dependency-output",
        )
    elif ctx["capability"] == "test":
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
    return {
        "target": ctx["label"]["name"],
        "out": out,
        "default_deps": [dep["target"] for dep in ctx["deps"]],
        "plugin_deps": [dep["target"] for dep in ctx["deps_by_role"].get("plugins") or []],
    }

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
        dependency_edges: BTreeMap::new(),
        srcs: srcs.iter().map(|src| (*src).to_string()).collect(),
        visibility: Vec::new(),
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
        tools: Vec::new(),
        diagnostics: Vec::new(),
    }
}

#[test]
fn graph_targets_with_loading_diagnostics_cannot_execute() {
    let mut target = target_with_capabilities("Unavailable", &[], &[], &["build"], []);
    target.diagnostics.push(
        once_frontend::Diagnostic::new(
            "required_tool_not_found",
            "`example-tool` not found on PATH",
        )
        .with_target("Unavailable")
        .with_repair("Install the required tool"),
    );

    let error = ensure_graph_target_valid(&target).unwrap_err();
    let failure = error
        .downcast_ref::<once_frontend::analysis::AnalysisFailure>()
        .expect("structured graph loading failure");

    assert_eq!(failure.diagnostic.code, "required_tool_not_found");
    assert_eq!(failure.diagnostic.target.as_deref(), Some("Unavailable"));
}

#[tokio::test]
async fn graph_tool_resolution_defers_to_host_path_without_mise_config() {
    let workspace = tempfile::tempdir().unwrap();
    let mut target = target_with_capabilities("Tool", &[], &[], &["build"], []);
    target.tools.push(once_frontend::ToolRequirement {
        name: "rust".to_string(),
        executables: vec!["rustc".to_string(), "cargo".to_string()],
    });

    let paths = resolve_graph_tools(workspace.path(), &[target])
        .await
        .unwrap();

    // Without a mise config the workspace relies on the host toolchain.
    // Returning no resolved paths keeps `host_which` walking `PATH` (and
    // verifying existence) rather than short-circuiting to a bare name.
    assert!(paths.paths.is_empty());
}

#[test]
fn graph_tool_cache_reuses_existing_paths_for_unchanged_configuration() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(
        workspace.path().join("mise.toml"),
        "[tools]\nnode = \"26\"\n",
    )
    .unwrap();
    let executable = workspace.path().join("tools/node");
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    std::fs::write(&executable, b"node").unwrap();
    let tools = vec!["node".to_string()];
    let executables = vec!["node".to_string()];
    let fingerprint = graph_tool_cache_fingerprint(workspace.path(), &tools, &executables).unwrap();
    let paths = BTreeMap::from([("node".to_string(), executable.display().to_string())]);
    let commands = vec![CachedToolCommand {
        argv: vec!["node".to_string(), "--version".to_string()],
        env: BTreeMap::new(),
        cwd: None,
        merge_stderr: false,
        output: "v26".to_string(),
    }];

    let cache_path = workspace.path().join("tool-cache.json");
    write_graph_tool_cache_at(&cache_path, fingerprint, &paths, &commands).unwrap();

    let cached = read_graph_tool_cache_at(&cache_path, fingerprint).unwrap();
    assert_eq!(cached.paths, paths);
    assert_eq!(cached.commands, commands);
}

#[test]
fn graph_tool_cache_invalidates_configuration_and_missing_executables() {
    let workspace = tempfile::tempdir().unwrap();
    let config = workspace.path().join("mise.toml");
    std::fs::write(&config, "[tools]\nnode = \"25\"\n").unwrap();
    let executable = workspace.path().join("tools/node");
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    std::fs::write(&executable, b"node").unwrap();
    let tools = vec!["node".to_string()];
    let executables = vec!["node".to_string()];
    let first = graph_tool_cache_fingerprint(workspace.path(), &tools, &executables).unwrap();
    let paths = BTreeMap::from([("node".to_string(), executable.display().to_string())]);
    let cache_path = workspace.path().join("tool-cache.json");
    write_graph_tool_cache_at(&cache_path, first, &paths, &[]).unwrap();

    std::fs::write(&config, "[tools]\nnode = \"26\"\n").unwrap();
    let second = graph_tool_cache_fingerprint(workspace.path(), &tools, &executables).unwrap();
    assert_ne!(first, second);
    assert!(read_graph_tool_cache_at(&cache_path, second).is_none());

    write_graph_tool_cache_at(&cache_path, second, &paths, &[]).unwrap();
    std::fs::write(executable, b"changed node binary").unwrap();
    assert!(read_graph_tool_cache_at(&cache_path, second).is_none());
}

#[test]
fn graph_tool_cache_path_is_host_cached_and_workspace_scoped() {
    let cache_home = Path::new("/cache/once/toolchains");
    let first = graph_tool_cache_path_from(cache_home, Path::new("/workspaces/first"));
    let second = graph_tool_cache_path_from(cache_home, Path::new("/workspaces/second"));

    assert!(first.starts_with(cache_home));
    assert_eq!(
        first.extension().and_then(|value| value.to_str()),
        Some("json")
    );
    assert_ne!(first, second);
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
    let session = BuildSession::new_with_analyzer(
        workspace.path(),
        &cache,
        graph.clone(),
        analyzer,
        SandboxMode::default(),
    );

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
    let session = BuildSession::new_with_analyzer(
        workspace.path(),
        &cache,
        graph.clone(),
        analyzer,
        SandboxMode::default(),
    );

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
    let session = BuildSession::new_with_analyzer(
        workspace.path(),
        &cache,
        graph.clone(),
        analyzer,
        SandboxMode::default(),
    );

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
async fn cached_dependency_outputs_are_materialized_before_dependents_run() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(workspace.path().join(".once/cache"));
    let graph = vec![
        target_with_capabilities("Root", &["ConsumerA", "ConsumerB"], &[], &["build"], []),
        target_with_capabilities(
            "ConsumerA",
            &["Dependency"],
            &[],
            &["build"],
            [("read_dependency_output".to_string(), AttrValue::Bool(true))],
        ),
        target_with_capabilities(
            "ConsumerB",
            &["Dependency"],
            &[],
            &["build"],
            [("read_dependency_output".to_string(), AttrValue::Bool(true))],
        ),
        test_target("Dependency", &[], "printf dependency > \"$1\""),
    ];
    let analyzer = AnalysisEngine::from_source(GRAPH_TEST_PRELUDE).unwrap();
    let session = BuildSession::new_with_analyzer(
        workspace.path(),
        &cache,
        graph.clone(),
        analyzer,
        SandboxMode::default(),
    );

    session
        .build_with_analysis(&graph[3])
        .await
        .unwrap()
        .unwrap();
    std::fs::remove_file(
        workspace
            .path()
            .join(".once/out/Dependency/Dependency-build.txt"),
    )
    .unwrap();

    session
        .build_with_analysis(&graph[0])
        .await
        .unwrap()
        .unwrap();

    for consumer in ["ConsumerA", "ConsumerB"] {
        assert_eq!(
            std::fs::read_to_string(
                workspace
                    .path()
                    .join(format!(".once/out/{consumer}/{consumer}-build.txt"))
            )
            .unwrap(),
            "dependency"
        );
    }
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
    let session = BuildSession::new_with_analyzer(
        workspace.path(),
        &cache,
        graph.clone(),
        analyzer,
        SandboxMode::default(),
    );

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
    let session = BuildSession::new_with_analyzer(
        workspace.path(),
        &cache,
        graph.clone(),
        analyzer,
        SandboxMode::default(),
    );

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
async fn run_arguments_do_not_invalidate_dependency_build_actions() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(workspace.path().join(".once/cache"));
    let graph = vec![
        target_with_capabilities("Root", &["Dep"], &[], &["run"], []),
        test_target("Dep", &[], "printf dependency > \"$1\""),
    ];
    let first_analyzer = AnalysisEngine::from_source(GRAPH_TEST_PRELUDE).unwrap();
    let first_session = BuildSession::new_with_analyzer(
        workspace.path(),
        &cache,
        graph.clone(),
        first_analyzer,
        SandboxMode::default(),
    );
    let first = first_session
        .build_with_analysis(&graph[1])
        .await
        .unwrap()
        .unwrap();

    let run_analyzer = AnalysisEngine::from_source_with_options(
        GRAPH_TEST_PRELUDE,
        AnalysisOptions {
            run_arguments: vec!["serve".to_string()],
            ..AnalysisOptions::default()
        },
    )
    .unwrap();
    let run_session = BuildSession::new_with_analyzer(
        workspace.path(),
        &cache,
        graph.clone(),
        run_analyzer,
        SandboxMode::default(),
    );
    let dependencies = run_session
        .build_direct_analysis_deps(&graph[0])
        .await
        .unwrap();

    assert_eq!(dependencies[0].1.cache_tag, "hit");
    assert_eq!(dependencies[0].1.action_digest, first.action_digest);
}

#[tokio::test]
async fn capability_options_preserve_the_build_analysis_module_identity() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(
        workspace.path().join("module.star"),
        r#"
def _impl(ctx):
    return {"target": ctx["label"]["id"]}

custom = target_kind(
    kind = "custom",
    capabilities = [
        capability("build", []),
        capability("run", []),
    ],
    impl = _impl,
)
"#,
    )
    .unwrap();
    std::fs::write(
        workspace.path().join("once.toml"),
        r#"
[modules]
paths = ["module.star"]

[[target]]
name = "root"
kind = "custom"
"#,
    )
    .unwrap();
    let cache = CacheProvider::open_local(workspace.path().join(".once/cache"));
    let build_session =
        BuildSession::load_workspace(workspace.path(), &cache, SandboxMode::default())
            .await
            .unwrap();
    let graph = once_frontend::load_graph_workspace(workspace.path()).unwrap();
    let run_session = BuildSession::new_with_options(
        workspace.path(),
        &cache,
        graph,
        AnalysisOptions {
            run_arguments: vec!["serve".to_string()],
            ..AnalysisOptions::default()
        },
        SandboxMode::default(),
    )
    .await
    .unwrap();

    assert_eq!(
        run_session.module_source_digest,
        build_session.module_source_digest
    );
}

#[tokio::test]
async fn named_dependency_roles_reach_starlark_in_declared_order() {
    let workspace = tempfile::tempdir().unwrap();
    let cache = CacheProvider::open_local(workspace.path().join(".once/cache"));
    let mut root = target_with_capabilities("Root", &["Library"], &[], &["build"], []);
    root.dependency_edges.insert(
        "plugins".to_string(),
        vec!["PluginB".to_string(), "PluginA".to_string()],
    );
    let graph = vec![
        root,
        target_with_capabilities("Library", &[], &[], &["build"], []),
        target_with_capabilities("PluginA", &[], &[], &["build"], []),
        target_with_capabilities("PluginB", &[], &[], &["build"], []),
    ];
    let analyzer = AnalysisEngine::from_source(GRAPH_TEST_PRELUDE).unwrap();
    let session = BuildSession::new_with_analyzer(
        workspace.path(),
        &cache,
        graph.clone(),
        analyzer,
        SandboxMode::default(),
    );

    let outcome = session
        .build_with_analysis(&graph[0])
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        outcome.provider["default_deps"],
        serde_json::json!(["Library"])
    );
    assert_eq!(
        outcome.provider["plugin_deps"],
        serde_json::json!(["PluginB", "PluginA"])
    );
}

#[cfg(unix)]
#[tokio::test]
async fn capability_runs_are_salted_by_dependency_action_digests() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("dep.txt"), b"one").unwrap();
    let cache = CacheProvider::open_local(workspace.path().join(".once/cache"));
    let graph = vec![
        target_with_capabilities("Dep", &[], &["dep.txt"], &["build"], []),
        target_with_capabilities(
            "Root",
            &["Dep"],
            &[],
            &["test"],
            [("read_dependency_output".to_string(), AttrValue::Bool(true))],
        ),
    ];
    let analyzer = AnalysisEngine::from_source(GRAPH_TEST_PRELUDE).unwrap();

    let session = BuildSession::new_with_analyzer(
        workspace.path(),
        &cache,
        graph.clone(),
        analyzer,
        SandboxMode::default(),
    );
    let first = session
        .run_with_analysis(&graph[1], "test")
        .await
        .unwrap()
        .unwrap();

    std::fs::write(workspace.path().join("dep.txt"), b"two").unwrap();
    let analyzer = AnalysisEngine::from_source(GRAPH_TEST_PRELUDE).unwrap();
    let session = BuildSession::new_with_analyzer(
        workspace.path(),
        &cache,
        graph.clone(),
        analyzer,
        SandboxMode::default(),
    );
    let second = session
        .run_with_analysis(&graph[1], "test")
        .await
        .unwrap()
        .unwrap();

    assert_ne!(first.action_digest, second.action_digest);
}
