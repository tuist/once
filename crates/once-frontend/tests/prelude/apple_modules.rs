use super::*;

fn evaluate(source: &str) -> (AnalysisStore, anyhow::Result<String>) {
    let workspace = TempDir::new().unwrap();
    let code = format!("{}\n{source}", apple_prelude_source());
    with_active_store(store_for(workspace.path(), "test"), || {
        Module::with_temp_heap(|module| {
            let ast = AstModule::parse("test.star", code, &Dialect::Standard)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            Evaluator::new(&module)
                .eval_module(ast, &globals_for_prelude())
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            Ok(module.get("result").unwrap().to_repr())
        })
    })
}

#[test]
fn scan_preserves_compilation_context_without_output_or_link_inputs() {
    let (_, result) = evaluate(
        r#"
result = _apple_module_scan_argv([
    "swiftc", "-c", "-emit-module", "-module-name", "Example",
    "-target", "arm64-apple-ios17.0-simulator", "-D", "FEATURE",
    "-Xcc", "-DHEADER_FEATURE", "-import-objc-header", "Bridge.h",
    "-o", "Example.o", "-output-file-map", "objects.json",
    "-module-cache-path", "old-cache", "Example.swift", "Library.a",
    "-Xlinker", "-dead_strip",
], ".once/out/modules/toolchain")
"#,
    );
    let result = result.unwrap();
    assert!(result.contains("-scan-dependencies"));
    assert!(result.contains("-disable-bridging-pch"));
    assert!(result.contains("arm64-apple-ios17.0-simulator"));
    assert!(result.contains("Bridge.h"));
    assert!(result.contains("HEADER_FEATURE"));
    assert!(!result.contains("Example.o"));
    assert!(!result.contains("objects.json"));
    assert!(!result.contains("Library.a"));
    assert!(!result.contains("old-cache"));
}

#[test]
fn scan_rejects_incomplete_records() {
    let (_, result) = evaluate(r#"result = _apple_scan_records({"modules": [{"swift": "Main"}]})"#);
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("incomplete module record"));
}

#[test]
fn deferred_compilation_declares_scan_before_planning() {
    let (store, result) = evaluate(
        r#"
def _apple_module_toolchain_identity(swiftc):
    return "compiler-and-development-kit"
_apple_swift_action(
    {"deps": [], "label": {"id": "test/Main"}},
    {"explicit_modules": True},
    {"identity": "compiler", "swiftc_path": "swiftc", "env": {}},
    [], argv = ["swiftc", "-c", "Main.swift", "-o", "Main.o"],
    inputs = ["Main.swift"], outputs = ["Main.o"], identifier = "main",
)
result = True
"#,
    );
    result.unwrap();
    assert_eq!(store.actions.len(), 2);
    let scan = &store.actions[0];
    assert!(scan.stdout.is_none());
    assert_eq!(scan.argv.last(), scan.outputs.first());
    assert_eq!(scan.argv[scan.argv.len() - 2], "-o");
    assert_eq!(store.actions[1].inputs, scan.outputs);
    assert!(matches!(
        store.actions[1].operation,
        Some(DeclaredActionOperation::ExpandActions { .. })
    ));
}

#[test]
fn implicit_compilation_keeps_the_original_action() {
    let (store, result) = evaluate(
        r#"
_apple_swift_action({}, {}, {}, [], argv = ["swiftc", "Main.swift"], inputs = ["Main.swift"], outputs = ["Main"])
result = True
"#,
    );
    result.unwrap();
    assert_eq!(store.actions.len(), 1);
    assert_eq!(store.actions[0].argv, ["swiftc", "Main.swift"]);
}

fn scan_fixture() -> serde_json::Value {
    serde_json::json!({
        "mainModuleName": "Main",
        "modules": [
            {"swift": "Main"}, {"details": {"swift": {
                "sourceImportedDependencies": [{"clang": "Native"}]
            }}},
            {"swift": "Wrapper"}, {
                "modulePath": ".once/out/modules/test/Wrapper.swiftmodule",
                "directDependencies": [{"clang": "Native"}],
                "details": {"swift": {
                    "moduleInterfacePath": "Wrapper.swiftinterface",
                    "commandLine": ["-frontend", "-compile-module-from-interface", "Wrapper.swiftinterface",
                        "-o", ".once/out/modules/test/Wrapper.swiftmodule", "-Xcc",
                        "-fmodule-file=Native=.once/out/modules/test/Native.pcm"]
                }}
            },
            {"clang": "Native"}, {
                "modulePath": ".once/out/modules/test/Native.pcm",
                "sourceFiles": ["include/module.modulemap", "include/Native.h"],
                "details": {"clang": {
                    "moduleMapPath": "include/module.modulemap",
                    "commandLine": ["-frontend", "-emit-pcm", "include/module.modulemap",
                        "-o", ".once/out/modules/test/Native.pcm"]
                }}
            }
        ]
    })
}

