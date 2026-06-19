use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use once_frontend::analysis::{
    globals_for_prelude, target_kind_has_impl, with_active_store, AnalysisStore,
};
use once_frontend::{built_in_target_kind_schema, graph_from_targets, Target};
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::syntax::{AstModule, Dialect};
use starlark::values::list::ListRef;
use tempfile::TempDir;

fn store_for(workspace: &Path, package: &str) -> AnalysisStore {
    AnalysisStore::new(
        workspace.to_path_buf(),
        package.to_string(),
        format!(".once/out/{package}"),
    )
}

fn apple_prelude_source() -> String {
    format!(
        "{}\n{}",
        include_str!("../prelude/common.star"),
        include_str!("../prelude/apple.star")
    )
}

fn android_prelude_source() -> String {
    format!(
        "{}\n{}",
        include_str!("../prelude/common.star"),
        include_str!("../prelude/android.star")
    )
}

fn all_prelude_source() -> String {
    format!(
        "{}\n{}\n{}",
        include_str!("../prelude/common.star"),
        include_str!("../prelude/apple.star"),
        include_str!("../prelude/rust.star")
    )
}

#[test]
fn target_kind_has_impl_returns_true_for_apple_library() {
    assert!(target_kind_has_impl("apple_library").unwrap());
}

#[test]
fn apple_application_exposes_build_and_run() {
    let target = Target {
        package: "apps/ios".to_string(),
        kind: "apple_application".to_string(),
        name: "App".to_string(),
        deps: vec!["apps/ios/AppKit".to_string()],
        srcs: Vec::new(),
        attrs: BTreeMap::new(),
        typed_attrs: BTreeMap::new(),
    };

    let graph = graph_from_targets(&[target]);
    let app = &graph[0];
    assert_eq!(app.label.id, "apps/ios/App");
    let mut names = app
        .capabilities
        .iter()
        .map(|capability| capability.name.as_str())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(names, vec!["build", "run"]);
}

#[test]
fn android_binary_exposes_build_and_run() {
    let schema = built_in_target_kind_schema("android_binary").expect("android_binary schema");
    let run = schema
        .capabilities
        .iter()
        .find(|capability| capability.name == "run")
        .expect("android_binary run capability");
    assert_eq!(run.output_groups, vec!["default"]);
    assert_eq!(run.requires_outputs, vec!["apk"]);

    let attr_names = schema
        .attrs
        .iter()
        .map(|attr| attr.name.as_str())
        .collect::<Vec<_>>();
    assert!(attr_names.contains(&"adb"));
    assert!(attr_names.contains(&"adb_serial"));
    assert!(attr_names.contains(&"launch_activity"));
    assert!(attr_names.contains(&"kotlinc"));
    assert!(attr_names.contains(&"kotlin_home"));
    assert!(attr_names.contains(&"kotlin_stdlib"));
    assert!(!attr_names.contains(&"keytool"));
}

#[test]
fn android_target_kind_schemas_expose_all_target_kinds() {
    for kind in ["android_resource", "android_library", "android_binary"] {
        let schema = built_in_target_kind_schema(kind).expect("android target kind schema");
        assert_eq!(schema.kind, kind);
        assert!(
            !schema.examples.is_empty(),
            "{kind} should expose a starter example"
        );
        assert!(
            target_kind_has_impl(kind).unwrap(),
            "{kind} should have an impl"
        );
    }

    let library = built_in_target_kind_schema("android_library").unwrap();
    let attr_names = library
        .attrs
        .iter()
        .map(|attr| attr.name.as_str())
        .collect::<Vec<_>>();
    assert!(attr_names.contains(&"kotlinc_opts"));
    assert!(attr_names.contains(&"kotlinc"));
    assert!(attr_names.contains(&"kotlin_stdlib"));
}

#[test]
fn prelude_android_kotlin_toolchain_helpers_resolve_stdlib() {
    let prelude = android_prelude_source();

    let home = eval_prelude_string_function_in(
        &prelude,
        "_android_kotlin_home",
        r#"("/opt/kotlinc/bin/kotlinc")"#,
    )
    .unwrap();
    assert_eq!(home, "/opt/kotlinc");

    let default_stdlib = eval_prelude_string_function_in(
        &prelude,
        "_android_kotlin_stdlib",
        r#"({"kotlin_home": "/opt/kotlinc"}, "/ignored/bin/kotlinc")"#,
    )
    .unwrap();
    assert_eq!(default_stdlib, "/opt/kotlinc/lib/kotlin-stdlib.jar");

    let configured_stdlib = eval_prelude_string_function_in(
        &prelude,
        "_android_kotlin_stdlib",
        r#"({"kotlin_stdlib": "/third_party/kotlin-stdlib.jar"}, "/ignored/bin/kotlinc")"#,
    )
    .unwrap();
    assert_eq!(configured_stdlib, "/third_party/kotlin-stdlib.jar");
}

