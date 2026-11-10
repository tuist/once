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
def host_path_is_within(path, root):
    return path == root or path.startswith(root + "/")
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
fn swift_package_resolution_accepts_an_ios_only_linux_sdk() {
    let source = format!(
        "{}\n{}\n{LINUX_HOST}\n{}",
        xcode_prelude_source(),
        include_str!("../../prelude/swift_package.star"),
        r#"
def _swiftpm_absolute_package_path(ctx, path):
    return "/workspace"
def _swift_package_metadata(argv, absolute, env = {}, check = True):
    return '{"name":"Root","dependencies":[],"products":[],"targets":[]}'
def _xcode_local_swift_package_specs(ctx, packages, platform, minimum_os, variant, root_identities = None):
    return {"specs": [], "products": {}, "modules": {}}
result = repr(_swift_package_workspace_resolver({
    "attrs": {"platform": "ios", "sdk_variant": "device"},
    "files": {"Package.swift": "manifest"},
    "label": {"id": "root", "name": "root", "package": ""},
}))
"#,
    );
    assert_eq!(
        eval_prelude_source_to_repr(source).unwrap(),
        r#"{"targets": [], "roots": [], "attrs": {"package_name": "Root", "_display_name": "Root", "_default_test_roots": []}}"#
    );
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
fn sdk_bundle_identity_tracks_sibling_resources_without_hashing_partial_link_trees() {
    let result = evaluate(
        r#"
def host_tree_sha256(path):
    if path != "/sdk":
        fail("SDK subtrees contain links to sibling resources: " + path)
    return "first-bundle"
first = _resolve_swiftc("ios", "device", "")
def host_tree_sha256(path):
    if path != "/sdk":
        fail("SDK subtrees contain links to sibling resources: " + path)
    return "changed-sibling-framework"
second = _resolve_swiftc("ios", "device", "")
result = repr([first["identity"] != second["identity"], _resolve_actool("")])
"#,
    )
    .unwrap();
    assert!(result.starts_with("[True,"), "{result}");
    assert!(
        result.contains("iPhoneOS.platform/Developer/usr/bin/actool"),
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
    assert!(application.argv.windows(4).any(|args| args
        == [
            "-Xclang-linker",
            "-resource-dir",
            "-Xclang-linker",
            "/sdk/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/clang",
        ]));
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

const WORKER_TOOLCHAIN: &str = r#"
worker = {
    "schema_version": 1,
    "environment": "immutable-image-a",
    "os": "linux",
    "arch": "arm64",
    "bin": "/worker/bin",
    "developer_dir": "/worker/sdk/Developer",
    "tools": "/worker/sdk/toolset/bin",
    "env": {"LD_LIBRARY_PATH": "/worker/compat"},
    "programs": {"rcodesign": "/worker/bin/rcodesign"},
    "sdks": {"iphonesimulator": {
        "sdk_path": "/worker/sdk/Simulator.sdk",
        "resource_dir": "/worker/sdk/swift",
    }},
}
execution = {"provider": "workers", "executor": "docker", "environment": "immutable-image-a", "properties": {"apple": worker}}
def execution_context():
    return execution
def host_os():
    return "macos"
def host_arch():
    return "x86_64"
def host_which(name):
    fail("must not discover worker tools on the coordinator: " + name)
def host_which_optional(name):
    fail("must not discover optional worker tools on the coordinator: " + name)
def host_command(argv, env = None, merge_stderr = None):
    fail("must not execute worker commands on the coordinator: " + str(argv))
def host_env(name):
    fail("must not inherit coordinator environment: " + name)
def host_file_sha256(path):
    fail("must not hash worker files on the coordinator: " + path)
def host_tree_sha256(path):
    fail("must not hash worker trees on the coordinator: " + path)
def host_file_exists(path):
    fail("must not inspect worker files on the coordinator: " + path)
def host_path_exists(path):
    fail("must not inspect worker paths on the coordinator: " + path)
"#;

#[test]
fn mac_coordinator_declares_linux_actions_with_dependency_inputs_without_host_probes() {
    let workspace = TempDir::new().unwrap();
    let source = format!(
        r#"{}
{WORKER_TOOLCHAIN}
def glob(patterns):
    return ["Sources/Value.swift", "Sources/Value.c"]
ctx = {{"label": {{"package": "test", "name": "Core", "id": "test/Core"}}, "attr": {{"platform": "ios", "sdk_variant": "simulator", "minimum_os": "17.0"}}, "srcs": ["Sources/*.swift"], "deps": [], "build_dir": ".once/out/test", "scratch_dir": ".once/tmp/test", "capability": "build"}}
core = _apple_library_impl(ctx)
ctx["label"] = {{"package": "test", "name": "Hello", "id": "test/Hello"}}
ctx["attr"]["bundle_id"] = "dev.once.hello"
ctx["deps"] = [core]
result = _apple_application_impl(ctx)
"#,
        apple_prelude_source(),
    );
    let (store, result) = with_active_store(store_for(workspace.path(), "test"), || {
        Module::with_temp_heap(|module| {
            let ast = AstModule::parse("worker.star", source, &Dialect::Standard)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            Evaluator::new(&module)
                .eval_module(ast, &globals_for_prelude())
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            anyhow::Ok(())
        })
    });
    result.unwrap();
    let clang = action_by_identifier(&store, "clang_compile_Core_Sources_Value.c");
    assert_eq!(clang.argv[0], "/worker/bin/clang");
    assert!(clang.inputs.iter().any(|path| path == "Sources/Value.c"));
    assert!(clang
        .argv
        .iter()
        .any(|arg| arg == "arm64-apple-ios17.0-simulator"));
    let application = action_by_identifier(&store, "apple_application_compile_Hello");
    assert_eq!(application.argv[0], "/worker/bin/swiftc");
    assert!(application
        .inputs
        .iter()
        .any(|path| path.ends_with("Core.a")));
    assert!(application
        .inputs
        .iter()
        .any(|path| path.ends_with("Core.swiftmodule")));
    assert_eq!(
        application.env.get("LD_LIBRARY_PATH").unwrap(),
        "/worker/compat"
    );
    assert!(application
        .argv
        .iter()
        .any(|arg| arg == "arm64-apple-ios17.0-simulator"));
    assert_eq!(
        action_by_identifier(&store, "apple_application_codesign_Hello").argv[0],
        "/worker/bin/rcodesign"
    );
}

#[test]
fn worker_selection_is_bound_to_the_image_and_rejects_unsupported_features() {
    let evaluate = |source: &str| {
        eval_prelude_source_to_repr(format!(
            "{}\n{WORKER_TOOLCHAIN}\n{source}",
            apple_prelude_source()
        ))
    };
    assert_eq!(
        evaluate("execution = {'provider': 'local'}\nresult = repr(_apple_execution_toolchain())")
            .unwrap(),
        "None",
    );
    let result = evaluate(
        r#"
first = _resolve_swiftc("ios", "simulator", "")["identity"]
worker["env"]["LD_LIBRARY_PATH"] = "/worker/changed"
second = _resolve_swiftc("ios", "simulator", "")["identity"]
result = repr(first != second)
"#,
    )
    .unwrap();
    assert_eq!(result, "True");
    for (source, expected) in [
        ("execution['environment'] = 'other-image'\nresult = repr(_resolve_swiftc('ios', 'simulator', ''))", "must match the selected execution environment"),
        ("execution['properties'] = {}\nresult = repr(_resolve_swiftc('ios', 'simulator', ''))", "image-bound Linux toolchain description"),
        ("result = repr(_resolve_swiftc('ios', 'device', ''))", "does not provide iphoneos"),
        ("result = repr(_resolve_swiftc('ios', 'simulator', '/Applications/Xcode.app/Contents/Developer'))", "must match the selected Apple worker"),
        ("result = repr(_resolve_attrs({}, {'explicit_modules': True}, 'Core', []))", "set explicit_modules = false"),
        ("result = repr(_swift_macro_impl({'label': {'id': 'Macros'}}))", "binary_swift_plugins"),
    ] {
        let error = evaluate(source).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
}

const DEVELOPMENT_PROFILE: &str = r#"
PROFILE = """<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Platform</key><array><string>iOS</string></array>
<key>TeamIdentifier</key><array><string>TEAM123456</string></array>
<key>Entitlements</key><dict>
<key>application-identifier</key><string>TEAM123456.dev.once.*</string>
<key>com.apple.developer.team-identifier</key><string>TEAM123456</string>
<key>get-task-allow</key><true/>
<key>keychain-access-groups</key><array><string>TEAM123456.*</string></array>
<key>com.apple.developer.associated-domains</key><string>*</string>
</dict></dict></plist>"""
def host_file_cms_content(path):
    if not path.endswith(".mobileprovision"):
        fail("profiles are read through CMS: " + path)
    return PROFILE
def host_file_sha256(path):
    return "sha256-of-key-material" if path.endswith(".p12") or path.endswith(".pem") else "file:" + path
def host_file_exists(path):
    return "missing" not in path
def host_file_read(path):
    if path.endswith("App.entitlements"):
        return ENTITLEMENTS
    fail("unexpected read: " + path)
ENTITLEMENTS = """<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>keychain-access-groups</key><array><string>$(AppIdentifierPrefix)dev.once.hello</string></array>
<key>com.apple.developer.associated-domains</key><array><string>applinks:once.dev</string></array>
</dict></plist>"""
"#;

fn development_application_store(
    workspace: &std::path::Path,
    setup: &str,
) -> (AnalysisStore, anyhow::Result<()>) {
    let profile = workspace.join("Dev.mobileprovision");
    std::fs::write(&profile, b"cms bytes").unwrap();
    let profile = profile.display();
    let source = format!(
        r#"{}
{LINUX_HOST}
{DEVELOPMENT_PROFILE}
def glob(patterns):
    return ["Sources/App.swift"]
framework = {{"label_id": "test/Fw", "target_kind": "apple_framework", "transitive_framework_bundles": [{{"path": ".once/out/fw/Fw.framework", "files": [".once/out/fw/Fw.framework"], "linkage": "dynamic"}}]}}
ctx = {{"label": {{"package": "test", "name": "Hello", "id": "test/Hello"}}, "attr": {{"platform": "ios", "sdk_variant": "device", "minimum_os": "17.0", "bundle_id": "dev.once.hello", "signing": "development", "signing_identity": "/keys/identity.p12", "signing_identity_password_file": "/keys/password", "provisioning_profile": "{profile}"}}, "srcs": ["Sources/*.swift"], "deps": [framework], "build_dir": ".once/out/test", "scratch_dir": ".once/tmp/test", "capability": "build"}}
{setup}
result = _apple_application_impl(ctx)
"#,
        apple_prelude_source()
    );
    with_active_store(store_for(workspace, "test"), || {
        Module::with_temp_heap(|module| {
            let ast = AstModule::parse("signing.star", source, &Dialect::Standard)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            Evaluator::new(&module)
                .eval_module(ast, &globals_for_prelude())
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            anyhow::Ok(())
        })
    })
}

#[test]
fn development_signing_embeds_the_profile_and_signs_nested_code_with_one_identity() {
    let workspace = TempDir::new().unwrap();
    let (store, result) = development_application_store(
        workspace.path(),
        r#"ctx["attr"]["entitlements"] = "App.entitlements""#,
    );
    result.unwrap();

    let entitlements = store
        .actions
        .iter()
        .find_map(|action| match &action.operation {
            Some(DeclaredActionOperation::WriteFile { path, bytes })
                if path.ends_with("processed-entitlements.plist") =>
            {
                Some(String::from_utf8(bytes.clone()).unwrap())
            }
            _ => None,
        })
        .expect("processed entitlements");
    for expected in [
        "<key>application-identifier</key>\n\t<string>TEAM123456.dev.once.hello</string>",
        "<key>com.apple.developer.team-identifier</key>\n\t<string>TEAM123456</string>",
        "<key>get-task-allow</key>\n\t<true/>",
        "<string>TEAM123456.dev.once.hello</string>\n\t</array>",
        "<string>applinks:once.dev</string>",
    ] {
        assert!(
            entitlements.contains(expected),
            "missing {expected}: {entitlements}"
        );
    }

    let profile = store
        .actions
        .iter()
        .find_map(|action| match &action.operation {
            Some(DeclaredActionOperation::MaterializeHostFile { destination, .. })
                if destination.ends_with("Hello.app/embedded.mobileprovision") =>
            {
                Some(destination.clone())
            }
            _ => None,
        })
        .expect("embedded provisioning profile");

    let identity_args = [
        "--p12-file",
        "/keys/identity.p12",
        "--p12-password-file",
        "/keys/password",
    ];
    let app = action_by_identifier(&store, "apple_application_codesign_Hello");
    assert!(
        app.argv.windows(4).any(|args| args == identity_args),
        "{:?}",
        app.argv
    );
    assert!(app
        .argv
        .iter()
        .any(|arg| arg.ends_with("processed-entitlements.plist")));
    assert!(app.inputs.contains(&profile), "{:?}", app.inputs);
    assert!(app
        .inputs
        .iter()
        .any(|input| input.ends_with("Frameworks/Fw.framework")));
    assert!(app
        .toolchain_identity
        .as_deref()
        .unwrap()
        .contains("sha256-of-key-material"));
    let framework = action_by_identifier(&store, "apple_application_embed_Fw.framework");
    assert!(framework.argv.windows(4).any(|args| args == identity_args));
    assert!(!framework
        .argv
        .iter()
        .any(|arg| arg.contains("entitlements")));
    assert_eq!(framework.toolchain_identity, app.toolchain_identity);

    for action in &store.actions {
        for path in action.inputs.iter().chain(&action.outputs) {
            assert!(
                !path.starts_with("/keys/"),
                "key material became an action path: {path}"
            );
        }
        assert!(action.env.values().all(|value| !value.contains("/keys/")));
        assert!(!action
            .toolchain_identity
            .as_deref()
            .unwrap_or_default()
            .contains("/keys/password"));
    }
}

#[test]
fn development_signing_rejects_what_the_profile_does_not_allow() {
    for (attrs, expected) in [
        (r#""bundle_id": "com.example.hello""#, "allows application identifier `TEAM123456.dev.once.*`, not `TEAM123456.com.example.hello`"),
        (r#""development_team": "OTHERTEAM1""#, "does not match provisioning profile team `TEAM123456`"),
        (r#""sdk_variant": "simulator""#, "set sdk_variant = \"device\""),
        (r#""signing_identity": "Apple Development: Once (TEAM123456)", "signing_identity_password_file": """#, "keychain signing identities need macOS codesign"),
        (r#""signing_identity": "/keys/identity.pem""#, "only applies to .p12 or .pfx identities"),
        (r#""provisioning_profile": """#, "requires signing_identity"),
        (r#""signing": "distribution""#, "must be `ad_hoc` or `development`"),
        (r#""signing_identity_password_file": """#, "point it at an empty file"),
        (r#""signing_identity": "/keys/missing""#, "signing identity file `/keys/missing` does not exist"),
    ] {
        let workspace = TempDir::new().unwrap();
        let setup = format!("ctx[\"attr\"].update({{{attrs}}})");
        let error = development_application_store(workspace.path(), &setup)
            .1
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{attrs}: {error}");
    }

    let workspace = TempDir::new().unwrap();
    let error = development_application_store(
        workspace.path(),
        r#"ctx["deps"].append({"label_id": "test/Widget", "application_extension": True, "app_path": ".once/out/w/Widget.appex", "signing": "ad_hoc"})"#,
    )
    .1
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("application extension `test/Widget` is ad hoc signed"),
        "{error}"
    );

    let error = eval_prelude_source_to_repr(format!(
        r#"{}
{LINUX_HOST}
ctx = {{"label": {{"package": "test", "name": "Hello", "id": "test/Hello"}}, "attr": {{"platform": "ios", "bundle_id": "dev.once.hello", "signing_identity": "/keys/identity.p12"}}}}
result = repr(_apple_signing(ctx, ctx["attr"], "ios", "device"))
"#,
        apple_prelude_source()
    ))
    .unwrap_err();
    assert!(
        error.contains("require signing = \\\"development\\\""),
        "{error}"
    );
}

#[test]
fn development_entitlements_follow_the_profile_on_each_platform() {
    let evaluate_on = |setup: &str, platform: &str, entitlements: &str| {
        evaluate(&format!(
            r#"{DEVELOPMENT_PROFILE}
{setup}
ctx = {{"label": {{"id": "test/Hello"}}}}
signing = {{"profile": "/p/Dev.mobileprovision"}}
result = _apple_development_entitlements(ctx, signing, "{platform}", "dev.once.hello", "", """{entitlements}""")
"#
        ))
    };
    let evaluate = |entitlements: &str| evaluate_on("", "ios", entitlements);
    let plist = |body: &str| format!("<plist version=\"1.0\"><dict>{body}</dict></plist>");
    for (body, expected) in [
        (
            "<key>aps-environment</key><string>development</string>",
            "entitlement `aps-environment` is not granted",
        ),
        (
            "<key>keychain-access-groups</key><array><string>OTHERTEAM1.shared</string></array>",
            "is outside what provisioning profile",
        ),
        (
            "<key>application-identifier</key><string>TEAM123456.dev.once.other</string>",
            "requires `TEAM123456.dev.once.hello`",
        ),
    ] {
        let error = evaluate(&plist(body)).unwrap_err();
        assert!(error.contains(expected), "{body}: {error}");
    }
    assert!(evaluate(&plist("<key>get-task-allow</key><false/>"))
        .unwrap()
        .contains("<false/>"));
    let error = evaluate(&plist("<key>com.apple.security.app-sandbox</key><true/>")).unwrap_err();
    assert!(error.contains("is not granted"), "{error}");
    let error = evaluate_on("", "tvos", "").unwrap_err();
    assert!(error.contains("is for iOS, not tvos"), "{error}");

    let macos = r#"PROFILE = PROFILE.replace("<string>iOS</string>", "<string>OSX</string>").replace("<key>application-identifier</key>", "<key>com.apple.application-identifier</key>")
"#;
    let result = evaluate_on(
        macos,
        "macos",
        &plist("<key>com.apple.security.app-sandbox</key><true/><key>com.apple.security.network.client</key><true/>"),
    )
    .unwrap();
    for expected in [
        "<key>com.apple.application-identifier</key>",
        "<key>com.apple.security.app-sandbox</key>",
        "<key>com.apple.security.network.client</key>",
    ] {
        assert!(result.contains(expected), "missing {expected}: {result}");
    }
    let error = evaluate_on(
        macos,
        "macos",
        &plist("<key>com.apple.developer.aps-environment</key><string>development</string>"),
    )
    .unwrap_err();
    assert!(error.contains("is not granted"), "{error}");
}

#[test]
fn keychain_identities_sign_with_macos_codesign_and_files_with_rcodesign() {
    let result = evaluate(
        r#"
keychain = {"mode": "development", "profile": "/p", "keychain_identity": "Apple Development: Once (TEAM123456)"}
macos = {"codesign_path": "/usr/bin/codesign", "identity": "once.apple.codesign.v1", "env": {}}
pem = {"mode": "development", "profile": "/p", "keychain_identity": "", "identity_file": "/keys/identity.pem", "identity_format": "pem", "identity_digest": "digest", "password_file": ""}
def host_which_optional(name):
    return "/opt/bin/rcodesign" if name == "rcodesign" else ""
def host_which(name):
    return "/usr/bin/" + name
def host_command(argv, env = None, merge_stderr = None):
    if argv != ["/usr/bin/security", "find-identity", "-v", "-p", "codesigning"]:
        fail("unexpected command: " + str(argv))
    return IDENTITIES
IDENTITIES = """  1) 1111111111111111111111111111111111111111 "Apple Development: Once (TEAM123456)"
  2) 2222222222222222222222222222222222222222 "Apple Development: Other (TEAM123456)"
     2 valid identities found
"""
first = _apple_codesign_with_signing(macos, keychain, "test/Hello")
IDENTITIES = IDENTITIES.replace("1111111111111111111111111111111111111111", "3333333333333333333333333333333333333333")
renewed = _apple_codesign_with_signing(macos, keychain, "test/Hello")
by_fingerprint = _apple_codesign_with_signing(macos, dict(keychain, keychain_identity = "2222222222222222222222222222222222222222"), "test/Hello")
result = repr([
    first["identity"] != renewed["identity"],
    by_fingerprint["sign_identity"],
    _apple_codesign_argv(renewed, "Hello.app", "e.plist"),
    _apple_codesign_argv(_apple_codesign_with_signing(macos, pem, "test/Hello"), "Hello.app"),
])
"#,
    )
    .unwrap();
    assert!(
        result.starts_with(r#"[True, "2222222222222222222222222222222222222222", ["/usr/bin/codesign", "--force", "--sign", "3333333333333333333333333333333333333333", "--timestamp=none", "--entitlements", "e.plist", "Hello.app"]"#),
        "{result}"
    );
    assert!(
        result.contains(r#"["/opt/bin/rcodesign", "sign", "--shallow", "--timestamp-url", "none", "--pem-file", "/keys/identity.pem", "Hello.app"]"#),
        "{result}"
    );
}

#[test]
fn unknown_or_ambiguous_keychain_identities_list_the_available_fingerprints() {
    for (selector, expected) in [
        ("Apple Development: Nobody", "no valid code signing identity `Apple Development: Nobody` in the keychain; set signing_identity to one of `Apple Development: Once (TEAM123456)` (1111111111111111111111111111111111111111)"),
        ("Apple Development: Once (TEAM123456)", "several keychain identities are named"),
    ] {
        let error = evaluate(&format!(
            r#"
def host_which(name):
    return "/usr/bin/" + name
def host_command(argv, env = None, merge_stderr = None):
    return """  1) 1111111111111111111111111111111111111111 "Apple Development: Once (TEAM123456)"
  2) 4444444444444444444444444444444444444444 "Apple Development: Once (TEAM123456)"
     2 valid identities found
""" if "Nobody" not in "{selector}" else """  1) 1111111111111111111111111111111111111111 "Apple Development: Once (TEAM123456)"
"""
result = repr(_apple_keychain_fingerprint("{selector}", "test/Hello"))
"#
        ))
        .unwrap_err();
        assert!(error.contains(expected), "{selector}: {error}");
    }
}

#[test]
fn entries_an_earlier_build_left_in_the_bundle_are_removed_before_signing() {
    let workspace = TempDir::new().unwrap();
    let app = workspace.path().join(".once/out/test/Hello.app");
    for file in [
        "Hello",
        "Info.plist",
        "embedded.mobileprovision",
        "_CodeSignature/CodeResources",
        "Frameworks/Fw.framework/Fw",
        "Frameworks/Old.framework/Old",
        "PlugIns/Old.appex/Info.plist",
        "removed.txt",
    ] {
        let path = app.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"").unwrap();
    }
    let removed = |store: &AnalysisStore| {
        store
            .actions
            .iter()
            .enumerate()
            .filter_map(|(index, action)| match &action.operation {
                Some(DeclaredActionOperation::PreparePath { path, .. }) => Some((
                    index,
                    path.trim_start_matches(".once/out/test/Hello.app/")
                        .to_string(),
                )),
                _ => None,
            })
            .collect::<Vec<_>>()
    };

    let (store, result) = development_application_store(workspace.path(), "");
    result.unwrap();
    let entries = removed(&store);
    let mut paths = entries
        .iter()
        .map(|(_, path)| path.as_str())
        .collect::<Vec<_>>();
    paths.sort_unstable();
    assert_eq!(
        paths,
        ["Frameworks/Old.framework", "PlugIns", "removed.txt"]
    );
    let sign = store
        .actions
        .iter()
        .position(|action| action.identifier.as_deref() == Some("apple_application_codesign_Hello"))
        .unwrap();
    assert!(entries.iter().all(|(index, _)| *index < sign));

    let (store, result) = development_application_store(
        workspace.path(),
        r#"ctx["attr"].update({"signing": "ad_hoc", "signing_identity": "", "signing_identity_password_file": "", "provisioning_profile": ""})"#,
    );
    result.unwrap();
    assert!(removed(&store)
        .iter()
        .any(|(_, path)| path == "embedded.mobileprovision"));

    let (store, result) = development_application_store(
        workspace.path(),
        r#"ctx["attr"]["postbuild_actions"] = ['{"identifier": "stamp", "argv": ["/bin/true"], "inputs": [], "outputs": []}']"#,
    );
    result.unwrap();
    assert!(removed(&store).is_empty());
}
