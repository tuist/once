use super::{eval_prelude_function_in, eval_prelude_source_to_repr, xcode_prelude_source};

/// Swift Package Manager hands the linker every object file a target
/// produced, so a file whose only contribution is a protocol conformance
/// still reaches the binary. Once collects a target into an archive instead,
/// and an archive member nothing references is dropped, which loses the
/// conformance at runtime. Library targets therefore force-load; a test
/// bundle and an executable are linked directly and do not.
#[test]
fn library_targets_force_load_so_conformances_survive() {
    let result = eval_prelude_source_to_repr(format!(
        "{}\n{}",
        xcode_prelude_source(),
        r#"
def workspace_root():
    return "/workspace"
def host_file_exists(path):
    return False
def host_file_read(path):
    return ""
def _xcode_swift_package_target_sources(package_path, target):
    return ["Sources/" + target["name"] + ".swift"]
packages = [{"identity": "kit", "path": "", "info": {"targets": [
    {"name": "Kit", "type": "regular"},
    {"name": "KitTests", "type": "test"},
    {"name": "kit-cli", "type": "executable"},
]}}]
graph = _xcode_local_swift_package_specs({"label": {"package": ""}, "attr": {}}, packages, "macos", "13.0", "simulator")
specs = {spec["name"]: spec for spec in graph["specs"]}
result = repr([
    [specs[name]["kind"], specs[name]["attrs"].get("alwayslink")]
    for name in ["SwiftPackage_kit_Kit", "SwiftPackage_kit_KitTests", "SwiftPackage_kit_kit-cli"]
])
"#
    ))
    .unwrap();
    assert_eq!(
        result,
        r#"[["apple_library", True], ["apple_test_bundle", None], ["apple_application", None]]"#
    );
}