#[cfg(unix)]
#[test]
fn prelude_android_kotlin_compile_declares_merged_classes_action() {
    let prelude = android_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "apps/hello",
        "name": "Hello",
        "id": "apps/hello/Hello",
    }},
    "attr": {{
        "kotlinc_opts": ["-Xjsr305=strict"],
    }},
    "deps": [],
    "srcs": [],
    "build_dir": ".once/out/apps/hello/Hello",
}}
tools = {{
    "android_jar": "/sdk/platforms/android-35/android.jar",
    "kotlin_stdlib": "/kotlin/lib/kotlin-stdlib.jar",
    "kotlinc": "/kotlin/bin/kotlinc",
    "identity": "android-tools",
    "sdk_root": "/sdk",
}}
classes_dir, classes_hash = _android_compile_kotlin(
    ctx,
    ctx["attr"],
    tools,
    ["apps/hello/src/MainActivity.kt"],
    ".once/out/apps/hello/Hello/java_classes",
    ".once/out/apps/hello/Hello/classes.sha256",
    ["apps/hello/Greeting.jar", "/kotlin/lib/kotlin-stdlib.jar"],
)
result = repr([classes_dir, classes_hash])
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "apps/hello/Hello");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(
        out.unwrap(),
        "[\".once/out/apps/hello/Hello/classes\", \".once/out/apps/hello/Hello/classes.kotlin.sha256\"]"
    );
    assert_eq!(store.actions.len(), 1);
    let action = &store.actions[0];
    assert_eq!(
        action.identifier.as_deref(),
        Some("android_kotlin_compile:apps/hello/Hello")
    );
    assert_eq!(
        action.inputs,
        vec![
            "apps/hello/src/MainActivity.kt",
            ".once/out/apps/hello/Hello/classes.sha256",
            "apps/hello/Greeting.jar",
        ]
    );
    assert_eq!(
        action.outputs,
        vec![
            ".once/out/apps/hello/Hello/classes",
            ".once/out/apps/hello/Hello/classes.kotlin.sha256",
        ]
    );
    let script = &action.argv[2];
    assert!(script.contains("cp -R"), "{script}");
    assert!(
        script.contains(".once/out/apps/hello/Hello/java_classes"),
        "{script}"
    );
    assert!(script.contains("/kotlin/lib/kotlin-stdlib.jar"), "{script}");
    assert!(script.contains("-Xjsr305=strict"), "{script}");
}

#[cfg(unix)]
#[test]
fn prelude_android_debug_signing_declares_local_keystore_action() {
    let prelude = android_prelude_source();
    let workspace = TempDir::new().unwrap();
    let package_dir = workspace.path().join("apps/hello");
    std::fs::create_dir_all(&package_dir).unwrap();
    std::fs::write(package_dir.join("debug.keystore"), b"debug-keystore-bytes").unwrap();

    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "apps/hello",
        "name": "Hello",
        "id": "apps/hello/Hello",
    }},
    "attr": {{
        "debug_keystore": "debug.keystore",
    }},
    "deps": [],
    "srcs": [],
    "build_dir": ".once/out/apps/hello/Hello",
}}
tools = {{
    "apksigner": "/sdk/build-tools/35.0.0/apksigner",
    "identity": "android-tools",
    "sdk_root": "/sdk",
}}
apk, keystore = _android_sign_or_copy(
    ctx,
    ctx["attr"],
    tools,
    ".once/out/apps/hello/Hello/aligned.apk",
)
result = repr([apk, keystore])
"#
    );
    let store = store_for(workspace.path(), "apps/hello/Hello");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(
        out.unwrap(),
        "[\".once/out/apps/hello/Hello/Hello.apk\", \".once/out/apps/hello/Hello/debug.keystore\"]"
    );
    assert_eq!(store.actions.len(), 1);
    let action = &store.actions[0];
    assert_eq!(
        action.identifier.as_deref(),
        Some("android_sign:apps/hello/Hello")
    );
    assert_eq!(
        action.inputs,
        vec![
            "apps/hello/debug.keystore",
            ".once/out/apps/hello/Hello/aligned.apk",
        ]
    );
    assert_eq!(
        action.outputs,
        vec![
            ".once/out/apps/hello/Hello/Hello.apk",
            ".once/out/apps/hello/Hello/debug.keystore",
        ]
    );
    let script = &action.argv[2];
    assert!(
        script.contains("cp 'apps/hello/debug.keystore'"),
        "{script}"
    );
    assert!(script.contains("apksigner' sign"), "{script}");
    let identity = action.toolchain_identity.as_deref().unwrap();
    assert!(
        identity.contains(
            "\x00debug_sign\x00keystore_sha256\x00764ea889b83367ee6a573d3c0f09847e303701bee50a5a9cc068c9c5736fe37f"
        ),
        "{identity:?}"
    );
    assert!(!identity.contains("pass:android"), "{identity:?}");
}

#[test]
fn apple_library_schema_exposes_multi_arch_attributes() {
    let schema = built_in_target_kind_schema("apple_library").expect("apple_library schema");
    let attr_names = schema
        .attrs
        .iter()
        .map(|attr| attr.name.as_str())
        .collect::<Vec<_>>();

    assert!(
        attr_names.contains(&"archs"),
        "apple_library should expose an archs attribute, got {attr_names:?}"
    );
    assert!(
        attr_names.contains(&"mac_catalyst"),
        "apple_library should expose a mac_catalyst attribute, got {attr_names:?}"
    );
}

