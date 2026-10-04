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
