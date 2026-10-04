use super::*;

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