#[test]
fn apple_library_swift_compile_is_split_into_module_and_archive_actions() {
    let source = include_str!("../prelude/apple.star");

    assert!(source.contains("identifier = \"swift_module_compile_"));
    assert!(source.contains("outputs = [swiftmodule, swiftdoc, swift_objc_header]"));
    assert!(source.contains("identifier = \"swift_archive_compile_"));
    assert!(source.contains("outputs = [swift_archive]"));
}

#[test]
fn target_kind_has_impl_returns_true_for_swift_macro() {
    assert!(target_kind_has_impl("swift_macro").unwrap());
}

#[test]
fn target_kind_has_impl_returns_true_for_all_apple_bundle_kinds() {
    // Every bundled Apple target kind now has a Starlark impl that
    // declares actions; the CLI's generic fallback action is
    // bypassed for these kinds in favour of the Starlark-driven
    // analysis.
    assert!(target_kind_has_impl("apple_framework").unwrap());
    assert!(target_kind_has_impl("apple_application").unwrap());
    assert!(target_kind_has_impl("apple_test_bundle").unwrap());
}

fn eval_prelude_function(
    function_name: &str,
    call_source: &str,
) -> std::result::Result<String, String> {
    let prelude = apple_prelude_source();
    eval_prelude_function_in(prelude, function_name, call_source)
}

fn eval_prelude_function_in(
    prelude: impl AsRef<str>,
    function_name: &str,
    call_source: &str,
) -> std::result::Result<String, String> {
    let prelude = prelude.as_ref();
    let source = format!("{prelude}\nresult = repr({function_name}{call_source})\n");
    eval_prelude_source_to_repr(source)
}

fn eval_prelude_source_to_repr(source: String) -> std::result::Result<String, String> {
    // Build a Starlark module that splices the prelude's source
    // inline and invokes the requested helper. Returning the
    // result as a string via `repr()` keeps the test independent
    // of starlark Value plumbing details.
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse("test.star", source, &Dialect::Standard)
            .map_err(|error| format!("parse: {error:?}"))?;
        let globals = globals_for_prelude();
        let mut eval = Evaluator::new(&module);
        // The prelude calls host_arch() in some helpers, but the
        // resolver path itself doesn't. The host primitives
        // already return inert values outside of an active
        // analysis store, so this evaluates cleanly.
        eval.eval_module(ast, &globals)
            .map_err(|error| format!("eval: {error:?}"))?;
        let result = module
            .get("result")
            .ok_or_else(|| "missing result".to_string())?;
        Ok(result
            .unpack_str()
            .ok_or_else(|| "result was not a string".to_string())?
            .to_string())
    })
}

fn eval_prelude_string_function(
    function_name: &str,
    call_source: &str,
) -> std::result::Result<String, String> {
    let prelude = apple_prelude_source();
    eval_prelude_string_function_in(prelude, function_name, call_source)
}

fn eval_prelude_string_function_in(
    prelude: impl AsRef<str>,
    function_name: &str,
    call_source: &str,
) -> std::result::Result<String, String> {
    let prelude = prelude.as_ref();
    let source = format!("{prelude}\nresult = {function_name}{call_source}\n");
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse("test.star", source, &Dialect::Standard)
            .map_err(|error| format!("parse: {error:?}"))?;
        let globals = globals_for_prelude();
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)
            .map_err(|error| format!("eval: {error:?}"))?;
        let result = module
            .get("result")
            .ok_or_else(|| "missing result".to_string())?;
        Ok(result
            .unpack_str()
            .ok_or_else(|| "result was not a string".to_string())?
            .to_string())
    })
}

fn starlark_string_literal(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}

#[cfg(unix)]
fn write_executable(path: &Path, contents: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, contents).unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn prelude_resolve_select_picks_matching_branch() {
    let out = eval_prelude_function(
        "_resolve_select",
        r#"({"select": {"ios": ["FOO"], "macos": ["BAR"]}}, ["ios"], "tgt", "defines")"#,
    )
    .unwrap();
    assert_eq!(out, "[\"FOO\"]");
}

#[test]
fn prelude_resolve_select_falls_back_to_default() {
    let out = eval_prelude_function(
        "_resolve_select",
        r#"({"select": {"macos": "M", "default": "fallback"}}, ["ios"], "tgt", "x")"#,
    )
    .unwrap();
    assert_eq!(out, "\"fallback\"");
}

#[test]
fn prelude_resolve_select_prefers_longest_composite_key() {
    let out = eval_prelude_function(
            "_resolve_select",
            r#"({"select": {"ios": "ios-any", "ios:simulator": "ios-sim"}}, ["ios", "simulator"], "tgt", "x")"#,
        )
        .unwrap();
    assert_eq!(out, "\"ios-sim\"");
}

#[test]
fn prelude_resolve_select_fails_without_default() {
    let err = eval_prelude_function(
        "_resolve_select",
        r#"({"select": {"macos": "M"}}, ["ios"], "tgt", "x")"#,
    )
    .unwrap_err();
    assert!(err.contains("no branch matching"), "{err}");
}

