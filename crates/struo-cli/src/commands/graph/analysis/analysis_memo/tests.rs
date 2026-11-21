use serde_json::json;
use struo_frontend::analysis::{AnalysisObservations, AnalysisResult, TransitiveSets};
use tempfile::TempDir;

use super::AnalysisMemo;

fn analysis(observations: AnalysisObservations) -> AnalysisResult {
    AnalysisResult {
        actions: Vec::new(),
        provider: json!({"binary": "app"}),
        declared_outputs: vec![".struo/out/app/app".to_string()],
        observations,
    }
}

fn recorded(entries: Vec<struo_frontend::analysis::Observation>) -> AnalysisObservations {
    let mut observations = AnalysisObservations::default();
    for entry in entries {
        observations.push_for_test(entry);
    }
    observations
}

#[test]
fn executable_identity_is_optional_and_independent_of_memoization_policy() {
    let workspace = TempDir::new().unwrap();
    let mut memo = AnalysisMemo::open(workspace.path(), TransitiveSets::default());
    memo.executable_identity = Some("binary-identity".to_owned());
    memo.enabled = false;
    assert!(!memo.is_enabled());
    assert_eq!(memo.executable_identity(), Some("binary-identity"));
    memo.executable_identity = None;
    assert_eq!(memo.executable_identity(), None);
}

#[cfg(unix)]
#[test]
fn a_stored_analysis_comes_back_with_the_answers_it_recorded() {
    let workspace = TempDir::new().unwrap();
    let memo = AnalysisMemo::open(workspace.path(), TransitiveSets::default());
    let observations = recorded(vec![struo_frontend::analysis::Observation::Env {
        name: "SDKROOT".to_string(),
        value: Some("/sdk".to_string()),
    }]);

    memo.write("abcdef", &analysis(observations.clone()));
    let (result, read_back) = memo.read("abcdef").expect("a record was written");

    assert_eq!(
        result.declared_outputs,
        vec![".struo/out/app/app".to_string()]
    );
    assert_eq!(result.provider, json!({"binary": "app"}));
    assert_eq!(read_back, observations);
    assert_eq!(result.observations, observations);
    memo.write("abcdef", &result);
    assert_eq!(memo.read("abcdef").unwrap().1, observations);
}

#[test]
fn damaged_provider_descendants_invalidate_actionless_memos_and_can_be_repaired() {
    use std::collections::BTreeMap;

    use struo_frontend::analysis::AnalysisEngine;
    use struo_frontend::Target;

    let workspace = TempDir::new().unwrap();
    let source = r#"
def _impl(ctx):
    leaf = transitive_set(direct = ["leaf"])
    return {"set": transitive_set(direct = ["root"], transitive = [leaf])}
custom = {"_struo_target_kind": True, "kind": "custom", "impl": _impl}
"#;
    let target = struo_frontend::graph_from_targets(&[Target {
        package: String::new(),
        kind: "custom".to_owned(),
        name: "producer".to_owned(),
        deps: Vec::new(),
        dependency_edges: BTreeMap::new(),
        srcs: Vec::new(),
        visibility: Vec::new(),
        attrs: BTreeMap::new(),
        typed_attrs: BTreeMap::new(),
        resolver_input_exclude: Vec::new(),
    }])
    .remove(0);
    let open = || {
        let mut memo = AnalysisMemo::open(workspace.path(), TransitiveSets::default());
        memo.enabled = true;
        memo
    };
    for corrupt in [false, true] {
        let result = AnalysisEngine::from_source(source)
            .unwrap()
            .analyze_target(&target, workspace.path(), &[])
            .unwrap();
        assert!(result.actions.is_empty());
        open().write("abcdef", &result);
        assert!(open().read("abcdef").is_some());
        let hex = result.provider["set"]["$struo_transitive_set"]
            .as_str()
            .unwrap();
        let root = workspace
            .path()
            .join(".struo/transitive-sets")
            .join(&hex[..2])
            .join(format!("{hex}.json"));
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&root).unwrap()).unwrap();
        let child = stored["children"][0].as_str().unwrap();
        let child = workspace
            .path()
            .join(".struo/transitive-sets")
            .join(&child[..2])
            .join(format!("{child}.json"));
        if corrupt {
            std::fs::write(&child, b"{}").unwrap();
        } else {
            std::fs::remove_file(&child).unwrap();
        }
        assert!(root.is_file());
        assert!(open().read("abcdef").is_none());
        AnalysisEngine::from_source(source)
            .unwrap()
            .analyze_target(&target, workspace.path(), &[])
            .unwrap();
        assert!(open().read("abcdef").is_some());
    }
}

#[test]
fn an_analysis_that_read_something_undescribable_is_not_stored() {
    let workspace = TempDir::new().unwrap();
    let memo = AnalysisMemo::open(workspace.path(), TransitiveSets::default());
    let mut observations = AnalysisObservations::default();
    observations.mark_incomplete_for_test();

    memo.write("abcdef", &analysis(observations));

    assert!(
        memo.read("abcdef").is_none(),
        "an analysis whose answers cannot be checked must never be stored"
    );
}

#[cfg(unix)]
#[test]
fn a_record_written_by_another_schema_is_ignored() {
    let workspace = TempDir::new().unwrap();
    let memo = AnalysisMemo::open(workspace.path(), TransitiveSets::default());
    memo.write("abcdef", &analysis(AnalysisObservations::default()));
    let path = memo.path("abcdef");
    let raw = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        raw.replace(super::SCHEMA, "struo.analysis-memo.from-the-future"),
    )
    .unwrap();

    assert!(memo.read("abcdef").is_none());
}

#[cfg(unix)]
#[test]
fn records_are_kept_apart_by_name() {
    let workspace = TempDir::new().unwrap();
    let memo = AnalysisMemo::open(workspace.path(), TransitiveSets::default());
    let mut other = analysis(AnalysisObservations::default());
    other.declared_outputs = vec![".struo/out/other/other".to_string()];

    memo.write("aaaaaa", &analysis(AnalysisObservations::default()));
    memo.write("bbbbbb", &other);

    assert_eq!(
        memo.read("aaaaaa").unwrap().0.declared_outputs,
        vec![".struo/out/app/app".to_string()]
    );
    assert_eq!(
        memo.read("bbbbbb").unwrap().0.declared_outputs,
        vec![".struo/out/other/other".to_string()]
    );
}
