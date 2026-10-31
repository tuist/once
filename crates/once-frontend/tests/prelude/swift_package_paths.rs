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

def _swift_package_metadata(argv, absolute, env = {{}}, check = True):
    return host_command(argv, env = env, check = check)

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

#[test]
fn swift_package_explicit_dependency_path_wins_over_duplicate_discovered_product() {
    let source = format!(
        r#"{}
def workspace_root():
    return "/workspace"
def host_file_exists(path):
    return path == "/workspace/current/Same/Package.swift"
def _xcode_swift_package_info(ctx, path, identity = "", cache = None, discovery = False):
    return {{"identity": identity, "path": path, "info": {{"name": "Same", "products": [{{"name": "Shared"}}]}}}}
def glob(patterns):
    fail("an explicit dependency must not inspect stale discovery candidates")
infos = _xcode_expand_swift_package_infos({{}}, [{{"identity": "root", "path": "Root", "info": {{"dependencies": [{{"fileSystem": [{{"identity": "same", "path": "/workspace/current/Same"}}]}}]}}}}], follow_resolved = False)
products = _xcode_local_package_products({{}}, ["Shared"], packages = infos)
result = products["Shared"]["path"]
"#,
        xcode_prelude_source()
    );
    assert_eq!(eval_prelude_source_to_repr(source).unwrap(), "current/Same");
}

#[test]
fn swift_package_discovery_follows_explicit_paths_instead_of_first_duplicate_name() {
    let source = format!(
        r#"{}
def workspace_root():
    return "/workspace"
def _xcode_swift_package_info(ctx, path, identity = "", cache = None, discovery = False):
    dependencies = [{{"fileSystem": [{{"identity": "same", "path": "/workspace/current/Same"}}]}}] if path == "Root" else []
    products = [] if path == "Root" else [{{"name": "Shared"}}]
    return {{"identity": identity or "same", "path": path, "info": {{"name": "Same", "products": products, "dependencies": dependencies}}}}
def glob(patterns):
    return ["stale/Same/Package.swift", "current/Same/Package.swift", "Root/Package.swift"]
products = _xcode_local_package_products({{}}, ["Shared"])
result = products["Shared"]["path"]
"#,
        xcode_prelude_source()
    );
    assert_eq!(eval_prelude_source_to_repr(source).unwrap(), "current/Same");
}