#[test]
fn prelude_cargo_metadata_targets_preserve_rust_target() {
    let prelude = all_prelude_source();
    let out = eval_prelude_function_in(
            &prelude,
            "_cargo_metadata_targets",
            r#"({
                "attrs": {
                    "target": "x86_64-apple-darwin",
                    "vendor_dir": "third_party/rust/vendor",
                },
            }, {
                "packages": [{
                    "id": "registry+https://github.com/rust-lang/crates.io-index#cpufeatures@0.2.17",
                    "name": "cpufeatures",
                    "version": "0.2.17",
                    "source": "registry+https://github.com/rust-lang/crates.io-index",
                    "manifest_path": "/workspace/vendor/cpufeatures-0.2.17/Cargo.toml",
                    "targets": [{
                        "name": "cpufeatures",
                        "kind": ["lib"],
                        "crate_types": ["lib"],
                        "src_path": "/workspace/vendor/cpufeatures-0.2.17/src/lib.rs",
                        "edition": "2018",
                    }],
                }],
                "resolve": {
                    "nodes": [{
                        "id": "registry+https://github.com/rust-lang/crates.io-index#cpufeatures@0.2.17",
                        "features": [],
                        "deps": [],
                    }],
                },
            })"#,
        )
        .unwrap();

    assert!(out.contains("\"target\": \"x86_64-apple-darwin\""), "{out}");
    assert!(
            out.contains("\"srcs\": [\"third_party/rust/vendor/cpufeatures-0.2.17/Cargo.toml\", \"third_party/rust/vendor/cpufeatures-0.2.17/build.rs\", \"third_party/rust/vendor/cpufeatures-0.2.17/src/**/*.rs\"]"),
            "{out}"
        );
}