fn plan(
    scan: &serde_json::Value,
    mode: &str,
    direct: &str,
) -> (AnalysisStore, anyhow::Result<String>) {
    evaluate(&format!(
        r#"
def host_file_read(path):
    return {scan}
def workspace_root():
    return "/workspace"
def host_file_sha256(path):
    return "content-of-" + _basename(path)
result = apple_explicit_module_plan({{
    "label": {{"id": "Main"}},
    "args": {{
        "scan": "scan.json", "compiler": "swiftc", "identity": "toolchain",
        "cache_path": ".once/out/modules/shared", "scan_cache": ".once/out/modules/test", "dependency_check": "{mode}", "host_roots": ["/DevelopmentKit"],
        "direct_modules": {direct}, "module_name": "Main",
        "action": {{"argv": ["swiftc", "Main.swift"], "inputs": ["Main.swift"],
                   "outputs": ["Main.o"], "identifier": "main"}},
    }},
}})
"#,
        scan = serde_json::to_string(&scan.to_string()).unwrap()
    ))
}

#[test]
fn planner_orders_modules_and_declares_headers_interfaces_and_module_map() {
    let (store, result) = plan(&scan_fixture(), "error", r#"{"Native": "Native"}"#);
    result.unwrap();
    assert_eq!(store.actions.len(), 4);
    let native = &store.actions[0];
    assert!(native.outputs[0].starts_with(".once/out/modules/shared/"));
    assert!(native.outputs[0].ends_with("/Native.pcm"));
    assert!(native.inputs.contains(&"include/Native.h".into()));
    assert!(native.inputs.contains(&"include/module.modulemap".into()));
    assert!(!native.depends_on_prior_actions);
    let wrapper = &store.actions[1];
    assert!(wrapper.inputs.contains(&native.outputs[0]));
    assert!(wrapper.inputs.contains(&"Wrapper.swiftinterface".into()));
    assert!(!wrapper.inputs.contains(&"include/Native.h".into()));
    assert!(wrapper
        .argv
        .contains(&format!("-fmodule-file=Native={}", native.outputs[0])));
    assert!(wrapper.argv.contains(&wrapper.outputs[0]));
    assert!(wrapper
        .argv
        .iter()
        .all(|arg| !arg.contains("modules/test/")));
    let compile = store.actions.last().unwrap();
    assert!(compile.inputs.contains(&wrapper.outputs[0]));
    assert!(compile.inputs.contains(&store.actions[2].outputs[0]));
    assert!(compile
        .argv
        .contains(&"-disable-implicit-swift-modules".into()));
    assert!(compile.argv.contains(&"-fno-implicit-modules".into()));
}

#[test]
fn strict_dependencies_return_a_structured_repair_without_compiling() {
    let (store, result) = plan(&scan_fixture(), "error", "{}");
    let diagnostic = result.unwrap();
    assert!(diagnostic.contains("undeclared_module_dependency"));
    assert!(diagnostic.contains("repairs"));
    assert!(diagnostic.contains("Native"));
    assert!(store.actions.is_empty());
    assert!(plan(&scan_fixture(), "off", "{}").1.is_ok());
}

#[test]
fn strict_dependencies_require_source_import_information() {
    let mut scan = scan_fixture();
    scan["modules"][1]["details"]["swift"] = serde_json::json!({});
    assert!(plan(&scan, "error", "{}")
        .1
        .unwrap_err()
        .to_string()
        .contains("does not report source imports"));
}

#[test]
fn planner_rejects_cycles_missing_dependencies_duplicates_and_escaped_outputs() {
    for reference in [
        serde_json::json!({"swift": "Wrapper"}),
        serde_json::json!({"clang": "Missing"}),
    ] {
        let mut scan = scan_fixture();
        scan["modules"][5]["directDependencies"] = serde_json::json!([reference]);
        assert!(plan(&scan, "off", "{}")
            .1
            .unwrap_err()
            .to_string()
            .contains("cyclic or unresolved"));
    }
    let mut scan = scan_fixture();
    scan["modules"][5]["modulePath"] = serde_json::json!("outside/Native.pcm");
    assert!(plan(&scan, "off", "{}")
        .1
        .unwrap_err()
        .to_string()
        .contains("escaped"));
    let mut scan = scan_fixture();
    scan["modules"][4] = serde_json::json!({"swift": "Wrapper"});
    assert!(plan(&scan, "off", "{}")
        .1
        .unwrap_err()
        .to_string()
        .contains("duplicate"));
}

#[test]
fn strict_dependencies_do_not_silently_run_without_explicit_modules() {
    let (_, result) = evaluate(
        r#"
_apple_swift_action({}, {"dependency_check": "error"}, {}, [])
result = True
"#,
    );
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("requires explicit_modules"));
}

