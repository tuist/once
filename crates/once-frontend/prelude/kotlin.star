def _kotlin_apple_sources(ctx):
    return _filter_by_extensions(glob(ctx["srcs"]), [".kt", ".kts"])

def _kotlin_apple_attr(attrs, key, default):
    value = attrs.get(key)
    if value == None:
        return default
    return value

def _kotlin_apple_default_target(platform, sdk_variant, arch):
    if platform == "ios":
        if sdk_variant == "device":
            if arch == "arm64":
                return "ios_arm64"
        elif arch == "arm64":
            return "ios_simulator_arm64"
        elif arch == "x86_64":
            return "ios_x64"
    if platform == "macos" or platform == "macosx":
        if arch == "arm64":
            return "macos_arm64"
        if arch == "x86_64":
            return "macos_x64"
    if platform == "tvos":
        if sdk_variant == "device" and arch == "arm64":
            return "tvos_arm64"
        if sdk_variant != "device" and arch == "arm64":
            return "tvos_simulator_arm64"
        if sdk_variant != "device" and arch == "x86_64":
            return "tvos_x64"
    if platform == "watchos":
        if sdk_variant == "device" and arch == "arm64":
            return "watchos_arm64"
        if sdk_variant != "device" and arch == "arm64":
            return "watchos_simulator_arm64"
        if sdk_variant != "device" and arch == "x86_64":
            return "watchos_x64"
    fail("unsupported Kotlin/Native Apple target for platform `" + platform + "`, sdk_variant `" + sdk_variant + "`, arch `" + arch + "`")

def _kotlin_apple_tool(ctx, attrs):
    configured = _kotlin_apple_attr(attrs, "kotlinc_native", "")
    if configured:
        tool = configured
    else:
        kotlin_home = _kotlin_apple_attr(attrs, "kotlin_home", "")
        if kotlin_home:
            tool = kotlin_home + "/bin/kotlinc-native"
        else:
            tool = host_which("kotlinc-native")
    version = host_command([tool, "-version"]).strip()
    identity = "once.kotlin.apple.native.v1\x00" + tool + "\x00" + version
    return (tool, identity)

def _kotlin_apple_trim_framework_suffix(path):
    suffix = ".framework"
    if _ends_with(path, suffix):
        return path[:len(path) - len(suffix)]
    return path

def _kotlin_apple_java_env(attrs):
    env = {}
    java_home = _kotlin_apple_attr(attrs, "java_home", "") or host_env("JAVA_HOME")
    if java_home:
        env["JAVA_HOME"] = java_home
        env["PATH"] = java_home + "/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    return env