#[test]
fn prelude_cargo_metadata_targets_split_proc_macro_host_deps() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
targets = _cargo_metadata_targets({{
    "attrs": {{
        "target": "x86_64-apple-darwin",
        "vendor_dir": "third_party/rust/vendor",
    }},
}}, {{
    "packages": [
        {{
            "id": "registry+https://github.com/rust-lang/crates.io-index#quote@1.0.45",
            "name": "quote",
            "version": "1.0.45",
            "source": "registry+https://github.com/rust-lang/crates.io-index",
            "manifest_path": "/workspace/vendor/quote-1.0.45/Cargo.toml",
            "targets": [{{
                "name": "quote",
                "kind": ["lib"],
                "crate_types": ["lib"],
                "src_path": "/workspace/vendor/quote-1.0.45/src/lib.rs",
                "edition": "2018",
            }}],
        }},
        {{
            "id": "registry+https://github.com/rust-lang/crates.io-index#linktime-proc-macro@0.2.0",
            "name": "linktime-proc-macro",
            "version": "0.2.0",
            "source": "registry+https://github.com/rust-lang/crates.io-index",
            "manifest_path": "/workspace/vendor/linktime-proc-macro-0.2.0/Cargo.toml",
            "targets": [{{
                "name": "linktime_proc_macro",
                "kind": ["proc-macro"],
                "crate_types": ["proc-macro"],
                "src_path": "/workspace/vendor/linktime-proc-macro-0.2.0/src/lib.rs",
                "edition": "2021",
            }}],
        }},
    ],
    "resolve": {{
        "nodes": [
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#quote@1.0.45",
                "features": [],
                "deps": [],
            }},
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#linktime-proc-macro@0.2.0",
                "features": [],
                "deps": [{{
                    "name": "quote",
                    "pkg": "registry+https://github.com/rust-lang/crates.io-index#quote@1.0.45",
                    "dep_kinds": [{{"kind": None}}],
                }}],
            }},
        ],
    }},
}})
by_name = {{target["name"]: target for target in targets}}
result = repr([
    by_name["quote-1.0.45"]["attrs"].get("target"),
    by_name["quote-1.0.45-host"]["attrs"].get("target"),
    by_name["linktime-proc-macro-0.2.0"]["attrs"].get("target"),
    by_name["linktime-proc-macro-0.2.0"]["deps"],
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(
        out,
        "[\"x86_64-apple-darwin\", None, None, [\"./quote-1.0.45-host\"]]"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn prelude_cargo_metadata_targets_use_host_metadata_for_host_variants() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "attrs": {{
        "target": "x86_64-apple-darwin",
        "vendor_dir": "third_party/rust/vendor",
    }},
}}
packages = [
    {{
        "id": "registry+https://github.com/rust-lang/crates.io-index#builder@1.0.0",
        "name": "builder",
        "version": "1.0.0",
        "source": "registry+https://github.com/rust-lang/crates.io-index",
        "manifest_path": "/workspace/vendor/builder-1.0.0/Cargo.toml",
        "targets": [
            {{
                "name": "builder",
                "kind": ["lib"],
                "crate_types": ["lib"],
                "src_path": "/workspace/vendor/builder-1.0.0/src/lib.rs",
                "edition": "2021",
            }},
            {{
                "name": "build-script-build",
                "kind": ["custom-build"],
                "crate_types": ["bin"],
                "src_path": "/workspace/vendor/builder-1.0.0/build.rs",
                "edition": "2021",
            }},
        ],
    }},
    {{
        "id": "registry+https://github.com/rust-lang/crates.io-index#cpufeatures@0.2.17",
        "name": "cpufeatures",
        "version": "0.2.17",
        "source": "registry+https://github.com/rust-lang/crates.io-index",
        "manifest_path": "/workspace/vendor/cpufeatures-0.2.17/Cargo.toml",
        "targets": [{{
            "name": "cpufeatures",
            "kind": ["lib"],
            "crate_types": ["lib"],
            "src_path": "/workspace/vendor/cpufeatures-0.2.17/src/lib.rs",
            "edition": "2018",
        }}],
    }},
    {{
        "id": "registry+https://github.com/rust-lang/crates.io-index#libc@0.2.186",
        "name": "libc",
        "version": "0.2.186",
        "source": "registry+https://github.com/rust-lang/crates.io-index",
        "manifest_path": "/workspace/vendor/libc-0.2.186/Cargo.toml",
        "targets": [{{
            "name": "libc",
            "kind": ["lib"],
            "crate_types": ["lib"],
            "src_path": "/workspace/vendor/libc-0.2.186/src/lib.rs",
            "edition": "2021",
        }}],
    }},
]
target_metadata = {{
    "packages": packages,
    "resolve": {{
        "nodes": [
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#builder@1.0.0",
                "features": [],
                "deps": [{{
                    "name": "cpufeatures",
                    "pkg": "registry+https://github.com/rust-lang/crates.io-index#cpufeatures@0.2.17",
                    "dep_kinds": [{{"kind": "build"}}],
                }}],
            }},
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#cpufeatures@0.2.17",
                "features": [],
                "deps": [],
            }},
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#libc@0.2.186",
                "features": [],
                "deps": [],
            }},
        ],
    }},
}}
host_metadata = {{
    "packages": packages,
    "resolve": {{
        "nodes": [
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#builder@1.0.0",
                "features": [],
                "deps": [{{
                    "name": "cpufeatures",
                    "pkg": "registry+https://github.com/rust-lang/crates.io-index#cpufeatures@0.2.17",
                    "dep_kinds": [{{"kind": "build"}}],
                }}],
            }},
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#cpufeatures@0.2.17",
                "features": [],
                "deps": [{{
                    "name": "libc",
                    "pkg": "registry+https://github.com/rust-lang/crates.io-index#libc@0.2.186",
                    "dep_kinds": [{{"kind": None}}],
                }}],
            }},
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#libc@0.2.186",
                "features": [],
                "deps": [],
            }},
        ],
    }},
}}
targets = _cargo_metadata_targets(ctx, target_metadata, host_metadata)
by_name = {{target["name"]: target for target in targets}}
result = repr([
    by_name["cpufeatures-0.2.17"]["deps"],
    by_name["cpufeatures-0.2.17-host"]["deps"],
    by_name["cpufeatures-0.2.17-host"]["attrs"].get("target"),
    by_name["libc-0.2.186-host"]["attrs"].get("target"),
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(out, "[[], [\"./libc-0.2.186-host\"], None, None]");
}

#[test]
fn prelude_cargo_metadata_targets_use_host_metadata_for_proc_macro_features() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "attrs": {{
        "target": "x86_64-apple-darwin",
        "vendor_dir": "third_party/rust/vendor",
    }},
}}
packages = [
    {{
        "id": "registry+https://github.com/rust-lang/crates.io-index#document-features@0.2.12",
        "name": "document-features",
        "version": "0.2.12",
        "source": "registry+https://github.com/rust-lang/crates.io-index",
        "manifest_path": "/workspace/vendor/document-features-0.2.12/Cargo.toml",
        "targets": [{{
            "name": "document_features",
            "kind": ["proc-macro"],
            "crate_types": ["proc-macro"],
            "src_path": "/workspace/vendor/document-features-0.2.12/lib.rs",
            "edition": "2018",
        }}],
    }},
    {{
        "id": "registry+https://github.com/rust-lang/crates.io-index#litrs@1.0.0",
        "name": "litrs",
        "version": "1.0.0",
        "source": "registry+https://github.com/rust-lang/crates.io-index",
        "manifest_path": "/workspace/vendor/litrs-1.0.0/Cargo.toml",
        "targets": [{{
            "name": "litrs",
            "kind": ["lib"],
            "crate_types": ["lib"],
            "src_path": "/workspace/vendor/litrs-1.0.0/src/lib.rs",
            "edition": "2021",
        }}],
    }},
]
target_metadata = {{
    "packages": packages,
    "resolve": {{
        "nodes": [
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#document-features@0.2.12",
                "features": [],
                "deps": [],
            }},
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#litrs@1.0.0",
                "features": [],
                "deps": [],
            }},
        ],
    }},
}}
host_metadata = {{
    "packages": packages,
    "resolve": {{
        "nodes": [
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#document-features@0.2.12",
                "features": ["default"],
                "deps": [{{
                    "name": "litrs",
                    "pkg": "registry+https://github.com/rust-lang/crates.io-index#litrs@1.0.0",
                    "dep_kinds": [{{"kind": None}}],
                }}],
            }},
            {{
                "id": "registry+https://github.com/rust-lang/crates.io-index#litrs@1.0.0",
                "features": [],
                "deps": [],
            }},
        ],
    }},
}}
targets = _cargo_metadata_targets(ctx, target_metadata, host_metadata)
by_name = {{target["name"]: target for target in targets}}
result = repr([
    by_name["document-features-0.2.12"]["attrs"]["features"],
    by_name["document-features-0.2.12"]["deps"],
])
"#
    );
    let out = eval_prelude_source_to_repr(source).unwrap();

    assert_eq!(out, "[[\"default\"], [\"./litrs-1.0.0-host\"]]");
}

