use super::*;

fn fixture() -> String {
    format!(
        r#"{}
backend = {{
    "id": "backend-id", "name": "backend", "version": "0.1.0",
    "manifest_path": "/cargo/backend/Cargo.toml",
    "source": "git+https://example.com/repo#locked", "checksum": "checksum",
    "targets": [
        {{"name": "backend", "kind": ["lib"], "src_path": "/cargo/backend/src/lib.rs"}},
        {{"kind": ["custom-build"], "src_path": "/cargo/backend/build.rs"}},
    ],
}}
graphics = {{
    "id": "graphics-id", "name": "graphics", "version": "0.2.2",
    "manifest_path": "/cargo/graphics/Cargo.toml", "targets": [],
}}
packages = [backend, graphics]
ctx = {{"label": {{"package": "desktop", "id": "desktop/cargo"}}, "attrs": {{
    "build_script_sources": {{"backend@0.1.0": {{"../graphics/src/scene.rs": "graphics@0.2.2/src/scene.rs"}}}},
}}}}
"#,
        all_prelude_source()
    )
}

#[test]
fn cargo_sources_preserve_locked_identity_and_separate_host_layouts() {
    let source = format!(
        r#"{}
files = _cargo_build_script_source_files(ctx, packages, {{}}, True, "vendor", {{}})["backend-id"]
def specs():
    result = []
    for name, host in [("backend-0.1.0", False), ("backend-0.1.0-host", True)]:
        spec = _cargo_metadata_target_spec(ctx, backend, backend["targets"][0], {{"features": ["selected"]}},
            "unused", "unused/src/lib.rs", name, "rust_crate", ["./dep"], ["./build-dep"],
            {{"dep": "renamed"}}, "" if host else "destination-triple", host_tool = host,
            host_source_root = "/cargo/backend", source_files = files)
        attrs = spec["attrs"]
        result.append([attrs["source"], attrs["checksum"], attrs["features"], attrs["crate_aliases"],
            spec["deps"], spec["build_deps"], attrs["_cargo_source_root"],
            attrs["_cargo_materialized_source_root"], attrs["_cargo_build_script_sources"],
            attrs["rustc_env"]["CARGO_MANIFEST_DIR"]])
    return result
result = repr(specs())
"#,
        fixture()
    );
    let result = eval_prelude_source_to_repr(source).unwrap();
    assert_eq!(
        result
            .matches("git+https://example.com/repo#locked")
            .count(),
        2
    );
    assert_eq!(result.matches("checksum").count(), 2);
    assert_eq!(result.matches("selected").count(), 2);
    assert_eq!(result.matches("renamed").count(), 2);
    assert_eq!(result.matches("./build-dep").count(), 2);
    assert_eq!(result.matches("./dep").count(), 2);
    for target in ["backend-0.1.0", "backend-0.1.0-host"] {
        assert!(result.contains(&format!(
            ".once/out/desktop/{target}/source-layout/__package"
        )));
        assert!(result.contains(&format!(
            ".once/out/desktop/{target}/source-layout/graphics/src/scene.rs"
        )));
    }
    assert_eq!(result.matches("/cargo/graphics/src/scene.rs").count(), 2);
}

#[test]
fn cargo_sources_reject_ambiguous_packages_and_unsafe_paths() {
    for expression in [
        r#"_cargo_source_package("graphics", [graphics, dict(graphics, id = "other")])"#,
        r#"_cargo_source_package("missing", packages)"#,
        r#"_cargo_build_script_source_files(ctx, packages, {"backend-id": True}, True, "vendor", {})"#,
        r#"ctx["attrs"]["build_script_sources"] = {"backend": {"../../escape": "graphics/src/scene.rs"}}
_cargo_build_script_source_files(ctx, packages, {}, True, "vendor", {})"#,
        r#"ctx["attrs"]["build_script_sources"] = {"backend": {"../__package/build.rs": "graphics/src/scene.rs"}}
_cargo_build_script_source_files(ctx, packages, {}, True, "vendor", {})"#,
        r#"ctx["attrs"]["build_script_sources"] = {"backend": {"../graphics/src/scene.rs": "graphics/../secret"}}
_cargo_build_script_source_files(ctx, packages, {}, True, "vendor", {})"#,
    ] {
        let error = eval_prelude_source_to_repr(format!(
            "{}\n{expression}\nresult = repr(None)",
            fixture()
        ))
        .unwrap_err();
        assert!(error.contains("cargo.build_script_sources"), "{error}");
    }
}

