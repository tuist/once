use super::{apple_prelude_source, eval_prelude_source_to_repr, xcode_prelude_source};

/// A macro is a tool for the targets that expand it, so its code must not
/// reach what they link. A target that unit-tests the macro is the exception:
/// it imports the module to reach the implementation types, so the macro
/// exposes its module and an archive under keys only that path reads.
#[test]
fn a_macro_offers_its_module_only_to_the_targets_that_test_it() {
    let result = eval_prelude_source_to_repr(format!(
        "{}\n{}",
        apple_prelude_source(),
        r#"
plugin = {
    "transitive_plugin_executables": ["/out/Plugin-tool#Plugin"],
    "transitive_plugin_module_dirs": ["/out/Plugin", "/out/Syntax"],
    "transitive_plugin_archives": ["/out/Plugin/Plugin.a", "/out/Syntax/Syntax.a"],
    "transitive_plugin_module_inputs": ["/out/Plugin/Plugin.swiftmodule"],
}
library = {"transitive_archives": ["/out/Lib/Lib.a"]}
collected = _apple_collect_macro_module_inputs([plugin, library])
(swiftmodule_dirs, header_dirs, modulemaps, hmaps, archives, framework_search_dirs, framework_module_names, framework_files, sdk_frameworks, sdk_dylibs, linkopts, vfs_overlays, plugin_dylibs, plugin_executables) = _collect_dep_compile_inputs([plugin, library], "/out/Consumer")
result = repr([
    collected["module_dirs"],
    collected["archives"],
    collected["module_inputs"],
    archives,
    plugin_executables,
])
"#
    ))
    .unwrap();
    assert_eq!(
        result,
        r#"[["/out/Plugin", "/out/Syntax"], ["/out/Plugin/Plugin.a", "/out/Syntax/Syntax.a"], ["/out/Plugin/Plugin.swiftmodule"], ["/out/Lib/Lib.a"], ["/out/Plugin-tool#Plugin"]]"#
    );
}

/// A macro only ever builds for the host, so a test target that links one has
/// to build for the host as well, and that carries every dependency of that
/// test target to the host too. Mixing a host build and a destination build of
/// the same module in one link redefines it.
#[test]
fn a_test_target_that_links_a_macro_builds_for_the_host() {
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
packages = [{"identity": "kit", "path": "", "info": {
    "platforms": [{"platformName": "ios", "version": "17.0"}],
    "targets": [
        {"name": "Support", "type": "regular"},
        {"name": "KitMacros", "type": "macro", "dependencies": [{"target": ["Support"]}]},
        {"name": "MacroTests", "type": "test", "dependencies": [
            {"target": ["KitMacros"]},
            {"target": ["Support"]},
        ]},
        {"name": "PlainTests", "type": "test", "dependencies": [{"target": ["Support"]}]},
    ],
}}]
graph = _xcode_local_swift_package_specs({"label": {"package": ""}, "attr": {}}, packages, "ios", "17.0", "simulator")
specs = {spec["name"]: spec for spec in graph["specs"]}
result = repr([
    [specs[name]["attrs"]["platform"], specs[name]["deps"]]
    for name in ["SwiftPackage_kit_MacroTests", "SwiftPackage_kit_PlainTests"]
])
"#
    ))
    .unwrap();
    assert_eq!(
        result,
        r#"[["macos", ["./SwiftPackage_kit_KitMacros", "./SwiftPackage_kit_Support_MacroHost"]], ["ios", ["./SwiftPackage_kit_Support"]]]"#
    );
}