#[cfg(unix)]
#[test]
fn prelude_rust_build_script_metadata_deps_are_not_duplicated() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "crates/app",
        "name": "app",
        "id": "crates/app/app",
    }},
    "attr": {{
        "build_script": "build.rs",
        "crate_root": "src/lib.rs",
    }},
    "deps": [{{
        "label_id": "third_party/rust/native",
        "crate_name": "native",
        "rlib": ".once/out/native/libnative.rlib",
        "links": "native",
        "build_script_stdout": ".once/out/native/build-script.stdout",
    }}],
    "srcs": ["src/**/*.rs"],
}}
_rust_compile(ctx, "rlib", "src/lib.rs", "libapp.rlib")
result = repr("ok")
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "crates/app/app");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"ok\"");
    let script = store
        .actions
        .iter()
        .find(|action| action.identifier.as_deref() == Some("crates/app/app:build-script"))
        .and_then(|action| action.argv.get(2))
        .unwrap();
    assert_eq!(script.matches("done <").count(), 1, "{script}");
}

#[cfg(unix)]
#[test]
fn prelude_rust_build_script_env_encodes_rustflags() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
rustc, _identity, host_triple = _rustc_toolchain("")
ctx = {{
    "label": {{
        "package": "crates/app",
        "name": "app",
        "id": "crates/app/app",
    }},
    "attr": {{
        "rustc_flags": ["-C", "opt-level=3"],
    }},
    "deps": [],
    "srcs": [],
}}
env = _rust_build_script_env(
    ctx,
    rustc,
    host_triple,
    host_triple,
    ".once/out/app/build",
    "crates/app/build.rs",
)
result = repr(env.get("CARGO_ENCODED_RUSTFLAGS"))
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "crates/app/app");

    let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"-C\\x1fopt-level=3\"");
}

#[cfg(unix)]
#[test]
fn prelude_rust_proc_macro_compile_uses_host_target() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
_rustc, _identity, host_triple = _rustc_toolchain("")
def other_target(host_triple):
    if host_triple == "aarch64-unknown-linux-gnu":
        return "x86_64-unknown-linux-gnu"
    return "aarch64-unknown-linux-gnu"
ctx = {{
    "label": {{
        "package": "macros/stringify",
        "name": "stringify",
        "id": "macros/stringify",
    }},
    "attr": {{
        "target": other_target(host_triple),
        "crate_root": "src/lib.rs",
    }},
    "deps": [],
    "srcs": ["src/**/*.rs"],
}}
_rust_compile(ctx, "proc-macro", "src/lib.rs", "libstringify.so")
result = repr("ok")
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "macros/stringify");

    let (store, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "\"ok\"");
    let action = &store.actions[0];
    assert!(
        !action.argv.iter().any(|arg| arg == "--target"),
        "{:?}",
        action.argv
    );
    assert_eq!(
        action.outputs,
        vec![".once/out/macros/stringify/libstringify.so".to_string()]
    );
}

#[cfg(unix)]
#[test]
fn prelude_rust_build_script_env_uses_absolute_c_tool_paths() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
rustc, _identity, host_triple = _rustc_toolchain("")
ctx = {{
    "label": {{
        "package": "third_party/rust/vendor/pkg-1.0.0",
        "name": "pkg",
        "id": "third_party/rust/vendor/pkg-1.0.0",
    }},
    "attr": {{}},
    "srcs": [],
}}
tool_env = _rust_c_tool_env(host_triple, host_triple)
build_env = _rust_build_script_env(
    ctx,
    rustc,
    host_triple,
    host_triple,
    ".once/out/pkg/build",
    "third_party/rust/vendor/pkg-1.0.0/build.rs",
)
result = repr([
    tool_env.get("CC") or "",
    tool_env.get("AR") or "",
    tool_env.get("RANLIB") or "",
    tool_env.get("PKG_CONFIG") or "",
    tool_env.get("PATH") or "",
    build_env.get("CC") or "",
    build_env.get("AR") or "",
    build_env.get("RANLIB") or "",
    build_env.get("PKG_CONFIG") or "",
    build_env.get("PATH") or "",
])
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "");

    let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));
    let values: Vec<String> = serde_json::from_str(&out.unwrap()).unwrap();

    assert!(std::path::Path::new(&values[0]).is_absolute());
    assert!(std::path::Path::new(&values[1]).is_absolute());
    assert!(std::path::Path::new(&values[2]).is_absolute());
    if !values[3].is_empty() {
        assert!(std::path::Path::new(&values[3]).is_absolute());
    }
    assert_eq!(values[0], values[5]);
    assert_eq!(values[1], values[6]);
    assert_eq!(values[2], values[7]);
    assert_eq!(values[3], values[8]);
    assert_eq!(values[4], values[9]);
    for entry in values[4].split(':') {
        assert!(std::path::Path::new(entry).is_absolute());
    }
    for tool in [&values[0], &values[1], &values[2], &values[3]] {
        if tool.is_empty() {
            continue;
        }
        let tool_dir = std::path::Path::new(tool)
            .parent()
            .unwrap()
            .to_string_lossy();
        assert!(values[4].split(':').any(|entry| entry == tool_dir));
    }
    assert!(values[4].split(':').any(|entry| entry == "/bin"));
}