#[test]
fn swift_package_ambiguous_discovery_and_conflicting_explicit_identity_report_both_paths() {
    let prelude = format!(
        r#"{}
def workspace_root():
    return "/workspace"
def _xcode_swift_package_info(ctx, path, identity = "", cache = None, discovery = False):
    return {{"identity": identity, "path": path, "info": {{"name": "Same", "products": [{{"name": "Shared"}}]}}}}
def glob(patterns):
    return ["stale/Same/Package.swift", "current/Same/Package.swift"]
"#,
        xcode_prelude_source()
    );
    let error = eval_prelude_source_to_repr(format!(
        "{prelude}\n_xcode_local_package_products({{}}, [\"Shared\"])\n"
    ))
    .unwrap_err();
    assert!(
        error.contains("stale/Same")
            && error.contains("current/Same")
            && error.contains(".package(path:)"),
        "{error}"
    );
    let error = eval_prelude_source_to_repr(format!(r#"{prelude}
_xcode_expand_swift_package_infos({{}}, [{{"identity": "same", "path": "stale/Same", "info": {{"dependencies": [{{"fileSystem": [{{"identity": "same", "path": "/workspace/current/Same"}}]}}]}}}}], follow_resolved = False)
"#)).unwrap_err();
    assert!(
        error.contains("stale/Same")
            && error.contains("current/Same")
            && error.contains("explicit .package(path:"),
        "{error}"
    );
}

#[cfg(unix)]
#[test]
fn swift_package_identity_accepts_existing_aliases_but_not_distinct_directories() {
    let workspace = TempDir::new().unwrap();
    let package = workspace.path().join("FetchDeeplinks");
    std::fs::create_dir(&package).unwrap();
    std::fs::create_dir(package.join("Nested")).unwrap();
    std::fs::create_dir(workspace.path().join("Other")).unwrap();
    std::os::unix::fs::symlink(&package, workspace.path().join("Alias")).unwrap();
    let mut aliases = vec!["Alias"];
    if workspace.path().join("FetchDeepLinks").exists() {
        aliases.push("FetchDeepLinks");
    }
    for alias in aliases {
        let source = format!(
            r#"{}
infos = _xcode_expand_swift_package_infos({{}}, [{{"identity": "fetchdeeplinks", "path": "FetchDeeplinks", "info": {{"dependencies": [{{"fileSystem": [{{"identity": "fetchdeeplinks", "path": workspace_root() + "/{alias}"}}]}}]}}}}], follow_resolved = False)
result = repr([len(infos), _xcode_same_package_path("FetchDeeplinks", "Other"), _xcode_same_package_path("FetchDeeplinks", "FetchDeeplinks/Nested")])
"#,
            xcode_prelude_source()
        );
        let (_, result) = with_active_store(store_for(workspace.path(), ""), || {
            eval_prelude_source_to_repr(source)
        });
        assert_eq!(result.unwrap(), "[1, False, False]");
    }
}

#[cfg(unix)]
#[test]
fn swift_package_warm_manifest_dump_invalidates_manifest_source_additions_and_removals() {
    use std::os::unix::fs::PermissionsExt;
    for discovery in [false, true] {
        let workspace = TempDir::new().unwrap();
        let package = workspace.path().join("Package");
        std::fs::create_dir(&package).unwrap();
        std::fs::write(
            package.join("Package.swift"),
            r#"{"name":"first","targets":[]}"#,
        )
        .unwrap();
        let tool = workspace.path().join("swift");
        std::fs::write(
            &tool,
            "#!/bin/sh\nprintf x >> \"$4/../count\"\ngrep -q invalid \"$4/Package.swift\" && exit 17\ncat \"$4/Package.swift\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let xcrun = workspace.path().join("xcrun");
        std::fs::write(
            &xcrun,
            format!("#!/bin/sh\nprintf '%s' '{}'\n", tool.display()),
        )
        .unwrap();
        std::fs::set_permissions(&xcrun, std::fs::Permissions::from_mode(0o755)).unwrap();
        let invoke = || {
            let source = format!(
                "{}\ndef host_which(name):\n    return {:?}\ninfo = _xcode_swift_package_info({{}}, \"Package\", discovery = {})\nresult = info[\"info\"][\"name\"] if info != None else \"skipped\"\n",
                xcode_prelude_source(),
                xcrun.display().to_string(),
                if discovery { "True" } else { "False" },
            );
            let (_, result) = with_active_store(store_for(workspace.path(), ""), || {
                eval_prelude_source_to_repr(source)
            });
            result.unwrap()
        };
        assert_eq!(invoke(), "first");
        assert_eq!(invoke(), "first");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("count")).unwrap(),
            "x"
        );
        std::fs::write(
            package.join("Package.swift"),
            r#"{"name":"second","targets":[]}"#,
        )
        .unwrap();
        assert_eq!(invoke(), "second");
        std::fs::write(package.join("File.swift"), "source").unwrap();
        invoke();
        std::fs::write(package.join("File.swift"), "edited").unwrap();
        invoke();
        std::fs::remove_file(package.join("File.swift")).unwrap();
        invoke();
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("count")).unwrap(),
            "xxxxx"
        );
        if discovery {
            std::fs::write(package.join("Package.swift"), "invalid template").unwrap();
            assert_eq!(invoke(), "skipped");
            assert_eq!(invoke(), "skipped");
            assert_eq!(
                std::fs::read_to_string(workspace.path().join("count")).unwrap(),
                "xxxxxxx"
            );
            std::fs::write(
                package.join("Package.swift"),
                r#"{"name":"repaired","targets":[]}"#,
            )
            .unwrap();
            assert_eq!(invoke(), "repaired");
            assert_eq!(invoke(), "repaired");
            assert_eq!(
                std::fs::read_to_string(workspace.path().join("count")).unwrap(),
                "xxxxxxxx"
            );
        }
    }
}
