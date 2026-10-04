use super::*;

#[test]
fn preserves_outputless_phases_and_skips_install_only_phases() {
    let source = format!(
        r#"{}
def workspace_root():
    return "/workspace"
objects = {{
    "before": {{"isa": "PBXShellScriptBuildPhase", "shellScript": "verify", "name": "Repeated name"}},
    "sources": {{"isa": "PBXSourcesBuildPhase"}},
    "after": {{"isa": "PBXShellScriptBuildPhase", "shellScript": "finish", "name": "Repeated name"}},
    "install": {{"isa": "PBXShellScriptBuildPhase", "shellScript": "install", "runOnlyForDeploymentPostprocessing": "1"}},
}}
phases = _xcode_shell_script_phases(
    {{"label": {{"package": "", "id": "Seed"}}, "attr": {{"project": "App.xcodeproj"}}}},
    objects, {{"buildPhases": ["before", "sources", "after", "install"]}}, {{}}, "", "App",
)
result = repr([[json_decode(a)["id"], json_decode(a)["cacheable"]] for a in phases["actions"] + phases["postbuild_actions"]])
"#,
        xcode_prelude_source()
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["before", False], ["after", False]]"#
    );
}

#[test]
fn maps_script_paths_to_the_configured_action_directory() {
    let source = format!(
        r#"{}
def workspace_root():
    return "/workspace with spaces"
result = repr(_apple_reconcile_build_action(
    {{"build_dir": ".once/out/configured/App"}},
    {{"build_dir": ".once/out/App", "inputs": ["Sources/main.swift", ".once/out/App/before"], "outputs": [".once/out/App/App.app/extra"], "env": {{"TARGET_BUILD_DIR": "/workspace with spaces/.once/out/App", "CUSTOM": "/workspace with spaces/.once/out/App/Intermediates/data"}}}},
))
"#,
        apple_prelude_source()
    );
    let result = eval_prelude_source_to_repr(source).unwrap();
    assert!(result.contains(".once/out/configured/App/before"));
    assert!(result.contains(".once/out/configured/App/App.app/extra"));
    assert!(result.contains("/workspace with spaces/.once/out/configured/App/Intermediates/data"));
    assert!(result.contains("Sources/main.swift"));
}

#[test]
fn uncached_postbuild_scripts_publish_modified_product_bytes() {
    let workspace = TempDir::new().unwrap();
    let source = format!(
        r#"{}
ctx = {{"label": {{"id": "App"}}, "attr": {{}}, "build_dir": ".once/out/App"}}
_apple_run_postbuild_actions(ctx, {{"postbuild_actions": [_json_encode({{"id": "phase", "shell": "/bin/sh", "script": "modify-product", "cacheable": False}})]}}, [".once/out/App/App.app/Info.plist", ".once/out/App/App.app/App"])
result = repr(True)
"#,
        apple_prelude_source()
    );
    let (store, result) = with_active_store(store_for(workspace.path(), ""), || {
        eval_prelude_source_to_repr(source)
    });
    result.unwrap();
    let action = action_by_identifier(&store, "postbuild_action:App:phase");
    assert!(!action.cacheable);
    assert!(action.inherit_parent_env);
    assert!(action.depends_on_prior_actions);
    assert_eq!(action.inputs, action.outputs);
    assert_eq!(action.outputs.len(), 2);
}

#[test]
fn file_lists_remain_inputs_and_have_separate_environment_counts() {
    let source = format!(
        r#"{}
def workspace_root():
    return "/workspace"
def host_file_exists(path):
    return True
def host_file_read(path):
    return "$(SRCROOT)/input.txt\n" if path.endswith("inputs.xcfilelist") else "$(TARGET_BUILD_DIR)/result.txt\n"
phase = {{"isa": "PBXShellScriptBuildPhase", "shellScript": "generate", "inputPaths": ["$(SRCROOT)/script.sh"], "inputFileListPaths": ["$(SRCROOT)/inputs.xcfilelist"], "outputFileListPaths": ["$(SRCROOT)/outputs.xcfilelist"]}}
phases = _xcode_shell_script_phases({{"label": {{"package": "", "id": "Seed"}}, "attr": {{"project": "App.xcodeproj"}}}}, {{"phase": phase}}, {{"buildPhases": ["phase"]}}, {{}}, "", "App")
action = json_decode(phases["actions"][0])
result = repr([action["inputs"], action["outputs"], action["env"]["SCRIPT_INPUT_FILE_COUNT"], action["env"]["SCRIPT_INPUT_FILE_LIST_COUNT"], action["env"]["SCRIPT_OUTPUT_FILE_COUNT"], action["env"]["SCRIPT_OUTPUT_FILE_LIST_0"], action["cacheable"]])
"#,
        xcode_prelude_source()
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["script.sh", "input.txt", "inputs.xcfilelist", "outputs.xcfilelist"], [".once/out/App/result.txt"], "1", "1", "0", "/workspace/outputs.xcfilelist", True]"#
    );
}