#[test]
fn package_settings_preserve_linking_warning_levels_and_conditions() {
    let result = eval_prelude_source_to_repr(format!("{}\n{}", xcode_prelude_source(), r#"
def workspace_root():
    return "/workspace"
def host_file_exists(path):
    return False
def host_file_read(path):
    return ""
def _xcode_swift_package_target_sources(package_path, target):
    return ["Sources/Kit.swift", "Sources/native.c", "Sources/native.cpp"]
packages = [{"identity": "kit", "path": "", "info": {"targets": [{"name": "Kit", "type": "regular", "settings": [
    {"tool": "linker", "kind": {"linkedLibrary": {"_0": "sqlite3"}}},
    {"tool": "linker", "kind": {"linkedLibrary": {"_0": "c++"}}},
    {"tool": "linker", "kind": {"linkedFramework": {"_0": "Security"}}, "condition": {"platformNames": ["macos"]}},
    {"tool": "linker", "kind": {"linkedFramework": {"_0": "Foundation"}}, "condition": {"platformNames": ["macos"]}},
    {"tool": "linker", "kind": {"unsafeFlags": {"_0": ["-Xlinker", "-dead_strip"]}}, "condition": {"config": "release"}},
    {"tool": "linker", "kind": {"linkedLibrary": {"_0": "disabled"}}, "condition": {"traits": ["Disabled"]}},
    {"tool": "swift", "kind": {"treatAllWarnings": {"_0": "error"}}},
    {"tool": "swift", "kind": {"treatAllWarnings": {"_0": "warning"}}, "condition": {"platformNames": ["ios"]}},
    {"tool": "c", "kind": {"treatAllWarnings": {"_0": "error"}}},
    {"tool": "cxx", "kind": {"treatAllWarnings": {"_0": "warning"}}},
]}]}}]
def inspect_settings():
    result = []
    for platform, configuration in [("macos", "Debug"), ("ios", "Release")]:
        graph = _xcode_local_swift_package_specs({"label": {"package": ""}, "attr": {}}, packages, platform, "13.0", "simulator", configuration)
        attrs = graph["specs"][0]["attrs"]
        result.append([attrs["linkopts"], [flag for flag in attrs["swift_flags"] if "warnings-as-errors" in flag], attrs["per_source_clang_flags"]])
    return repr(result)
result = inspect_settings()
"#)).unwrap();
    assert_eq!(
        result,
        r#"[[["-lsqlite3", "-lc++", "-framework", "Security", "-framework", "Foundation"], ["-warnings-as-errors"], {"Sources/native.c": "[\"-Werror\"]", "Sources/native.cpp": "[\"-Wno-error\"]"}], [["-lsqlite3", "-lc++", "-Xlinker", "-dead_strip"], ["-warnings-as-errors", "-no-warnings-as-errors"], {"Sources/native.c": "[\"-Werror\"]", "Sources/native.cpp": "[\"-Wno-error\"]"}]]"#
    );
}

#[test]
fn declared_package_names_preserve_destination_host_and_deferred_graph_edges() {
    let result = eval_prelude_source_to_repr(format!(
        "{}\n{}",
        xcode_prelude_source(),
        r#"
def workspace_root():
    return "/workspace"
def host_file_exists(path):
    return False
def host_file_read(path):
    return ""
def _xcode_swift_package_target_sources(package_path, target):
    return ["Sources/" + target["name"] + ".swift"]
packages = [
    {"identity": "appauth-ios", "path": "", "info": {
        "products": [{"name": "AppAuth", "targets": ["Core", "Extras"]}],
        "targets": [{"name": "Core", "type": "regular"}, {"name": "Extras", "type": "regular"}],
    }},
    {"identity": "macro-tools", "path": "", "info": {
        "products": [{"name": "BuildMacros", "targets": ["Builder"]}],
        "targets": [{"name": "Builder", "type": "macro"}],
    }},
    {"identity": "LocalAlias", "path": "", "info": {
        "products": [{"name": "Local", "targets": ["Local"]}],
        "targets": [{"name": "Local", "type": "regular"}],
    }},
    {"identity": "scope.registry", "path": "", "info": {
        "products": [{"name": "Registered", "targets": ["Registered"]}],
        "targets": [{"name": "Registered", "type": "regular"}],
    }},
    {"identity": "consumer", "path": "", "info": {
        "dependencies": [
            {"sourceControl": [{"identity": "appauth-ios", "nameForTargetDependencyResolutionOnly": "Auth"}]},
            {"sourceControl": [{"identity": "macro-tools", "nameForTargetDependencyResolutionOnly": "Tools"}]},
            {"sourceControl": [{"identity": "not-yet", "nameForTargetDependencyResolutionOnly": "Future"}]},
            {"fileSystem": [{"identity": "local-real", "nameForTargetDependencyResolutionOnly": "LocalAlias"}]},
            {"registry": [{"identity": "scope.registry"}]},
        ],
        "targets": [
            {"name": "Client", "type": "regular", "dependencies": [
                {"product": ["AppAuth", "Auth", None, {"platformNames": ["ios"]}]},
                {"product": ["Local", "LocalAlias", None, None]},
                {"product": ["Registered", "scope.registry", None, None]},
                {"product": ["Later", "Future", None, None]},
            ]},
            {"name": "Plugin", "type": "macro", "dependencies": [{"product": ["AppAuth", "Auth", None, None]}]},
            {"name": "MacroTests", "type": "test", "dependencies": [
                {"product": ["BuildMacros", "Tools", None, None]},
                {"product": ["AppAuth", "Auth", None, None]},
            ]},
        ],
    }},
    {"identity": "unrelated", "path": "", "info": {
        "products": [{"name": "AppAuth", "targets": ["Decoy"]}],
        "targets": [{"name": "Decoy", "type": "regular"}],
    }},
]
graph = _xcode_local_swift_package_specs(
    {"label": {"package": ""}, "attr": {}}, packages, "ios", "17.0", "simulator",
    lazy_products = {"not-yet\x1fLater": True}, lazy_dependency = "Resolve",
)
specs = {spec["name"]: spec for spec in graph["specs"]}
result = repr([
    specs["SwiftPackage_consumer_Client"]["deps"],
    specs["SwiftPackage_consumer_Client_MacroHost"]["deps"],
    specs["SwiftPackage_consumer_Plugin"]["deps"],
    [specs["SwiftPackage_consumer_MacroTests"]["attrs"]["platform"], specs["SwiftPackage_consumer_MacroTests"]["deps"]],
])
"#
    ))
    .unwrap();
    assert_eq!(
        result,
        r#"[["./SwiftPackage_appauth-ios_Core", "./SwiftPackage_appauth-ios_Extras", "./SwiftPackage_LocalAlias_Local", "./SwiftPackage_scope.registry_Registered", "./Resolve"], ["./SwiftPackage_LocalAlias_Local_MacroHost", "./SwiftPackage_scope.registry_Registered_MacroHost", "./Resolve"], ["./SwiftPackage_appauth-ios_Core_MacroHost", "./SwiftPackage_appauth-ios_Extras_MacroHost"], ["macos", ["./SwiftPackage_macro-tools_Builder", "./SwiftPackage_appauth-ios_Core_MacroHost", "./SwiftPackage_appauth-ios_Extras_MacroHost"]]]"#
    );
}

#[test]
fn prelude_xcode_swift_package_dependencies_resolve_a_declared_package_name() {
    let prelude = xcode_prelude_source();
    let call = |aliases: &str| {
        eval_prelude_function_in(
            &prelude,
            "_xcode_swift_package_dependencies",
            &format!(
                r#"(
                    {{"dependencies": [{{"product": ["AppAuth", "AppAuth", None, None]}}]}},
                    "googlesignin-ios",
                    {{}},
                    {{"appauth-ios\x1fAppAuth": "AppAuthTarget"}},
                    "ios",
                    [],
                    {{}},
                    "",
                    {aliases},
                )"#
            ),
        )
        .unwrap()
    };
    let aliases = r#"_xcode_swift_package_dependency_aliases({
        "dependencies": [{
            "sourceControl": [{
                "identity": "appauth-ios",
                "nameForTargetDependencyResolutionOnly": "AppAuth",
            }],
        }],
    })"#;

    assert_eq!(call("{}"), "[]");
    assert_eq!(call(aliases), r#"["./AppAuthTarget"]"#);
}
