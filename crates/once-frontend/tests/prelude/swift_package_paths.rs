use super::*;

fn package_info_with_host(workspace: &Path, describe: &str) -> String {
    let prelude = xcode_prelude_source();
    let dumped = serde_json::json!({
        "name": "Layout",
        "products": [],
        "targets": [
            {"name": "Lib", "type": "regular", "dependencies": [], "exclude": [], "settings": []},
            {"name": "LibTests", "type": "test", "dependencies": [], "exclude": [], "settings": []},
        ],
    })
    .to_string();
    let source = format!(
        r#"{prelude}
calls = []
def host_which(name):
    return "/usr/bin/" + name

def host_command(argv, env = None, cwd = None, merge_stderr = None, check = True):
    if argv == ["/usr/bin/xcrun", "--find", "swift"]:
        return "/usr/bin/swift\n"
    if "dump-package" in argv:
        return {dumped:?}
    if "describe" in argv:
        calls.append("describe")
        if check:
            fail("package description must not fail analysis")
        return {describe:?}
    fail("unexpected host command: " + str(argv))

info = _xcode_swift_package_info({{}}, "Packages/Layout", "layout")
result = repr([calls, [target.get("path") for target in info["info"]["targets"]]])
"#
    );
    let store = store_for(workspace, "");
    let (_store, result) = with_active_store(store, || eval_prelude_source_to_repr(source));
    result.unwrap()
}

#[test]
fn swift_package_default_layout_skips_package_description() {
    let workspace = TempDir::new().unwrap();
    for directory in ["Sources/Lib", "Tests/LibTests"] {
        std::fs::create_dir_all(workspace.path().join("Packages/Layout").join(directory)).unwrap();
    }

    assert_eq!(
        package_info_with_host(workspace.path(), "{}"),
        "[[], [None, None]]"
    );
}

#[test]
fn swift_package_description_failure_keeps_dumped_manifest() {
    let workspace = TempDir::new().unwrap();
    std::fs::create_dir_all(workspace.path().join("Packages/Layout/Sources/Lib")).unwrap();

    assert_eq!(
        package_info_with_host(workspace.path(), ""),
        r#"[["describe"], [None, None]]"#
    );
}