#[test]
fn inferred_native_dependencies_do_not_count_as_declared_dependencies() {
    let (store, result) = evaluate(
        r#"
def _apple_module_toolchain_identity(swiftc):
    return "toolchain"
_apple_swift_action(
    {"label": {"id": "Main", "package": ""}},
    {"explicit_modules": True, "_declared_deps": ["Direct"]},
    {"swiftc_path": "swiftc", "env": {}},
    [{"module_name": "Direct", "label_id": "Direct"}, {"module_name": "Inferred", "label_id": "Inferred"}],
    argv = ["swiftc", "Main.swift"], inputs = ["Main.swift"], outputs = ["Main.o"], identifier = "main",
)
result = True
"#,
    );
    result.unwrap();
    let Some(DeclaredActionOperation::ExpandActions { args, .. }) = &store.actions[1].operation
    else {
        panic!("missing expansion")
    };
    assert_eq!(
        args["direct_modules"],
        serde_json::json!({"Direct": "Direct"})
    );
}

#[test]
fn prebuilt_workspace_modules_are_inputs_not_recompiled_modules() {
    let scan = serde_json::json!({
        "mainModuleName": "Main",
        "modules": [
            {"swift": "Main"}, {"details": {"swift": {"sourceImportedDependencies": [{"swiftPrebuiltExternal": "Vendor"}]}}},
            {"swiftPrebuiltExternal": "Vendor"}, {
                "modulePath": "Vendor/Vendor.swiftmodule",
                "details": {"swiftPrebuiltExternal": {"compiledModulePath": "Vendor/Vendor.swiftmodule"}}
            }
        ]
    });
    let (store, result) = plan(&scan, "error", r#"{"Vendor": "Vendor"}"#);
    result.unwrap();
    assert_eq!(store.actions.len(), 2);
    assert!(store.actions[1]
        .inputs
        .contains(&"Vendor/Vendor.swiftmodule".into()));
}

#[test]
fn development_kit_imports_are_exempt_from_strict_dependency_checks() {
    let mut scan = scan_fixture();
    scan["modules"][5]["details"]["clang"]["moduleMapPath"] =
        serde_json::json!("/DevelopmentKit/Native/module.modulemap");
    let (_, result) = plan(&scan, "error", "{}");
    assert_eq!(result.unwrap(), "None");
}

#[test]
fn arbitrary_host_modules_are_rejected_instead_of_silently_cached() {
    let mut scan = scan_fixture();
    scan["modules"][5]["details"]["clang"]["moduleMapPath"] =
        serde_json::json!("/untracked/Native/module.modulemap");
    let (store, result) = plan(&scan, "off", "{}");
    assert!(result.unwrap().contains("untracked_host_module_input"));
    assert!(store.actions.is_empty());
}

#[cfg(unix)]
#[test]
fn development_kit_symbolic_link_aliases_are_recognized() {
    let temporary = TempDir::new().unwrap();
    let kit = temporary.path().join("VersionedKit");
    std::fs::create_dir(&kit).unwrap();
    std::fs::write(kit.join("module.modulemap"), "module Native {}").unwrap();
    let alias = temporary.path().join("CurrentKit");
    std::os::unix::fs::symlink(&kit, &alias).unwrap();
    let (_, result) = evaluate(&format!(
        r#"
result = _apple_check_module_inputs(
    {{"args": {{"host_roots": [{kit:?}]}}, "label": {{"id": "Main"}}}},
    _apple_scan_records({{"modules": [{{"clang": "Native"}}, {{"sourceFiles": [{input:?}]}}]}}),
)
"#,
        kit = kit.to_str().unwrap(),
        input = alias.join("module.modulemap").to_str().unwrap()
    ));
    assert_eq!(result.unwrap(), "None");
}

#[test]
fn same_named_modules_with_different_inputs_have_distinct_output_paths() {
    let first = plan(&scan_fixture(), "off", "{}").0;
    let mut scan = scan_fixture();
    scan["modules"][5]["sourceFiles"] =
        serde_json::json!(["other/module.modulemap", "other/Different.h"]);
    let second = plan(&scan, "off", "{}").0;
    assert_ne!(first.actions[0].outputs, second.actions[0].outputs);
    assert_ne!(first.actions[1].outputs, second.actions[1].outputs);
}

#[test]
fn bridging_header_module_metadata_is_preserved() {
    let mut scan = scan_fixture();
    scan["modules"][1]["details"]["swift"]["bridgingHeader"] =
        serde_json::json!({"moduleDependencies": ["Native"]});
    let (store, result) = plan(&scan, "off", "{}");
    result.unwrap();
    let Some(DeclaredActionOperation::WriteFile { bytes, .. }) = &store.actions[2].operation else {
        panic!("missing module map")
    };
    let map: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(map[0]["isBridgingHeaderDependency"], true);
}

#[test]
fn absolute_workspace_imports_are_not_exempt_from_dependency_checks() {
    let mut scan = scan_fixture();
    scan["modules"][5]["details"]["clang"]["moduleMapPath"] =
        serde_json::json!("/workspace/include/module.modulemap");
    assert!(plan(&scan, "error", "{}")
        .1
        .unwrap()
        .contains("undeclared_module_dependency"));
}

#[test]
fn nested_native_references_and_framework_providers_are_declared_imports() {
    let (store, result) = evaluate(
        r#"
def _apple_module_toolchain_identity(swiftc):
    return "toolchain"
_apple_swift_action(
    {"label": {"id": "apps/client/Main", "package": "apps/client"}},
    {"explicit_modules": True, "_declared_deps": ["./Local", "../Shared", "vendor/Framework"]},
    {"swiftc_path": "swiftc", "env": {}},
    [{"module_name": "Local", "label_id": "apps/client/Local"},
     {"module_name": "Shared", "label_id": "apps/Shared"},
     {"framework_module_name": "Framework", "label_id": "vendor/Framework"},
     {"module_name": "Inferred", "label_id": "apps/client/Inferred"}],
    argv = ["swiftc", "Main.swift"], inputs = ["Main.swift"], outputs = ["Main.o"], identifier = "main",
)
result = True
"#,
    );
    result.unwrap();
    let Some(DeclaredActionOperation::ExpandActions { args, .. }) = &store.actions[1].operation
    else {
        panic!("missing expansion")
    };
    assert_eq!(
        args["direct_modules"],
        serde_json::json!({
            "Local": "apps/client/Local", "Shared": "apps/Shared", "Framework": "vendor/Framework"
        })
    );
}

#[test]
fn bridging_headers_outside_identified_roots_are_rejected() {
    let mut scan = scan_fixture();
    scan["modules"][1]["details"]["swift"]["bridgingHeader"] =
        serde_json::json!({"sourceFiles": ["/untracked/Bridge.h"]});
    assert!(plan(&scan, "off", "{}")
        .1
        .unwrap()
        .contains("untracked_host_module_input"));
}

#[test]
fn scanner_cache_inputs_are_rejected_instead_of_remapped_to_empty_scratch() {
    let mut scan = scan_fixture();
    scan["modules"][3]["details"]["swift"]["moduleInterfacePath"] =
        serde_json::json!(".once/out/modules/test/Materialized.swiftinterface");
    let (store, result) = plan(&scan, "off", "{}");
    assert!(result.unwrap().contains("unsupported_module_scan_artifact"));
    assert!(store.actions.is_empty());

    let mut scan = scan_fixture();
    scan["modules"][5]["details"]["clang"]["commandLine"] = serde_json::json!([
        "-frontend",
        "-emit-pcm",
        "-I.once/out/modules/test/generated"
    ]);
    assert!(plan(&scan, "off", "{}")
        .1
        .unwrap()
        .contains("unsupported_module_scan_artifact"));
}

#[test]
fn absolute_scanner_outputs_are_normalized_before_remapping() {
    let mut scan = scan_fixture().to_string();
    scan = scan.replace(
        ".once/out/modules/test/",
        "/workspace/.once/out/modules/test/",
    );
    let (store, result) = plan(&serde_json::from_str(&scan).unwrap(), "off", "{}");
    result.unwrap();
    assert!(store.actions[1].argv.contains(&format!(
        "-fmodule-file=Native={}",
        store.actions[0].outputs[0]
    )));
}
