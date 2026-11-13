use super::*;

#[test]
fn bazel_test_query_selects_roots_without_removing_build_targets() {
    let source = format!(
        r#"{}
def host_which_optional(name):
    return "/tools/bazel"
def host_command(argv, cwd = None, env = None):
    if argv[1] == "--version":
        return "bazel 9"
    if argv[2] == "selected":
        return "cc_test rule //:unit\ncc_test rule //:outside_graph"
    return "cc_library rule //:library\ncc_test rule //:unit\ncc_test rule //:benchmark"
graph = bazel_workspace["resolver"]({{"label": {{"id": "bazel", "package": ""}}, "attr": {{"test_query": "selected"}}}})
result = repr([graph["roots"], graph["attrs"]["_default_test_roots"], [target["name"] for target in graph["targets"]]])
"#,
        all_prelude_source(),
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["bz_library"], ["bz_unit"], ["bz_library", "bz_unit", "bz_benchmark"]]"#
    );
}

#[test]
fn bazel_workspace_forwards_declared_environment_to_generated_targets() {
    let source = format!(
        r#"{}
def host_env(name):
    return {{"HOME": "/home/test", "PATH": "/tools"}}.get(name)
def host_which_optional(name):
    return "/tools/bazelisk" if name == "bazelisk" else ""
def host_file_exists(path):
    return False
def workspace_root():
    return "/workspace"
def host_command(argv, cwd = None, env = None):
    if env.get("SDK_ROOT") != "" or env.get("CUSTOM") != "declared":
        fail("missing declared environment")
    if argv[0] != "/tools/bazelisk" or cwd != "/workspace/project":
        fail("wrong tool or workspace")
    return "bazel 9" if argv[1] == "--version" else "cc_library rule //lib:core\ncc_test rule //lib:core_test"
ctx = {{"label": {{"id": "project/bazel", "package": "project"}}, "attr": {{"env": {{"SDK_ROOT": "", "CUSTOM": "declared"}}}}}}
graph = _bazel_workspace_resolver(ctx)
result = repr([graph["roots"], graph["attrs"]["_default_test_roots"], [target["attrs"]["env"] for target in graph["targets"]]])
"#,
        all_prelude_source(),
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["bz_lib_core"], ["bz_lib_core_test"], [{"SDK_ROOT": "", "CUSTOM": "declared"}, {"SDK_ROOT": "", "CUSTOM": "declared"}]]"#
    );
}

#[test]
fn bazel_explicit_executable_does_not_fall_back_to_bazelisk() {
    let source = format!(
        r#"{}
def host_which_optional(name):
    return "/tools/bazelisk" if name == "bazelisk" else ""
def host_file_exists(path):
    return False
result = repr(_bazel_resolve_executable({{"label": {{"id": "bazel"}}, "attr": {{"bazel": "missing-bazel"}}}}))
"#,
        all_prelude_source(),
    );
    assert!(eval_prelude_source_to_repr(source)
        .unwrap_err()
        .contains("Bazel executable `missing-bazel` was not found"));
}

#[test]
fn bazel_actions_preserve_empty_overrides_and_native_action_environment() {
    for (mnemonic, native) in [("CppCompile", true), ("Unsupported", false)] {
        let workspace = TempDir::new().unwrap();
        let source = format!(
            r#"{}
def host_env(name):
    return "/tools" if name == "PATH" else ""
def host_which_optional(name):
    return "/tools/bazel"
def host_command(argv, cwd = None, env = None, max_output_bytes = None):
    if len(argv) > 1 and argv[1] == "aquery":
        if env.get("SDK_ROOT") != "" or env.get("CUSTOM") != "declared":
            fail("missing analysis environment")
        return '{{"actions": [{{"mnemonic": "{mnemonic}", "arguments": {arguments}, "environmentVariables": [{{"key": "CUSTOM", "value": "native"}}]}}]}}'
    return "/output-base" if len(argv) > 1 and argv[1] == "info" else ""
ctx = {{
    "label": {{"id": "library", "name": "library", "package": ""}},
    "attr": {{"_bazel_resolved": True, "bazel_label": "//:library", "bazel_rule_kind": "cc_library", "env": {{"SDK_ROOT": "", "CUSTOM": "declared"}}}},
    "capability": "build",
}}
result = repr(bazel_target["impl"](ctx))
"#,
            all_prelude_source(),
            arguments = if native {
                r#"["cc", "source.cc"]"#
            } else {
                "[]"
            },
        );
        let (store, result) = with_active_store(store_for(workspace.path(), ""), || {
            eval_prelude_source_to_repr(source)
        });
        result.unwrap();
        let action = store.actions.last().unwrap();
        assert_eq!(action.env.get("SDK_ROOT").map(String::as_str), Some(""));
        assert_eq!(
            action.env.get("CUSTOM").map(String::as_str),
            Some(if native { "native" } else { "declared" })
        );
    }
}