def _kotlin_apple_framework_impl(ctx):
    attrs = ctx["attr"]
    platform = _kotlin_apple_attr(attrs, "platform", "")
    if not platform:
        fail(ctx["label"]["id"] + ": `platform` is required")
    sdk_variant = _kotlin_apple_attr(attrs, "sdk_variant", "simulator")
    arch = _kotlin_apple_attr(attrs, "arch", "") or host_arch()
    target = _kotlin_apple_attr(attrs, "target", "") or _kotlin_apple_default_target(platform, sdk_variant, arch)
    product_name = _kotlin_apple_attr(attrs, "product_name", "") or ctx["label"]["name"]
    module_name = _kotlin_apple_attr(attrs, "module_name", "") or product_name
    compiler_opts = _kotlin_apple_attr(attrs, "compiler_opts", [])
    konan_data_dir = _kotlin_apple_attr(attrs, "konan_data_dir", "")

    sources = _kotlin_apple_sources(ctx)
    if len(sources) == 0:
        fail("kotlin_apple_framework " + ctx["label"]["id"] + " has no Kotlin sources (.kt/.kts)")

    tool, identity = _kotlin_apple_tool(ctx, attrs)
    framework_dir = product_name + ".framework"
    framework_path = ctx["build_dir"] + "/" + framework_dir
    framework_binary = declare_output(framework_dir + "/" + product_name)
    framework_header = declare_output(framework_dir + "/Headers/" + product_name + ".h")
    framework_modulemap = declare_output(framework_dir + "/Modules/module.modulemap")
    framework_info_plist = declare_output(framework_dir + "/Info.plist")
    framework_files = [
        framework_binary,
        framework_header,
        framework_modulemap,
        framework_info_plist,
    ]
    output_base = _kotlin_apple_trim_framework_suffix(framework_path)
    compile_env = _kotlin_apple_java_env(attrs)
    if konan_data_dir:
        compile_env["KONAN_DATA_DIR"] = konan_data_dir
    argv = [
        tool,
        "-target", target,
        "-produce", "framework",
        "-module-name", module_name,
        "-o", output_base,
    ] + compiler_opts + sources

    prepare_path(framework_path, kind = "remove", identifier = "kotlin_apple_framework_clean:" + ctx["label"]["id"])

    run_action(
        argv = argv,
        inputs = sources,
        outputs = framework_files,
        env = compile_env,
        toolchain_identity = identity + "\x00target\x00" + target + "\x00module\x00" + module_name,
        identifier = "kotlin_apple_framework:" + ctx["label"]["id"],
    )

    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "kotlin_apple_framework",
        "platform": platform,
        "sdk_variant": sdk_variant,
        "arch": arch,
        "target": target,
        "framework_path": ctx["build_dir"] + "/" + framework_dir,
        "framework_module_name": module_name,
        "framework_files": framework_files,
        "transitive_frameworks": [ctx["build_dir"] + "/" + framework_dir],
        "affected_inputs": sources,
    }

kotlin_apple_framework = target_kind(
    docs = "Compiles Kotlin/Native sources into an Apple framework bundle that Apple app and test targets can import and embed.",
    attrs = [
        attr("platform", "string", required = True, docs = "Apple platform such as `ios`, `macos`, `tvos`, or `watchos`.", configurable = False),
        attr("sdk_variant", "string", default = "\"simulator\"", docs = "`simulator` or `device`; ignored for macOS target inference.", configurable = False),
        attr("arch", "string", docs = "Target architecture used for Kotlin/Native target inference. Defaults to the host arch.", configurable = False),
        attr("target", "string", docs = "Kotlin/Native target, such as `ios_arm64`, `ios_simulator_arm64`, or `macos_arm64`. Defaults from platform, sdk_variant, and arch.", configurable = False),
        attr("product_name", "string", docs = "Framework product name. Defaults to the target name.", configurable = False),
        attr("module_name", "string", docs = "Swift/ObjC module name exported by the framework. Defaults to product_name.", configurable = False),
        attr("kotlinc_native", "string", docs = "Override kotlinc-native path.", configurable = False),
        attr("kotlin_home", "string", docs = "Kotlin/Native installation root used to find bin/kotlinc-native.", configurable = False),
        attr("java_home", "string", docs = "Java runtime home exposed to kotlinc-native. Defaults to JAVA_HOME when available.", configurable = False),
        attr("konan_data_dir", "string", docs = "Optional Kotlin/Native cache directory exposed as KONAN_DATA_DIR.", configurable = False),
        attr("compiler_opts", "list<string>", default = "[]", docs = "Additional kotlinc-native arguments appended after Once-managed flags.", configurable = False),
    ],
    deps = [],
    providers = ["kotlin_native_framework", "apple_framework", "apple_bundle", "native_linkable"],
    capabilities = [capability("build", ["default", "framework"])],
    examples = [
        example(
            "kotlin-apple-framework-minimal",
            name = "Minimal Kotlin Apple framework",
            use_when = "Use this when shared Kotlin code should be imported from Swift or Objective-C on Apple platforms.",
        ),
        example(
            "native-mobile-shared-code-e2e",
            name = "Shared mobile code app graph",
            use_when = "Use this when an Android app should package Swift and Rust native libraries alongside an Apple app that embeds Kotlin and links Rust.",
        ),
    ],
    impl = _kotlin_apple_framework_impl,
)
