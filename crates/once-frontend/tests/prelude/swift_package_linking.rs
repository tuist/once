use super::{eval_prelude_source_to_repr, xcode_prelude_source};

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
