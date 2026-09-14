use super::*;

fn expand(body: &str) -> anyhow::Result<AnalysisResult> {
    let engine = AnalysisEngine::from_source(format!(
        r#"
def plan(ctx):
{body}
def impl(ctx):
    expand_actions(implementation = "plan", inputs = ["scan.json"], outputs = ["result"], args = {{"value": "hello"}})
    return {{}}
custom = {{"_once_target_kind": True, "kind": "custom", "impl": impl}}
"#
    ))?;
    let workspace = TempDir::new()?;
    let target = target("custom");
    let analysis = engine.analyze_target(&target, workspace.path(), &[])?;
    engine.expand_actions(&target, workspace.path(), &analysis.actions[0])
}

#[test]
fn expansion_preserves_arguments_and_declared_outputs() {
    let result = expand("    write_path(ctx[\"outputs\"][0], ctx[\"args\"][\"value\"])").unwrap();
    assert_eq!(result.actions.len(), 1);
    assert_eq!(result.actions[0].outputs, ["result"]);
    assert!(matches!(
        result.actions[0].operation,
        Some(DeclaredActionOperation::WriteFile { .. })
    ));
}

#[test]
fn expansion_rejects_missing_outputs_and_recursive_planners() {
    assert!(expand("    return None")
        .unwrap_err()
        .to_string()
        .contains("promised output"));
    assert!(expand("    expand_actions(implementation = \"plan\", inputs = [], outputs = [\"result\"], args = {})")
        .unwrap_err().to_string().contains("recursively"));
}

#[test]
fn expansion_preserves_structured_diagnostics() {
    let error = expand("    return {\"code\": \"missing_dependency\", \"message\": \"Declare a dependency\", \"target\": ctx[\"label\"][\"id\"], \"attribute\": \"deps\", \"repairs\": [\"Add dependency\"]}").unwrap_err();
    let diagnostic = &error.downcast_ref::<AnalysisFailure>().unwrap().diagnostic;
    assert_eq!(diagnostic.code, "missing_dependency");
    assert_eq!(diagnostic.target.as_deref(), Some("apps/ios/Sample"));
    assert_eq!(diagnostic.repairs, ["Add dependency"]);
}

#[test]
fn expansion_rejects_invalid_callback_results() {
    assert!(expand("    return [1]")
        .unwrap_err()
        .to_string()
        .contains("structured diagnostic"));
}

#[test]
fn expansion_rejects_function_arguments_instead_of_stringifying_them() {
    let workspace = TempDir::new().unwrap();
    let (_, result) = with_active_store(store_for(workspace.path(), "test"), || {
        run(r#"
def callback(ctx):
    pass
expand_actions(implementation = "callback", inputs = [], outputs = [], args = {"callback": callback})
"#)
    });
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("serializable data"));
}

#[test]
fn host_tree_identity_is_revalidated_between_analysis_sessions() {
    let workspace = TempDir::new().unwrap();
    let tree = TempDir::new().unwrap();
    std::fs::write(tree.path().join("header.h"), "first").unwrap();
    let source = format!(
        "value = host_tree_sha256({:?})",
        tree.path().to_str().unwrap()
    );
    let (_, first) =
        with_active_store(store_for(workspace.path(), "test"), || eval_string(&source));
    std::fs::write(tree.path().join("header.h"), "second revision").unwrap();
    let (_, second) =
        with_active_store(store_for(workspace.path(), "test"), || eval_string(&source));
    assert_ne!(first.unwrap(), second.unwrap());
}
