use super::*;

#[test]
fn unresolved_bootstrap_file_lists_allow_import_but_block_execution() {
    let source = format!(
        r#"{}
def workspace_root():
    return "/workspace"
phase = {{"isa": "PBXShellScriptBuildPhase", "shellScript": "generate", "inputFileListPaths": ["${{PODS_ROOT}}/inputs.xcfilelist"]}}
phases = _xcode_shell_script_phases({{"label": {{"package": "", "id": "Seed"}}, "attr": {{"project": "App.xcodeproj"}}}}, {{"phase": phase}}, {{"buildPhases": ["phase"]}}, {{}}, "", "App")
action = json_decode(phases["actions"][0])
result = repr([action["cacheable"], action["unresolved_file_lists"]])
"#,
        xcode_prelude_source()
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[False, ["${PODS_ROOT}/inputs.xcfilelist"]]"#
    );
    let source = format!(
        r#"{}
_apple_run_prebuild_actions({{"capability": "build", "label": {{"id": "App"}}, "attr": {{}}, "build_dir": ".once/out/App"}}, {{"prebuild_actions": [_json_encode({{"unresolved_file_lists": ["${{PODS_ROOT}}/inputs.xcfilelist"]}})]}})
result = repr(True)
"#,
        apple_prelude_source()
    );
    assert!(eval_prelude_source_to_repr(source)
        .unwrap_err()
        .contains("Prepare the project's dependencies"));
}

#[test]
fn scripts_run_before_the_native_phases_that_consume_their_outputs() {
    let source = format!(
        r#"{}
def workspace_root():
    return "/workspace"
objects = {{
    "sources": {{"isa": "PBXSourcesBuildPhase"}},
    "link-generator": {{"isa": "PBXShellScriptBuildPhase", "shellScript": "generate", "outputPaths": ["$(TARGET_BUILD_DIR)/Generated.framework", "$(TARGET_BUILD_DIR)/tool-without-extension"]}},
    "frameworks": {{"isa": "PBXFrameworksBuildPhase"}},
    "resource-generator": {{"isa": "PBXShellScriptBuildPhase", "shellScript": "generate", "outputPaths": ["$(TARGET_BUILD_DIR)/Generated.bundle"]}},
    "resources": {{"isa": "PBXResourcesBuildPhase"}},
    "after": {{"isa": "PBXShellScriptBuildPhase", "shellScript": "verify"}},
}}
phases = _xcode_shell_script_phases(
    {{"label": {{"package": "", "id": "Seed"}}, "attr": {{"project": "App.xcodeproj"}}}},
    objects, {{"buildPhases": ["sources", "link-generator", "frameworks", "resource-generator", "resources", "after"]}}, {{}}, "", "App",
)
result = repr([[json_decode(action)["id"] for action in phases[key]] for key in ["actions", "prepackage_actions", "postbuild_actions"]])
"#,
        xcode_prelude_source()
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["link-generator"], ["resource-generator"], ["after"]]"#
    );
}

#[test]
fn generated_asset_catalogs_precede_compiler_asset_accessors() {
    let source = format!(
        r#"{}
def workspace_root():
    return "/workspace"
objects = {{"sources": {{"isa": "PBXSourcesBuildPhase"}}, "generate": {{"isa": "PBXShellScriptBuildPhase", "shellScript": "generate", "outputPaths": ["$(TARGET_BUILD_DIR)/Generated.xcassets"]}}, "resources": {{"isa": "PBXResourcesBuildPhase"}}}}
phases = _xcode_shell_script_phases({{"label": {{"package": "", "id": "Seed"}}, "attr": {{"project": "App.xcodeproj"}}}}, objects, {{"buildPhases": ["sources", "generate", "resources"]}}, {{}}, "", "App")
result = repr([len(phases["actions"]), len(phases["prepackage_actions"]), len(phases["postbuild_actions"])])
"#,
        xcode_prelude_source()
    );
    assert_eq!(eval_prelude_source_to_repr(source).unwrap(), "[1, 0, 0]");
}

#[test]
fn later_cached_scripts_consume_the_effective_bundle_tree() {
    let workspace = TempDir::new().unwrap();
    let source = format!(
        r#"{}
ctx = {{"label": {{"id": "App"}}, "attr": {{}}, "build_dir": ".once/out/App"}}
_apple_run_postbuild_actions(ctx, {{"postbuild_actions": [
    _json_encode({{"id": "first", "shell": "/bin/sh", "script": "write-new-resource", "cacheable": False}}),
    _json_encode({{"id": "second", "shell": "/bin/sh", "script": "read-new-resource", "outputs": [".once/out/App/App.app/observed.txt"], "cacheable": True}}),
]}}, [".once/out/App/App.app/App"], ".once/out/App/App.app")
result = repr(True)
"#,
        apple_prelude_source()
    );
    let (store, result) = with_active_store(store_for(workspace.path(), ""), || {
        eval_prelude_source_to_repr(source)
    });
    result.unwrap();
    let second = action_by_identifier(&store, "postbuild_action:App:second");
    assert!(second.cacheable);
    assert_eq!(second.sandbox.as_deref(), Some("off"));
    assert!(second.inputs.contains(&".once/out/App/App.app".to_string()));
}

#[test]
fn generated_bundles_are_declared_trees_before_their_contents_exist() {
    let workspace = TempDir::new().unwrap();
    let source = format!(
        r#"{}
ctx = {{"label": {{"id": "App", "package": ""}}, "build_dir": ".once/out/config/App", "attr": {{"prepackage_actions": [_json_encode({{"build_dir": ".once/out/App", "outputs": [".once/out/App/Generated.bundle"]}})]}}}}
result = repr(_apple_materialize_resources(ctx, [".once/out/App/Generated.bundle"], "App.app", "macos", "13.0", "", "App", "resource"))
"#,
        apple_prelude_source()
    );
    let (store, result) = with_active_store(store_for(workspace.path(), ""), || {
        eval_prelude_source_to_repr(source)
    });
    result.unwrap();
    let action = action_by_identifier(&store, "resource_generated_tree_Generated.bundle");
    assert!(action
        .inputs
        .contains(&".once/out/config/App/Generated.bundle".to_string()));
    assert!(matches!(
        &action.operation,
        Some(DeclaredActionOperation::CopyPath {
            mode: DeclaredCopyPathMode::Tree,
            ..
        })
    ));
}