#[test]
fn cargo_sources_reject_case_aliases_and_overlapping_destinations() {
    for destinations in [
        vec!["../__PACKAGE/build.rs"],
        vec!["../__Package/src/lib.rs"],
        vec!["../graphics/src/scene.rs", "../Graphics/SRC/SCENE.rs"],
        vec!["../graphics/src", "../GRAPHICS/SRC/scene.rs"],
        vec!["../graphics/src/scene.rs", "../GRAPHICS/SRC"],
    ] {
        let mappings = destinations
            .iter()
            .map(|destination| format!("{destination:?}: \"graphics/src/scene.rs\""))
            .collect::<Vec<_>>()
            .join(", ");
        let error = eval_prelude_source_to_repr(format!(
            "{}\nctx[\"attrs\"][\"build_script_sources\"] = {{\"backend\": {{{mappings}}}}}\n_cargo_build_script_source_files(ctx, packages, {{}}, True, \"vendor\", {{}})\nresult = repr(None)",
            fixture()
        ))
        .unwrap_err();
        assert!(error.contains("cargo.build_script_sources"), "{error}");
    }
}

#[test]
fn cargo_sources_skip_only_uniquely_locked_inactive_consumers_in_platform_snapshots() {
    for (selector, duplicate, succeeds) in [
        ("backend", false, true),
        ("backend@0.1.0", false, true),
        ("backned@0.1.0", false, false),
        ("backend@0.1.0", true, false),
    ] {
        let mut lock = "[[package]]\nname = \"backend\"\nversion = \"0.1.0\"\nsource = \"git+https://example.com/mac#locked\"\n".to_string();
        if duplicate {
            lock.push_str("[[package]]\nname = \"backend\"\nversion = \"0.1.0\"\nsource = \"git+https://example.com/other#locked\"\n");
        }
        let source = format!(
            r#"{}
def _rustc_toolchain(target):
    return ("rustc", "test", "x86_64-unknown-linux-gnu")
ctx = {{
    "label": {{"package": "", "id": "cargo"}},
    "attr": {{
        "target": "x86_64-unknown-linux-gnu",
        "metadata_file": "target.json", "host_metadata_file": "host.json",
        "build_script_sources": {{{selector:?}: {{"../graphics/src/scene.rs": "graphics@0.2.2/src/scene.rs"}}}},
    }},
    "files": {{"Cargo.toml": "", "Cargo.lock": {lock:?}}},
}}
def snapshot(host):
    return {{
        "packages": [], "workspace_members": [], "resolve": {{"nodes": []}},
        "once_snapshot": {{
            "inputs": _cargo_snapshot_inputs(ctx),
            "selection": _cargo_snapshot_selection(ctx, host, "x86_64-unknown-linux-gnu"),
        }},
    }}
ctx["files"]["target.json"] = _json_encode(snapshot(False))
ctx["files"]["host.json"] = _json_encode(snapshot(True))
result = repr(_cargo_resolved_metadata(ctx)["specs"])
"#,
            all_prelude_source()
        );
        let result = eval_prelude_source_to_repr(source);
        if succeeds {
            assert_eq!(result.unwrap(), "[]");
        } else {
            let error = result.unwrap_err();
            assert!(error.contains("cargo.build_script_sources"), "{error}");
        }
    }
}

#[test]
fn cargo_sources_materialize_files_as_declared_build_script_inputs() {
    let workspace = TempDir::new().unwrap();
    let host = workspace.path().join("host");
    std::fs::create_dir_all(&host).unwrap();
    std::fs::write(host.join("build.rs"), "fn main() {}").unwrap();
    let source_file = workspace.path().join("scene.rs");
    std::fs::write(&source_file, "source input").unwrap();
    let destination = ".once/out/backend/source-layout/graphics/src/scene.rs";
    let source = format!(
        r#"{}
def _rustc_toolchain(target):
    return ("rustc", "rustc-test", "test-host-triple")
ctx = {{"label": {{"package": "", "name": "backend", "id": "backend"}}, "srcs": [], "deps": [], "attr": {{
    "crate_root": ".once/out/backend/source-layout/__package/src/lib.rs",
    "build_script": ".once/out/backend/source-layout/__package/build.rs",
    "_cargo_source_root": {host:?},
    "_cargo_materialized_source_root": ".once/out/backend/source-layout/__package",
    "_cargo_build_script_sources": {{{destination:?}: {source_file:?}}},
}}}}
_rust_compile(ctx, "rlib", "src/lib.rs", "libbackend.rlib")
result = repr("ok")
"#,
        all_prelude_source(),
        host = host.to_string_lossy(),
        source_file = source_file.to_string_lossy(),
    );
    let (store, result) = with_active_store(store_for(workspace.path(), "backend"), || {
        eval_prelude_source_to_repr(source)
    });
    result.unwrap();
    assert!(store.actions.iter().any(|action| matches!(
        action.operation,
        Some(DeclaredActionOperation::MaterializeHostFile { ref destination, .. })
            if destination.ends_with("graphics/src/scene.rs")
    )));
    for identifier in [
        "backend:build-script-rustc",
        "backend:build-script",
        "backend:rustc",
    ] {
        let action = action_by_identifier(&store, identifier);
        assert!(
            action.inputs.contains(&destination.to_string()),
            "{identifier}: {:?}",
            action.inputs
        );
        assert!(action
            .inputs
            .contains(&".once/out/backend/source-layout/__package".to_string()));
    }
}
