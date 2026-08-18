use once_frontend::analysis::{AnalysisObservations, AnalysisResult};
use serde_json::json;
use tempfile::TempDir;

use super::AnalysisMemo;

fn analysis(observations: AnalysisObservations) -> AnalysisResult {
    AnalysisResult {
        actions: Vec::new(),
        provider: json!({"binary": "app"}),
        declared_outputs: vec![".once/out/app/app".to_string()],
        observations,
    }
}

fn recorded(entries: Vec<once_frontend::analysis::Observation>) -> AnalysisObservations {
    let mut observations = AnalysisObservations::default();
    for entry in entries {
        observations.push_for_test(entry);
    }
    observations
}

#[test]
fn a_stored_analysis_comes_back_with_the_answers_it_recorded() {
    let workspace = TempDir::new().unwrap();
    let memo = AnalysisMemo::open(workspace.path());
    let observations = recorded(vec![once_frontend::analysis::Observation::Env {
        name: "SDKROOT".to_string(),
        value: Some("/sdk".to_string()),
    }]);

    memo.write("abcdef", &analysis(observations.clone()));
    let (result, read_back) = memo.read("abcdef").expect("a record was written");

    assert_eq!(
        result.declared_outputs,
        vec![".once/out/app/app".to_string()]
    );
    assert_eq!(result.provider, json!({"binary": "app"}));
    assert_eq!(read_back, observations);
}

#[test]
fn an_analysis_that_read_something_undescribable_is_not_stored() {
    let workspace = TempDir::new().unwrap();
    let memo = AnalysisMemo::open(workspace.path());
    let mut observations = AnalysisObservations::default();
    observations.mark_incomplete_for_test();

    memo.write("abcdef", &analysis(observations));

    assert!(
        memo.read("abcdef").is_none(),
        "an analysis whose answers cannot be checked must never be stored"
    );
}

#[test]
fn a_record_written_by_another_schema_is_ignored() {
    let workspace = TempDir::new().unwrap();
    let memo = AnalysisMemo::open(workspace.path());
    memo.write("abcdef", &analysis(AnalysisObservations::default()));
    let path = memo.path("abcdef");
    let raw = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        raw.replace(super::SCHEMA, "once.analysis-memo.from-the-future"),
    )
    .unwrap();

    assert!(memo.read("abcdef").is_none());
}

#[test]
fn records_are_kept_apart_by_name() {
    let workspace = TempDir::new().unwrap();
    let memo = AnalysisMemo::open(workspace.path());
    let mut other = analysis(AnalysisObservations::default());
    other.declared_outputs = vec![".once/out/other/other".to_string()];

    memo.write("aaaaaa", &analysis(AnalysisObservations::default()));
    memo.write("bbbbbb", &other);

    assert_eq!(
        memo.read("aaaaaa").unwrap().0.declared_outputs,
        vec![".once/out/app/app".to_string()]
    );
    assert_eq!(
        memo.read("bbbbbb").unwrap().0.declared_outputs,
        vec![".once/out/other/other".to_string()]
    );
}
