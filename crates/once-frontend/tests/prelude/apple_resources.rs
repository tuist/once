use super::*;

#[test]
fn implicit_catalog_discovery_supports_a_package_root_target() {
    let workspace = TempDir::new().unwrap();
    std::fs::create_dir(workspace.path().join("Assets.xcassets")).unwrap();
    std::fs::write(workspace.path().join("Assets.xcassets/Contents.json"), "{}").unwrap();
    let source = format!(
        r#"{}
info = _xcode_swift_package_with_implicit_resources("", workspace_root(), {{"targets": [{{
    "type": "regular", "name": "Root", "path": ".",
}}]}})
result = repr(info["targets"][0].get("resources"))
"#,
        xcode_prelude_source()
    );
    let (_, result) = with_active_store(store_for(workspace.path(), ""), || {
        eval_prelude_source_to_repr(source)
    });
    assert_eq!(
        result.unwrap(),
        r#"[{"path": "Assets.xcassets", "rule": {"process": {}}}]"#
    );
}

#[test]
fn native_package_root_generates_bundle_accessors_for_implicit_catalogs() {
    let workspace = TempDir::new().unwrap();
    let sources = workspace.path().join("Sources/Lib");
    std::fs::create_dir_all(sources.join("Assets.xcassets")).unwrap();
    std::fs::write(sources.join("Assets.xcassets/Contents.json"), "{}").unwrap();
    std::fs::write(sources.join("Lib.swift"), "import Foundation").unwrap();
    let source = format!(
        r#"{}
def _resolve_swiftc(platform, variant, developer):
    return {{"swiftc_path": "/swiftc", "env": {{}}}}
def _swiftpm_absolute_package_path(ctx, path):
    return workspace_root()
def host_command(argv, env = None):
    return '{{"name":"Resources","targets":[{{"name":"Lib","type":"regular"}}]}}'
graph = _swift_package_workspace_resolver({{
    "attrs": {{}}, "files": {{"Package.swift": "manifest"}},
    "label": {{"package": "", "name": "root", "id": "root"}},
}})
lib = [target for target in graph["targets"] if target["name"] == "SwiftPackage_Resources_Lib"][0]
result = repr([lib["attrs"]["resources"], lib["attrs"]["resource_bundle_name"],
    [json_decode(action)["outputs"] for action in lib["attrs"]["prebuild_actions"]]])
"#,
        all_prelude_source()
    );
    let (_, result) = with_active_store(store_for(workspace.path(), ""), || {
        eval_prelude_source_to_repr(source)
    });
    assert_eq!(
        result.unwrap(),
        r#"[["Sources/Lib/Assets.xcassets"], "Resources_Lib.bundle", [[".once/out/SwiftPackage_Resources_Lib/resource_bundle_accessor.swift"]]]"#
    );
}

#[test]
fn resource_bundles_compile_catalogs_together_but_preserve_copied_trees() {
    let workspace = TempDir::new().unwrap();
    for path in [
        "Processed/One.xcassets/Contents.json",
        "Processed/One.xcassets/Red.colorset/Contents.json",
        "Processed/Two.xcassets/Contents.json",
        "Processed/Two.xcassets/Blue.colorset/Contents.json",
        "Processed/Empty.xcassets/Contents.json",
        "Processed/message.txt",
        "Copied/Raw.xcassets/Contents.json",
        "Copied/Raw.xcassets/Red.colorset/Contents.json",
    ] {
        let path = workspace.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "{}").unwrap();
    }
    let source = format!(
        r#"{}
def _resolve_actool(developer):
    return {{"actool_path": "/actool", "env": {{}}, "identity": "actool"}}
ctx = {{
    "label": {{"package": "", "name": "Resources", "id": "Resources"}},
    "attr": {{"platform": "ios", "sdk_variant": "device", "minimum_os": "16.0",
        "resources": ["Processed", "Copied"], "structured_resources": ["Copied"]}},
    "build_dir": ".once/out/Resources", "deps": [], "srcs": [],
}}
result = repr(_apple_resource_bundle_target_impl(ctx))
"#,
        apple_prelude_source()
    );
    let (store, result) = with_active_store(store_for(workspace.path(), ""), || {
        eval_prelude_source_to_repr(source)
    });
    result.unwrap();
    let catalogs: Vec<_> = store
        .actions
        .iter()
        .filter(|action| action.argv.first().is_some_and(|arg| arg == "/actool"))
        .collect();
    assert_eq!(catalogs.len(), 1);
    let catalog = catalogs[0];
    assert_eq!(
        catalog.inputs,
        ["Processed/One.xcassets", "Processed/Two.xcassets"]
    );
    assert!(catalog
        .argv
        .windows(2)
        .any(|pair| pair == ["--platform", "iphoneos"]));
    assert!(catalog
        .outputs
        .iter()
        .any(|output| output.ends_with("/Assets.car")));
    assert!(!store.actions.iter().any(|action| action
        .outputs
        .iter()
        .any(|output| output.ends_with("/Contents.json"))));
    assert!(store.actions.iter().any(|action| action
        .outputs
        .iter()
        .any(|output| output.ends_with("/Copied"))));
    assert!(store.actions.iter().any(|action| action
        .outputs
        .iter()
        .any(|output| output.ends_with("/message.txt"))));
}

#[test]
fn package_resource_discovery_respects_versions_explicit_rules_and_ignored_paths() {
    let source = format!(
        r#"{}
def glob(patterns):
    return ["Sources/Lib/" + path for path in [
        "Images.xcassets/Contents.json", "Panel.nib/objects.nib", "View.xib", "Main.storyboard",
        "Localizable.xcstrings", "Model.xcdatamodeld/Model.xcdatamodel/contents",
        "Mapping.xcmappingmodel/contents", "Old/Legacy.xib", "Oldish/Kept.xib",
        ".hidden/Hidden.xib", "Nested.xcodeproj/Hidden.xib", "Code/Explicit.xib",
    ]]
def resources(version, declared = [], excluded = ["Old"], sources = ["Code"]):
    info = {{"toolsVersion": {{"_version": version}}, "targets": [{{
        "type": "regular", "name": "Lib", "resources": declared,
        "exclude": excluded, "sources": sources,
    }}]}}
    result = _xcode_swift_package_with_implicit_resources("", "/workspace", info)
    return [resource["path"] for resource in result["targets"][0]["resources"]]
result = repr([
    resources("5.2"), resources("5.3"), resources("5.9"),
    resources("6.0", [{{"path": ".", "rule": {{"process": {{}}}}}}]),
    resources("6.0", [{{"path": "./Images.xcassets/", "rule": {{"copy": {{}}}}}}]),
    resources("6.0", excluded = ["."]),
])
"#,
        xcode_prelude_source()
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"[[], ["Images.xcassets", "Panel.nib", "View.xib", "Main.storyboard", "Model.xcdatamodeld", "Mapping.xcmappingmodel", "Oldish/Kept.xib"], ["Images.xcassets", "Panel.nib", "View.xib", "Main.storyboard", "Localizable.xcstrings", "Model.xcdatamodeld", "Mapping.xcmappingmodel", "Oldish/Kept.xib"], ["."], ["./Images.xcassets/", "Panel.nib", "View.xib", "Main.storyboard", "Localizable.xcstrings", "Model.xcdatamodeld", "Mapping.xcmappingmodel", "Oldish/Kept.xib"], []]"#
    );
}