fn package_source(source: &str) -> String {
    format!(
        "{}\n{}\n{source}",
        xcode_prelude_source(),
        include_str!("../../prelude/swift_package.star")
    )
}

#[test]
fn native_package_options_propagate_without_inferred_dependencies() {
    let source = package_source(
        r#"
def _resolve_swiftc(platform, variant, developer):
    return {"swiftc_path": "/swiftc", "env": {}}
def _swiftpm_absolute_package_path(ctx, path):
    return "/workspace"
def host_command(argv, env = None):
    return '{"name": "Root", "dependencies": [], "products": [], "targets": []}'
def _swift_package_metadata(argv, absolute, env = {}, check = True):
    return host_command(argv, env = env)
def _xcode_local_swift_package_specs(ctx, packages, platform, minimum_os, variant, root_identities = None):
    return {"specs": [{"kind": "apple_library", "attrs": {}, "deps": ["./Declared"]}, {"kind": "archive_download", "attrs": {}}], "products": {}, "modules": {}}
graph = _swift_package_workspace_resolver({
    "attrs": {"explicit_modules": True, "dependency_check": "error"},
    "files": {"Package.swift": "manifest"}, "label": {"id": "root", "name": "root", "package": ""},
})
result = repr([[spec["attrs"], spec.get("deps")] for spec in graph["targets"]])
"#,
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[[{"explicit_modules": True, "dependency_check": "error"}, ["./Declared"]], [{}, None]]"#
    );
}

#[test]
fn nested_package_source_membership_is_observed_for_graph_reuse() {
    use once_frontend::analysis::{AnalysisEngine, CommandPolicy, UnchangedWorkspace};

    let workspace = TempDir::new().unwrap();
    let sources = workspace.path().join("Dependencies/Local/Sources/Local");
    std::fs::create_dir_all(&sources).unwrap();
    std::fs::write(sources.join("First.swift"), "public let first = 1").unwrap();
    let source = package_source(
        r#"result = repr(_xcode_swift_package_target_sources("Dependencies/Local", {"name": "Local", "type": "regular"}))"#,
    );
    let (store, result) = with_active_store(store_for(workspace.path(), ""), || {
        eval_prelude_source_to_repr(source)
    });
    assert_eq!(
        result.unwrap(),
        r#"["Dependencies/Local/Sources/Local/First.swift"]"#
    );
    let observations_hold = || {
        AnalysisEngine::new().unwrap().observations_hold(
            workspace.path(),
            &store.observations,
            CommandPolicy::TrustDeclaredInputs,
            &UnchangedWorkspace::Unknown,
        )
    };
    assert!(observations_hold());
    std::fs::write(sources.join("Second.swift"), "public let second = 2").unwrap();
    assert!(!observations_hold());
}

#[test]
fn product_dependencies_expand_all_modules_even_when_a_target_has_the_same_name() {
    let source = package_source(
        r#"
targets = {"network\x1fNIO": "NIO", "network\x1fNIOCore": "Core"}
products = {"network\x1fNIO": ["NIO", "Core"], "NIO": "Unrelated"}
result = repr([
    _xcode_swift_package_dependencies({"dependencies": [{"product": ["NIO", "network"]}]}, "app", targets, products, "macos"),
    _xcode_swift_package_dependencies({"dependencies": [{"target": ["NIO"]}]}, "network", targets, products, "macos"),
    _xcode_swift_package_dependencies({"dependencies": [{"product": ["NIO", "missing"]}]}, "app", targets, products, "macos"),
])
"#,
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["./NIO", "./Core"], ["./NIO"], []]"#
    );
}

