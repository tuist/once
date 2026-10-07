use super::*;

const LINUX_HOST: &str = r#"
def host_os():
    return "linux"
def host_arch():
    return "x86_64"
def host_which(name):
    if name in ["swift", "rcodesign"]:
        return "/host/bin/" + name
    fail("unexpected host tool: " + name)
def host_which_optional(name):
    return ""
def host_env(name):
    return ""
def host_path_exists(path):
    return True
def host_file_exists(path):
    return True
def host_tree_sha256(path):
    return "tree:" + path
def host_file_sha256(path):
    return "file:" + path
def host_command(argv, env = None, merge_stderr = None):
    if argv == ["/host/bin/swift", "-print-target-info"]:
        return '{"paths":{"runtimeResourcePath":"/host/lib/swift"}}'
    if argv == ["/host/bin/swift", "sdk", "list"]:
        return " darwin\n"
    if len(argv) > 3 and argv[:4] == ["/host/bin/swift", "sdk", "configure", "darwin"]:
        if argv[4] == "arm64-apple-ios":
            platform = "iPhoneOS"
        elif argv[4] == "x86_64-apple-ios-simulator":
            platform = "iPhoneSimulator"
        else:
            fail("unexpected SDK triple: " + argv[4])
        return "sdkRootPath: /sdk/Developer/Platforms/" + platform + ".platform/Developer/SDKs/" + platform + "27.0.sdk\nswiftResourcesPath: /sdk/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift\n"
    if "--version" in argv:
        return "Swift toolchain version test"
    fail("unexpected command: " + str(argv))
"#;

fn evaluate(source: &str) -> Result<String, String> {
    eval_prelude_source_to_repr(format!(
        "{}\n{LINUX_HOST}\n{source}",
        apple_prelude_source()
    ))
}

#[test]
fn discovers_registered_darwin_sdk_and_matching_host_compilers_without_xcrun() {
    let result = evaluate(
        r#"
swiftc = _resolve_swiftc("ios", "device", "")
clang = _resolve_clang("ios", "device", "")
result = repr([swiftc, clang, _resolve_libtool("ios", "device", ""), _resolve_lipo("ios", "device", "")])
"#,
    )
    .unwrap();
    for expected in [
        "/host/bin/swiftc",
        "/host/bin/clang",
        "/host/bin/clang++",
        "iPhoneOS27.0.sdk",
        "-resource-dir",
        "-tools-directory",
        "-use-ld=lld",
        "-enable-cross-import-overlays",
        "-external-plugin-path",
        "/sdk/toolset/bin/libtool",
        "/sdk/toolset/bin/lipo",
        "/sdk/toolset/bin:/host/bin:/usr/bin:/bin",
    ] {
        assert!(result.contains(expected), "missing {expected}: {result}");
    }
    assert!(!result.contains("xcrun"));
}

#[test]
fn explicit_sdk_uses_metadata_instead_of_unversioned_xcode_paths() {
    let result = evaluate(
        r#"
def host_file_read(path):
    if path != "/pinned/swift-sdk.json":
        fail("unexpected file: " + path)
    return '{"schemaVersion":"4.0","targetTriples":{"arm64-apple-ios":{"sdkRootPath":"Developer/Platforms/iPhoneOS.platform/Developer/SDKs/iPhoneOS26.5.sdk","swiftResourcesPath":"Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift"}}}'
result = repr(_resolve_swiftc("ios", "device", "/pinned/Developer"))
"#,
    )
    .unwrap();
    assert!(result
        .contains("/pinned/Developer/Platforms/iPhoneOS.platform/Developer/SDKs/iPhoneOS26.5.sdk"));
    assert!(result.contains("/pinned/toolset/bin"));
}

