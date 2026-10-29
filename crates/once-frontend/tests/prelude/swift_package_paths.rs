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

#[test]
fn prelude_xcode_swift_package_public_headers_path_keeps_an_explicit_empty_path() {
    let prelude = xcode_prelude_source();
    let empty = eval_prelude_function_in(
        &prelude,
        "_xcode_swift_package_public_headers_path",
        r#"({"publicHeadersPath": ""})"#,
    )
    .unwrap();
    let missing =
        eval_prelude_function_in(&prelude, "_xcode_swift_package_public_headers_path", "({})")
            .unwrap();
    let custom = eval_prelude_function_in(
        &prelude,
        "_xcode_swift_package_public_headers_path",
        r#"({"publicHeadersPath": "Sources/include"})"#,
    )
    .unwrap();

    assert_eq!(empty, r#""""#);
    assert_eq!(missing, r#""include""#);
    assert_eq!(custom, r#""Sources/include""#);
}

#[test]
fn swift_package_graph_preserves_public_header_paths() {
    for package_path in ["Packages/Layout", ".once/xcode-packages/layout"] {
        for header_path in ["missing", "null", "", "public"] {
            let workspace = TempDir::new().unwrap();
            let target_path = format!("{package_path}/Sources/Lib");
            let public_dir = match header_path {
                "missing" | "null" => format!("{target_path}/include"),
                "" => target_path.clone(),
                _ => format!("{target_path}/public"),
            };
            std::fs::create_dir_all(workspace.path().join(&public_dir)).unwrap();
            for (path, contents) in [
                (
                    format!("{target_path}/lib.c"),
                    "int api(void) { return 1; }",
                ),
                (format!("{target_path}/Private.h"), "// excluded"),
                (format!("{public_dir}/Api.h"), "int api(void);"),
                (
                    format!("{public_dir}/module.modulemap"),
                    "module Lib { header \"Api.h\" export * }",
                ),
            ] {
                std::fs::write(workspace.path().join(path), contents).unwrap();
            }
            let mut target = serde_json::json!({
                "name": "Lib", "type": "regular", "path": "Sources/Lib",
                "exclude": ["Private.h"],
            });
            match header_path {
                "missing" => {}
                "null" => target["publicHeadersPath"] = serde_json::Value::Null,
                value => target["publicHeadersPath"] = value.into(),
            }
            let packages = serde_json::json!([{
                "identity": "layout", "path": package_path,
                "info": {"targets": [target]},
            }])
            .to_string();
            let source = format!(
                r#"{}
graph = _xcode_local_swift_package_specs({{"label": {{"package": ""}}, "attr": {{}}}}, json_decode({packages:?}), "macos", "13.0", "simulator")
attrs = graph["specs"][0]["attrs"]
result = _json_encode([attrs["exported_headers"], attrs["exported_header_dirs"], attrs["modulemap"]])
"#,
                xcode_prelude_source(),
            );
            let (_, result) = with_active_store(store_for(workspace.path(), ""), || {
                eval_prelude_source_to_repr(source)
            });
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&result.unwrap()).unwrap(),
                serde_json::json!([
                    [format!("{public_dir}/Api.h")],
                    [public_dir],
                    format!("{public_dir}/module.modulemap"),
                ]),
                "{package_path}: {header_path:?}",
            );
        }
    }
}