#[cfg(unix)]
#[test]
fn prelude_rust_compile_env_does_not_forward_unix_host_path() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
ctx = {{
    "label": {{
        "package": "crates/app",
        "name": "app",
        "id": "crates/app/app",
    }},
    "attr": {{}},
    "srcs": [],
}}
env = _rust_compile_env(ctx)
result = repr([
    env.get("PATH"),
    env.get("LIB"),
    env.get("INCLUDE"),
])
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "");

    let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "[None, None, None]");
}

#[cfg(unix)]
#[test]
fn prelude_rust_build_script_env_does_not_use_host_c_tool_for_cross_target() {
    let prelude = all_prelude_source();
    let source = format!(
        r#"{prelude}
_rustc, _identity, host_triple = _rustc_toolchain("")
def other_target(host_triple):
    if host_triple == "aarch64-unknown-linux-gnu":
        return "x86_64-unknown-linux-gnu"
    return "aarch64-unknown-linux-gnu"
target = other_target(host_triple)
env = _rust_c_tool_env(target, host_triple)
result = repr([
    env.get("CC"),
    env.get("AR"),
    env.get("PATH"),
    env.get("CC_" + target.replace("-", "_")),
    env.get("CC_" + target),
])
"#
    );
    let workspace = TempDir::new().unwrap();
    let store = store_for(workspace.path(), "");

    let (_, out) = with_active_store(store, || eval_prelude_source_to_repr(source));

    assert_eq!(out.unwrap(), "[None, None, None, None, None]");
}

#[test]
fn prelude_ios_simulator_selection_filters_to_iphone_and_ipad() {
    let out =
        eval_prelude_string_function("_ios_simulator_selection_script", r#"("/usr/bin/xcrun")"#)
            .unwrap();

    assert!(out.contains("ONCE_APPLE_SIMULATOR_UDID"), "{out}");
    assert!(out.contains("simctl list devices booted"), "{out}");
    assert!(out.contains("simctl list devices available"), "{out}");
    assert!(out.contains("/iPhone/ s/^.*"), "{out}");
    assert!(out.contains("/iPad/ s/^.*"), "{out}");
    assert!(out.contains("(Booted)[[:space:]]*$"), "{out}");
    assert!(out.contains("(Shutdown)[[:space:]]*$"), "{out}");
    assert!(!out.contains("sed -n 's/.*"), "{out}");
}

#[cfg(unix)]
#[test]
fn prelude_ios_simulator_selection_script_picks_booted_ios_device() {
    let tmp = TempDir::new().unwrap();
    let xcrun = tmp.path().join("xcrun");
    write_executable(
        &xcrun,
        r#"#!/bin/sh
if [ "${1:-}" = "simctl" ] && [ "${2:-}" = "list" ] && [ "${3:-}" = "devices" ] && [ "${4:-}" = "booted" ]; then
  printf '%s\n' '    Apple TV (AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA) (Booted)'
  printf '%s\n' '    iPhone Preview (BBBBBBBB-BBBB-BBBB-BBBB-BBBBBBBBBBBB) (Extra) (Booted)'
  printf '%s\n' '    iPhone 15 Pro (11111111-1111-1111-1111-111111111111) (Booted)'
  exit 0
fi
if [ "${1:-}" = "simctl" ] && [ "${2:-}" = "list" ] && [ "${3:-}" = "devices" ] && [ "${4:-}" = "available" ]; then
  printf '%s\n' '    iPad Pro (22222222-2222-2222-2222-222222222222) (Shutdown)'
  exit 0
fi
exit 1
"#,
    );
    let call = format!(
        "({})",
        starlark_string_literal(&xcrun.display().to_string())
    );
    let selection_script =
        eval_prelude_string_function("_ios_simulator_selection_script", &call).unwrap();
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("{selection_script}\nprintf '%s' \"$simulator_id\""))
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "11111111-1111-1111-1111-111111111111"
    );
}

#[cfg(unix)]
#[test]
fn prelude_ios_simulator_selection_script_errors_without_ios_device() {
    let tmp = TempDir::new().unwrap();
    let xcrun = tmp.path().join("xcrun");
    write_executable(
        &xcrun,
        r#"#!/bin/sh
if [ "${1:-}" = "simctl" ] && [ "${2:-}" = "list" ] && [ "${3:-}" = "devices" ]; then
  printf '%s\n' '    Apple TV (AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA) (Booted)'
  exit 0
fi
exit 1
"#,
    );
    let call = format!(
        "({})",
        starlark_string_literal(&xcrun.display().to_string())
    );
    let selection_script =
        eval_prelude_string_function("_ios_simulator_selection_script", &call).unwrap();
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("{selection_script}\nprintf '%s' \"$simulator_id\""))
        .output()
        .unwrap();

    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("no booted or available iOS simulator found"));
}