#[test]
fn local_only_dependencies_do_not_resolve_or_require_a_lockfile() {
    let source = package_source(
        r#"
def host_command(argv, env = None):
    fail("local dependencies must not run package resolve")
result = repr(_swift_package_remote_infos(
    {"files": {}}, {"dependencies": [{"fileSystem": [{"identity": "local", "path": "/workspace/local"}]}]},
    "swift", {}, "/workspace", "",
))
"#,
    );
    assert_eq!(eval_prelude_source_to_repr(source).unwrap(), "[]");
}

#[test]
fn local_package_closure_is_deduplicated_and_ignores_nested_lockfiles() {
    let source = package_source(
        r#"
def workspace_root():
    return "/workspace"
def host_file_exists(path):
    return path.endswith("/Package.swift")
def _xcode_package_resolved_pins_at(path):
    fail("nested lockfiles are not the root dependency resolution")
def _xcode_swift_package_info(ctx, path, identity = "", cache = None):
    return {"identity": identity, "path": path, "info": {"dependencies": [
        {"fileSystem": [{"identity": "shared", "path": "/workspace/shared"}]},
    ] if identity == "left" or identity == "right" else []}}
result = repr([package["identity"] for package in _xcode_expand_swift_package_infos({}, [{
    "identity": "root", "path": "", "info": {"dependencies": [
        {"fileSystem": [{"identity": "left", "path": "/workspace/left"}]},
        {"fileSystem": [{"identity": "right", "path": "/workspace/right"}]},
    ]},
}], follow_resolved = False)])
"#,
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"["root", "left", "right", "shared"]"#
    );
}

#[test]
fn transitive_remote_dependencies_still_require_resolution() {
    let source = package_source(
        r#"
calls = []
def host_command(argv, env = None):
    calls.append(argv)
    return ""
def host_file_exists(path):
    return path == "/workspace/Package.resolved"
def host_file_read(path):
    return '{"version": 3, "pins": []}'
_swift_package_remote_infos(
    {"files": {}}, {"dependencies": [{"fileSystem": [{"identity": "local"}]}]},
    "swift", {}, "/workspace", "", remote_dependencies = True,
)
result = repr(calls)
"#,
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["swift", "package", "resolve", "--package-path", "/workspace"]]"#
    );
}

