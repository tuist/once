use super::{eval_prelude_source_to_repr, xcode_prelude_source};

fn evaluate(body: &str) -> String {
    eval_prelude_source_to_repr(format!("{}\n{body}", xcode_prelude_source())).unwrap()
}

#[test]
fn unifies_dependency_traits_and_expands_nested_traits_until_stable() {
    let result = evaluate(
        r#"
packages = [
    {"identity": "streaming", "info": {"traits": [
        {"name": "Streaming", "enabledTraits": ["Storage"]},
        {"name": "Storage", "enabledTraits": ["Streaming"]},
    ]}},
    {"identity": "transport", "info": {
        "traits": [{"name": "Client"}, {"name": "Server"}],
        "dependencies": [{"registry": [{"identity": "streaming", "traits": [
            {"name": "Streaming", "condition": {"traits": ["Client", "Unused"]}},
            {"name": "Disabled", "condition": {"traits": ["Unused"]}},
        ]}]}],
    }},
    {"identity": "client", "info": {"dependencies": [{"sourceControl": [
        {"identity": "TRANSPORT", "traits": [{"name": "Client"}]},
    ]}]}},
    {"identity": "server", "info": {"dependencies": [{"fileSystem": [
        {"identity": "transport", "traits": [{"name": "Server"}]},
    ]}]}},
]
traits = _xcode_swift_package_resolved_traits(packages)
reordered = _xcode_swift_package_resolved_traits(list(reversed(packages)))
result = repr([traits["transport"], traits["streaming"], traits == reordered])
"#,
    );
    assert_eq!(
        result,
        r#"[["Client", "Server"], ["Storage", "Streaming"], True]"#
    );
}

#[test]
fn preserves_omitted_defaults_and_respects_explicit_trait_selection() {
    let result = evaluate(
        r#"
def package(identity):
    return {"identity": identity, "info": {"traits": [
        {"name": "default", "enabledTraits": ["DefaultFeature"]},
        {"name": "DefaultFeature", "enabledTraits": ["Nested"]},
        {"name": "Nested"},
        {"name": "OptIn"},
    ]}}
root = package("root")
root["info"]["dependencies"] = [{"sourceControl": [
    {"identity": "omitted"},
    {"identity": "disabled", "traits": []},
    {"identity": "selected", "traits": [{"name": "OptIn"}]},
    {"identity": "defaults", "traits": [{"name": "default"}, {"name": "OptIn"}]},
]}]
packages = [root] + [package(name) for name in ["omitted", "disabled", "selected", "defaults"]]
traits = _xcode_swift_package_resolved_traits(packages, ["ROOT"])
direct_roots = _xcode_swift_package_resolved_traits(packages, ["root", "disabled"])
result = repr([
    [traits[name] for name in ["root", "omitted", "disabled", "selected", "defaults"]],
    direct_roots["disabled"],
])
"#,
    );
    assert_eq!(
        result,
        r#"[[["DefaultFeature", "Nested"], ["DefaultFeature", "Nested"], [], ["OptIn"], ["DefaultFeature", "Nested", "OptIn"]], ["DefaultFeature", "Nested"]]"#
    );
}

#[test]
fn applies_resolved_traits_to_compilation_and_conditional_dependencies() {
    let result = evaluate(
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
    {"identity": "app", "path": "", "info": {"dependencies": [{"sourceControl": [
        {"identity": "streaming", "traits": [{"name": "Streaming"}]},
    ]}]}},
    {"identity": "streaming", "path": "Vendor/Streaming", "info": {
        "traits": [{"name": "Streaming"}],
        "targets": [
            {"name": "Storage", "type": "regular"},
            {"name": "Streaming", "type": "regular", "settings": [
                {"tool": "swift", "kind": {"define": {"_0": "HAS_STORAGE"}}, "condition": {"traits": ["Streaming", "Other"]}},
            ], "dependencies": [
                {"target": ["Storage", {"traits": ["Streaming", "Other"]}]},
                {"target": ["Disabled", {"traits": ["Other"]}]},
            ]},
            {"name": "Disabled", "type": "regular"},
            {"name": "Plugin", "type": "macro"},
        ],
    }},
]
graph = _xcode_local_swift_package_specs({"label": {"package": ""}, "attr": {}}, packages, "macos", "13.0", "simulator")
specs = {spec["name"]: spec for spec in graph["specs"]}
target = specs["SwiftPackage_streaming_Streaming"]
host = specs["SwiftPackage_streaming_Streaming_MacroHost"]
macro = specs["SwiftPackage_streaming_Plugin"]
result = repr([
    target["attrs"]["defines"],
    target["deps"],
    "HAS_STORAGE" in target["attrs"]["swift_flags"],
    "Streaming" in host["attrs"]["defines"],
    "Streaming" in macro["attrs"]["swift_flags"],
    _xcode_package_condition_allows({"platformNames": ["ios"], "traits": ["Streaming"]}, "macos", ["Streaming"]),
])
"#,
    );
    assert_eq!(
        result,
        r#"[["SWIFT_PACKAGE", "DEBUG", "Streaming"], ["./SwiftPackage_streaming_Storage"], True, True, True, False]"#
    );
}