#[cfg(unix)]
#[test]
fn prelude_swift_testing_macros_plugin_uses_swift_toolchain_path() {
    let tmp = TempDir::new().unwrap();
    let xcrun = tmp.path().join("xcrun");
    let swiftc = tmp
        .path()
        .join("Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc");
    std::fs::create_dir_all(swiftc.parent().unwrap()).unwrap();
    write_executable(
        &xcrun,
        &format!(
            r#"#!/bin/sh
if [ "${{1:-}}" = "--find" ] && [ "${{2:-}}" = "swiftc" ]; then
  printf '%s\n' {}
  exit 0
fi
exit 1
"#,
            starlark_string_literal(&swiftc.display().to_string())
        ),
    );
    let store = store_for(tmp.path(), "");
    let call = format!(
        "({}, {{}})",
        starlark_string_literal(&xcrun.display().to_string())
    );

    let (_, out) = with_active_store(store, || {
        eval_prelude_string_function("_swift_testing_macros_plugin", &call)
    });

    assert_eq!(
            out.unwrap(),
            tmp.path()
                .join("Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/host/plugins/testing/libTestingMacros.dylib")
                .display()
                .to_string()
        );
}

#[cfg(unix)]
#[test]
fn prelude_swift_testing_macros_plugin_rejects_unexpected_swiftc_path() {
    let tmp = TempDir::new().unwrap();
    let xcrun = tmp.path().join("xcrun");
    write_executable(
        &xcrun,
        r#"#!/bin/sh
if [ "${1:-}" = "--find" ] && [ "${2:-}" = "swiftc" ]; then
  printf '%s\n' '/tmp/swiftc'
  exit 0
fi
exit 1
"#,
    );
    let store = store_for(tmp.path(), "");
    let call = format!(
        "({}, {{}})",
        starlark_string_literal(&xcrun.display().to_string())
    );

    let (_, err) = with_active_store(store, || {
        eval_prelude_string_function("_swift_testing_macros_plugin", &call).unwrap_err()
    });

    assert!(
        err.contains("unable to derive Swift toolchain path"),
        "{err}"
    );
}

#[test]
fn prelude_ios_simulator_selection_helper_feeds_run_and_test_scripts() {
    let source = include_str!("../prelude/apple.star");

    assert_eq!(
        source
            .matches("_ios_simulator_selection_script(xcrun) +")
            .count(),
        2
    );
}

/// The prelude `_serialize_hmap` helper must lay out the
/// header-map byte sequence correctly: 4-byte magic, version 1,
/// reserved 0, the rest of the header, a power-of-two bucket
/// array, and a string table that starts with a 0 byte. We assert
/// each invariant from a Starlark-driven run so the format
/// implementation stays a Starlark concern.
#[test]
fn prelude_serialize_hmap_lays_out_canonical_header_and_entries() {
    let prelude = apple_prelude_source();
    let source = format!(
        "{prelude}\n\
             entries = {{\"Foo.h\": \"AppCore/Foo.h\", \"Bar.h\": \"AppCore/Bar.h\"}}\n\
             bytes = _serialize_hmap(entries)\n"
    );
    let mut bytes: Option<Vec<u8>> = None;
    Module::with_temp_heap(|module| {
        let ast = AstModule::parse("test.star", source, &Dialect::Standard)?;
        let globals = globals_for_prelude();
        let mut eval = Evaluator::new(&module);
        eval.eval_module(ast, &globals)?;
        let value = module.get("bytes").expect("bytes binding");
        let list = ListRef::from_value(value).expect("bytes is a list");
        let collected: Vec<u8> = list
            .iter()
            .map(|item| u8::try_from(item.unpack_i32().expect("int byte")).expect("0..=255"))
            .collect();
        bytes = Some(collected);
        starlark::Result::Ok(())
    })
    .expect("prelude eval");
    let bytes = bytes.unwrap();

    // magic + version + reserved
    assert_eq!(&bytes[0..4], &0x6861_6D70_u32.to_le_bytes());
    assert_eq!(&bytes[4..6], &1u16.to_le_bytes());
    assert_eq!(&bytes[6..8], &0u16.to_le_bytes());

    // num_entries == 2; num_buckets is a power of two; strings
    // offset lands right after the bucket array.
    let strings_off = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    let num_entries = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    let num_buckets = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    assert_eq!(num_entries, 2);
    assert!(num_buckets.is_power_of_two() && num_buckets >= 2);
    assert_eq!(strings_off, 24 + (num_buckets as usize) * 12);
    assert_eq!(bytes[strings_off], 0);
}

#[test]
fn prelude_apple_config_tokens_rejects_select_on_platform() {
    let err = eval_prelude_function(
        "_apple_config_tokens",
        r#"({"platform": {"select": {"default": "ios"}}}, "tgt")"#,
    )
    .unwrap_err();
    assert!(
        err.contains("attribute `platform` cannot use select()"),
        "{err}"
    );
}

/// `_resolve_attrs` must reject `select()` on attributes the target kind
/// schema marks non-configurable (e.g. `module_name`). Without
/// this guard, a select on `module_name` would silently resolve
/// against the configuration and the build would proceed with a
/// rewritten module name, defeating the schema's intent.
#[test]
fn prelude_resolve_attrs_rejects_select_on_non_configurable_attr() {
    let err = eval_prelude_function(
            "_resolve_attrs",
            r#"({"platform": "ios", "module_name": {"select": {"ios": "X", "default": "Y"}}}, "tgt", ["module_name"])"#,
        )
        .unwrap_err();
    assert!(
        err.contains("attribute `module_name` is not configurable but uses select()"),
        "{err}"
    );
}