#[test]
fn unreferenced_local_products_reuse_the_manifest_cache() {
    let source = package_source(
        r#"
def workspace_root():
    return "/workspace"
def glob(patterns):
    return ["Modules/Core/Package.swift"]
def host_command(argv, env = None):
    fail("the manifest is already parsed")
cache = {"/workspace/Modules/Core": {"info": {
    "name": "DifferentDisplayName", "products": [{"name": "Core", "targets": ["Core"]}],
}}}
result = repr(_xcode_local_package_products({}, ["Core"], cache = cache)["Core"]["identity"])
"#,
    );
    assert_eq!(eval_prelude_source_to_repr(source).unwrap(), r#""Core""#);
}

#[test]
fn product_discovery_skips_invalid_manifests_without_weakening_explicit_loading() {
    let workspace = TempDir::new().unwrap();
    let source = package_source(
        r#"
def workspace_root():
    return "/workspace"
def glob(patterns):
    return ["bin/template/Package.swift", "Modules/Core/Package.swift"]
def host_which(name):
    return name
def host_command(argv, check = True):
    if argv == ["xcrun", "--find", "swift"]:
        return "swift"
    if "template" in argv[-1]:
        if check:
            fail("explicit invalid manifest must fail")
        return ""
    return '{"name":"Core","products":[{"name":"Core","targets":["Core"]}],"targets":[]}'
def _swift_package_metadata(argv, absolute, env = {}, check = True):
    return host_command(argv, check = check)
result = repr(_xcode_local_package_products({}, ["Core"])["Core"]["path"])
"#,
    );
    let (_, result) = with_active_store(store_for(workspace.path(), ""), || {
        eval_prelude_source_to_repr(source.clone())
    });
    assert_eq!(result.unwrap(), r#""Modules/Core""#);
    let source = format!("{source}\n_xcode_swift_package_info({{}}, \"bin/template\")");
    let (_, result) = with_active_store(store_for(workspace.path(), ""), || {
        eval_prelude_source_to_repr(source)
    });
    assert!(result
        .unwrap_err()
        .contains("explicit invalid manifest must fail"));
}

#[test]
fn package_destinations_include_target_overrides_and_ignore_non_native_products() {
    let source = package_source(
        r#"
project = {
    "native_targets": [
        {"productType": "com.apple.product-type.application", "buildConfigurationList": "Mac"},
        {"productType": "com.apple.product-type.application", "buildConfigurationList": "Phone"},
        {"productType": "com.apple.product-type.app-extension", "buildConfigurationList": "Phone"},
        {"productType": "unsupported", "buildConfigurationList": "Watch"},
    ],
    "objects": {"Mac": {}, "Phone": {"buildConfigurations": ["PhoneDebug"]}, "PhoneDebug": {"name": "Debug", "buildSettings": {"SDKROOT": "iphoneos"}}},
    "project_settings": {"SDKROOT": "macosx"},
    "path_maps": {},
}
def destinations(settings):
    project["project_settings"] = settings
    return _xcode_package_platforms({"attr": {}}, project, "Debug")
result = repr([destinations(settings) for settings in [{"SDKROOT": "macosx"}, {"MACOSX_DEPLOYMENT_TARGET": "13.0"}, {"SUPPORTED_PLATFORMS": "macosx"}]])
"#,
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["macos", "ios"], ["macos", "ios"], ["macos", "ios"]]"#
    );
}

#[test]
fn framework_aliases_from_one_project_are_available_for_every_workspace_destination() {
    let source = package_source(
        r#"
def _xcode_realpath(path):
    return path
def _xcode_workspace_relative(path):
    return path
def _xcode_workspace_xcframework_specs(ctx, platform, sdk_variant):
    return [{"name": "Vendor", "kind": "apple_xcframework_import", "attrs": {"bundle": "Vendor.xcframework", "platform": platform}}]
def _xcode_register_absolute_xcframework_refs(objects, specs, names, platform, sdk_variant):
    pass
graphs = _xcode_workspace_framework_graphs({"attr": {}}, [
    {"objects": {"File": {"isa": "PBXFileReference", "name": "PublicModule.xcframework"}}, "file_paths": {"File": "Vendor.xcframework"}},
    {"objects": {}, "file_paths": {}},
], ["ios", "macos"])
result = repr([[platform, graph["specs"][0]["attrs"]["platform"], graph["modules"][_xcode_product_dependency_key("PublicModule")], graph["names"]["Vendor.xcframework"]] for platform, graph in graphs.items()])
"#,
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["ios", "ios", "Vendor_ios", "Vendor_ios"], ["macos", "macos", "Vendor_macos", "Vendor_macos"]]"#
    );
}

#[test]
fn shared_schemes_preserve_only_common_test_exclusions() {
    let source = package_source(
        r#"
def _xcode_scheme_paths(ctx):
    return ["first.xcscheme", "second.xcscheme"]
def _xcode_referenced_test_plan_paths(ctx):
    return []
def host_file_read(path):
    extra = '<Test Identifier="OnlyFirst"/>' if path == "first.xcscheme" else ''
    return '<TestableReference><BuildableReference BlueprintName = "Tests"/><SkippedTests><Test Identifier = "OfflineSuite"/>' + extra + '</SkippedTests></TestableReference><TestableReference><BuildableReference BlueprintName="OtherTests"/><SkippedTests><Test Identifier="OtherSuite/testCase"/></SkippedTests></TestableReference>'
settings = _xcode_test_plan_settings({})
result = repr([settings["Tests"]["skipped_tests"], settings["OtherTests"]["skipped_tests"]])
"#,
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["OfflineSuite"], ["OtherSuite/testCase"]]"#
    );
}

#[test]
fn generated_only_package_targets_keep_their_build_tool_dependency() {
    let source = package_source(
        r#"
graph = _xcode_local_swift_package_specs({"label": {"package": ""}}, [{"identity": "app", "path": "", "info": {"targets": [
    {"name": "App", "type": "regular", "pluginUsages": [{"plugin": ["Generator", None]}]},
]}}], "macos", "13.0", "simulator")
result = repr([[spec["kind"], spec["deps"]] for spec in graph["specs"]])
"#,
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[["swift_package_build_tools", []], ["apple_library", ["./SwiftPackage_app_App_BuildTools"]], ["swift_package_build_tools", []], ["apple_library", ["./SwiftPackage_app_App_MacroHost_BuildTools"]]]"#
    );
}