#[test]
fn simulator_selection_preserves_the_requested_destination() {
    let result = evaluate(r#"result = repr(_resolve_swiftc("ios", "simulator", ""))"#).unwrap();
    assert!(result.contains("iPhoneSimulator27.0.sdk"), "{result}");
    assert!(result.contains("iphonesimulator"), "{result}");
    assert!(!result.contains("iPhoneOS27.0.sdk"), "{result}");
}

#[test]
fn llvm_lipo_fallback_and_missing_resource_tools_have_actionable_repairs() {
    let result = evaluate(
        r#"
def host_file_exists(path):
    return not path.endswith("/lipo")
def host_which_optional(name):
    return "/llvm/bin/llvm-lipo" if name == "llvm-lipo" else ""
result = repr(_resolve_lipo("ios", "device", ""))
"#,
    )
    .unwrap();
    assert!(result.contains("/llvm/bin/llvm-lipo"), "{result}");

    let error = evaluate(
        r#"
def host_file_exists(path):
    return not path.endswith("/lipo")
result = repr(_resolve_lipo("ios", "device", ""))
"#,
    )
    .unwrap_err();
    assert!(error.contains("archs = [\"arm64\"]"), "{error}");

    let error = evaluate(
        r#"
def host_file_exists(path):
    return not path.endswith("/actool")
result = repr(_resolve_actool(""))
"#,
    )
    .unwrap_err();
    assert!(error.contains("install-toolchain.sh"), "{error}");
}

#[test]
fn compiler_environment_and_cross_sdk_resources_are_tracked() {
    let result = evaluate(r#"
first = _resolve_swiftc("ios", "device", "")
def host_env(name):
    return "/compat/lib" if name == "LD_LIBRARY_PATH" else ""
second = _resolve_swiftc("ios", "device", "")
result = repr([second["env"], first["identity"] != second["identity"], _apple_module_host_roots(second)])
"#).unwrap();
    assert!(
        result.contains("\"LD_LIBRARY_PATH\": \"/compat/lib\""),
        "{result}"
    );
    assert!(result.contains("True"), "{result}");
    assert!(result.contains("/host/lib"), "{result}");
    assert!(
        result.contains("/sdk/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift"),
        "{result}"
    );
}

#[test]
fn missing_sdk_reports_installation_and_repair_commands() {
    let error = evaluate(
        r#"
def host_command(argv, env = None, merge_stderr = None):
    return "No Swift SDKs are currently installed."
result = repr(_resolve_swiftc("ios", "device", ""))
"#,
    )
    .unwrap_err();
    assert!(
        error.contains("xtool sdk install /path/to/Xcode.xip"),
        "{error}"
    );
    assert!(error.contains("swift sdk list"), "{error}");

    let error = evaluate(
        r#"
def host_path_exists(path):
    return False
result = repr(_resolve_swiftc("ios", "device", ""))
"#,
    )
    .unwrap_err();
    assert!(error.contains("re-install or repair"), "{error}");
}

#[test]
fn resources_use_installed_compatibility_tools_and_signing_uses_rcodesign() {
    let result = evaluate(
        r#"
codesign = _resolve_codesign("")
result = repr([
    _resolve_actool(""), _resolve_momc(""), _resolve_ibtool(""),
    _apple_codesign_argv(codesign, "Hello.app", "entitlements.plist"),
    _apple_codesign_argv({"codesign_path": "/usr/bin/codesign"}, "Hello.app"),
])
"#,
    )
    .unwrap();
    assert!(result.contains("iPhoneOS.platform/Developer/usr/bin/actool"));
    assert!(result.contains("iPhoneOS.platform/Developer/usr/bin/momc"));
    assert!(result.contains("iPhoneOS.platform/Developer/usr/bin/ibtool"));
    assert!(result.contains("\"sign\", \"--shallow\", \"--timestamp-url\", \"none\", \"--entitlements-xml-file\", \"entitlements.plist\", \"Hello.app\""), "{result}");
    assert!(
        result.contains("\"--force\", \"--sign\", \"-\", \"--timestamp=none\", \"Hello.app\""),
        "{result}"
    );
}

#[test]
fn sdk_contents_partition_the_compiler_identity() {
    let result = evaluate(
        r#"
first = _resolve_swiftc("ios", "device", "")["identity"]
def host_tree_sha256(path):
    return "changed:" + path
second = _resolve_swiftc("ios", "device", "")["identity"]
result = repr(first != second)
"#,
    )
    .unwrap();
    assert_eq!(result, "True");
}

#[test]
fn device_architecture_is_not_the_linux_host_architecture() {
    assert_eq!(
        evaluate(r#"result = repr([_apple_default_arch("ios", "device"), _apple_default_arch("ios", "simulator"), _apple_config_tokens({}, {"platform": "ios", "sdk_variant": "device"}, "Hello")])"#).unwrap(),
        r#"["arm64", "x86_64", ["ios", "device", "arm64", "default"]]"#,
    );
}

#[test]
fn declares_device_library_and_application_actions_on_an_x86_linux_host() {
    let workspace = TempDir::new().unwrap();
    let source = format!(
        r#"{}
{LINUX_HOST}
def glob(patterns):
    return ["Sources/Value.swift", "Sources/Value.c"]
ctx = {{"label": {{"package": "test", "name": "Core", "id": "test/Core"}}, "attr": {{"platform": "ios", "sdk_variant": "device", "minimum_os": "17.0"}}, "srcs": ["Sources/*.swift"], "deps": [], "build_dir": ".once/out/test", "scratch_dir": ".once/tmp/test", "capability": "build"}}
core = _apple_library_impl(ctx)
ctx["label"] = {{"package": "test", "name": "Hello", "id": "test/Hello"}}
ctx["attr"]["bundle_id"] = "dev.once.hello"
ctx["deps"] = [core]
result = _apple_application_impl(ctx)
"#,
        apple_prelude_source()
    );
    let (store, result) = with_active_store(store_for(workspace.path(), "test"), || {
        Module::with_temp_heap(|module| {
            let ast = AstModule::parse("linux.star", source, &Dialect::Standard)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            Evaluator::new(&module)
                .eval_module(ast, &globals_for_prelude())
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            anyhow::Ok(())
        })
    });
    result.unwrap();
    let archive = action_by_identifier(&store, "libtool_swift_archive_Core");
    assert_eq!(archive.argv[0], "/sdk/toolset/bin/libtool");
    let clang = action_by_identifier(&store, "clang_compile_Core_Sources_Value.c");
    assert_eq!(clang.argv[0], "/host/bin/clang");
    assert!(clang.argv.iter().any(|arg| arg == "arm64-apple-ios17.0"));
    let application = action_by_identifier(&store, "apple_application_compile_Hello");
    assert_eq!(application.argv[0], "/host/bin/swiftc");
    assert!(application
        .argv
        .iter()
        .any(|arg| arg == "arm64-apple-ios17.0"));
    assert!(application.argv.iter().any(|arg| arg.ends_with("Core.a")));
    let sign = action_by_identifier(&store, "apple_application_codesign_Hello");
    assert_eq!(sign.argv[0], "/host/bin/rcodesign");
    assert_eq!(sign.argv[1], "sign");
    assert!(sign
        .outputs
        .iter()
        .any(|output| output.ends_with("_CodeSignature/CodeResources")));
    assert!(store
        .actions
        .iter()
        .all(|action| !action.argv.iter().any(|arg| arg.contains("xcrun"))));
}

#[test]
fn macos_only_workflows_fail_with_focused_messages() {
    for (call, expected) in [
        (
            "_resolve_attrs({}, {\"mac_catalyst\": True}, \"Catalyst\", [])",
            "set mac_catalyst = false",
        ),
        (
            "_resolve_attrs({}, {\"swift_testing\": True}, \"Testing\", [])",
            "set swift_testing = false",
        ),
        ("_resolve_intentbuilderc(\"\")", "provide generated sources"),
        (
            "_resolve_apple_thinning_tools(\"\")",
            "Apple app thinning requires Xcode on macOS",
        ),
        (
            "_swift_macro_impl({\"label\": {\"id\": \"Macros\"}})",
            "binary_swift_plugins",
        ),
        (
            "_apple_test_bundle_impl({\"label\": {\"id\": \"Tests\"}})",
            "macOS test runtimes",
        ),
    ] {
        let error = evaluate(&format!("result = repr({call})")).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
}