#[test]
fn missing_file_lists_fail_instead_of_silently_weakening_the_contract() {
    let source = format!(
        r#"{}
def workspace_root():
    return "/workspace"
def host_file_exists(path):
    return False
result = repr(_xcode_shell_phase_paths({{"inputFileListPaths": ["$(SRCROOT)/missing.xcfilelist"]}}, "inputPaths", "inputFileListPaths", {{"SRCROOT": "/workspace"}}))
"#,
        xcode_prelude_source()
    );
    assert!(eval_prelude_source_to_repr(source)
        .unwrap_err()
        .contains("Xcode script file list does not exist"));
}

#[test]
fn rebasing_does_not_rewrite_a_neighboring_target_prefix() {
    let source = format!(
        r#"{}
result = repr(_apple_rebase_build_value("-I/workspace/.once/out/App/Headers:/workspace/.once/out/AppOther", "/workspace/.once/out/App", "/workspace/.once/out/config/App"))
"#,
        apple_prelude_source()
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#""-I/workspace/.once/out/config/App/Headers:/workspace/.once/out/AppOther""#
    );
}

#[test]
fn postbuild_bundle_capture_tracks_undeclared_resources_and_clears_stale_signatures() {
    let workspace = TempDir::new().unwrap();
    let source = format!(
        r#"{}
ctx = {{"label": {{"id": "App"}}, "attr": {{}}, "build_dir": ".once/out/App"}}
result = repr(_apple_run_postbuild_actions(ctx, {{"postbuild_actions": [_json_encode({{"id": "phase", "shell": "/bin/sh", "script": "modify-product", "cacheable": False}})]}}, [".once/out/App/App.app/App"], ".once/out/App/App.app"))
"#,
        apple_prelude_source()
    );
    let (store, result) = with_active_store(store_for(workspace.path(), ""), || {
        eval_prelude_source_to_repr(source)
    });
    assert_eq!(result.unwrap(), r#"[".once/out/App/App.app"]"#);
    let action = action_by_identifier(&store, "postbuild_action:App:phase");
    assert!(action
        .outputs
        .contains(&".once/out/App/App.app".to_string()));
    assert!(!action
        .outputs
        .contains(&".once/out/App/App.app/App".to_string()));
    assert_eq!(
        action.clean_paths,
        [
            ".once/out/App/App.app/_CodeSignature",
            ".once/out/App/App.app/Contents/_CodeSignature"
        ]
    );
}

#[test]
fn dependency_products_are_staged_for_the_native_environment() {
    let workspace = TempDir::new().unwrap();
    let source = format!(
        r#"{}
_apple_script_dependency_products({{
    "label": {{"id": "App"}}, "build_dir": ".once/out/config/App",
    "deps": [{{"framework_path": ".once/out/Framework/Library.framework", "framework_files": [".once/out/Framework/Library.framework/Library"]}}],
}})
result = repr(True)
"#,
        apple_prelude_source()
    );
    let (store, result) = with_active_store(store_for(workspace.path(), ""), || {
        eval_prelude_source_to_repr(source)
    });
    result.unwrap();
    let action = action_by_identifier(
        &store,
        "script_product:.once/out/config/App/Library.framework",
    );
    assert!(action.cacheable);
    assert_eq!(action.outputs, [".once/out/config/App/Library.framework"]);
    assert!(action
        .inputs
        .contains(&".once/out/Framework/Library.framework/Library".to_string()));
}

#[test]
fn native_scripts_use_the_selected_developer_directory() {
    let source = format!(
        r#"{}
def workspace_root():
    return "/workspace"
phases = _xcode_shell_script_phases(
    {{"label": {{"id": "Seed", "package": ""}}, "attr": {{"project": "App.xcodeproj", "xcode_developer_dir": "/opt/Selected Xcode/Contents/Developer"}}}},
    {{"phase": {{"isa": "PBXShellScriptBuildPhase", "shellScript": "verify"}}}},
    {{"buildPhases": ["phase"]}}, {{"CUSTOM_TOOL": "$(DEVELOPER_DIR)/Toolchains"}}, "", "App",
)
env = json_decode(phases["actions"][0])["env"]
result = repr([env["DEVELOPER_DIR"], env["CUSTOM_TOOL"]])
"#,
        xcode_prelude_source()
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["/opt/Selected Xcode/Contents/Developer", "/opt/Selected Xcode/Contents/Developer/Toolchains"]"#
    );
}
