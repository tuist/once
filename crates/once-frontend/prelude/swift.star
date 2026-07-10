def _swift_android_sources(ctx):
    return _filter_by_extensions(glob(ctx["srcs"]), [".swift"])

def _swift_android_target_for_abi(abi, android_api):
    if abi == "arm64-v8a":
        return "aarch64-unknown-linux-android" + str(android_api)
    if abi == "armeabi-v7a":
        return "armv7-unknown-linux-androideabi" + str(android_api)
    if abi == "x86":
        return "i686-unknown-linux-android" + str(android_api)
    if abi == "x86_64":
        return "x86_64-unknown-linux-android" + str(android_api)
    fail("unsupported Android ABI `" + abi + "` for Swift")

def _swift_android_target_with_api(target, android_api):
    if _ends_with(target, "-android"):
        return target + str(android_api)
    if _ends_with(target, "-androideabi"):
        return target + str(android_api)
    return target

def _swift_android_abi_from_target(target):
    if "android" not in target:
        return ""
    if target.startswith("aarch64"):
        return "arm64-v8a"
    if target.startswith("armv7"):
        return "armeabi-v7a"
    if target.startswith("i686"):
        return "x86"
    if target.startswith("x86_64"):
        return "x86_64"
    return ""

def _swift_android_attr(attrs, key, default):
    value = attrs.get(key)
    if value == None:
        return default
    return value

def _swift_android_reject_unsupported_attrs(attrs, label_id, keys):
    for key in keys:
        value = attrs.get(key)
        if value == None:
            continue
        if type(value) == type("") and value == "":
            continue
        if type(value) == type([]) and len(value) == 0:
            continue
        if type(value) == type({}) and len(value) == 0:
            continue
        if value == False:
            continue
        fail(label_id + ": attribute `" + key + "` is declared for Bazel parity but is not implemented by swift_android_library yet")

def _swift_android_glob_attr(attrs, key):
    return _file_globs(_swift_android_attr(attrs, key, []))

def _swift_android_data_inputs(attrs):
    return _swift_android_glob_attr(attrs, "data")

def _swift_android_compile_inputs(attrs):
    return _swift_android_glob_attr(attrs, "swiftc_inputs")

def _swift_android_swift_flags(attrs):
    flags = []
    flags.extend(_swift_android_attr(attrs, "swift_flags", []))
    flags.extend(_swift_android_attr(attrs, "copts", []))
    for define in _swift_android_attr(attrs, "defines", []):
        flags.extend(["-D", define])
    return flags

def _swift_android_tool(ctx, attrs):
    swiftc = _swift_android_attr(attrs, "swiftc", "") or host_which("swiftc")
    version = host_command([swiftc, "--version"]).strip()
    identity = "once.swift.android.swiftc.v1\x00" + swiftc + "\x00" + version
    sdk = _swift_android_attr(attrs, "sdk", "")
    resource_dir = _swift_android_attr(attrs, "resource_dir", "")
    identity = identity + "\x00sdk\x00" + sdk + "\x00resource_dir\x00" + resource_dir
    return (swiftc, identity)

def _swift_android_host_exe(name):
    if host_os() == "windows":
        return name + ".exe"
    return name

def _swift_android_swift_tool(swiftc):
    bin_dir = _parent_dir(swiftc)
    if bin_dir:
        candidate = bin_dir + "/" + _swift_android_host_exe("swift")
        if host_file_exists(candidate):
            return candidate
    return host_which("swift")

def _swift_android_default_sdk_name(swift):
    for line in host_command([swift, "sdk", "list"]).split("\n"):
        value = line.strip()
        if value and "android" in value:
            return value
    return ""

def _swift_android_config_value(config, key):
    prefix = key + ": "
    for line in config.split("\n"):
        value = line.strip()
        if value.startswith(prefix):
            return value[len(prefix):].strip()
    return ""

def _swift_android_sdk_config(ctx, attrs, swiftc, target):
    sdk = _swift_android_attr(attrs, "sdk", "")
    resource_dir = _swift_android_attr(attrs, "resource_dir", "")
    sdk_name = _swift_android_attr(attrs, "swift_sdk", "")
    if (sdk and resource_dir) or not target:
        return (sdk, resource_dir, sdk_name)
    swift = _swift_android_swift_tool(swiftc)
    if not sdk_name:
        sdk_name = _swift_android_default_sdk_name(swift)
    if not sdk_name:
        return (sdk, resource_dir, sdk_name)
    config = host_command([swift, "sdk", "configure", sdk_name, target, "--show-configuration"])
    if not sdk:
        sdk = _swift_android_config_value(config, "sdkRootPath")
    if not resource_dir:
        resource_dir = _swift_android_config_value(config, "swiftResourcesPath")
    return (sdk, resource_dir, sdk_name)

def _swift_android_ndk_tool_dir(ctx, attrs):
    configured = _swift_android_attr(attrs, "tools_directory", "")
    if configured:
        return configured
    ndk = _swift_android_attr(attrs, "android_ndk", "") or host_env("ANDROID_NDK_HOME")
    if not ndk:
        return ""
    candidates = []
    os = host_os()
    arch = host_arch()
    if os == "macos":
        candidates.extend(["darwin-" + arch, "darwin-arm64", "darwin-x86_64"])
    elif os == "linux":
        candidates.extend(["linux-" + arch, "linux-x86_64", "linux-arm64"])
    elif os == "windows":
        candidates.extend(["windows-" + arch, "windows-x86_64"])
    for tag in _unique(candidates):
        directory = ndk + "/toolchains/llvm/prebuilt/" + tag + "/bin"
        if host_file_exists(directory + "/" + _swift_android_host_exe("clang")):
            return directory
    fail(ctx["label"]["id"] + ": Android NDK under `" + ndk + "` has no usable LLVM prebuilt for this host")

def _swift_android_managed_flags(ctx, attrs, target):
    if "android" not in target:
        return []
    tool_dir = _swift_android_ndk_tool_dir(ctx, attrs)
    if not tool_dir:
        fail(ctx["label"]["id"] + ": set `tools_directory`, `android_ndk`, or ANDROID_NDK_HOME so Swift can link Android outputs with the NDK tools")
    return [
        "-tools-directory", tool_dir,
        "-Xclang-linker", "-fuse-ld=lld",
        "-Xlinker", "-z",
        "-Xlinker", "max-page-size=16384",
    ]

def _swift_android_collect_native_libraries(deps, own):
    out = []
    out.extend(own)
    for dep in deps:
        out.extend(dep.get("transitive_android_native_libraries") or [])
        out.extend(dep.get("android_native_libraries") or [])
    return _unique_native_libraries(out)

def _swift_android_collect_swiftmodule_dirs(deps, own_values):
    out = []
    out.extend(own_values)
    for dep in deps:
        out.extend(dep.get("transitive_swiftmodule_dirs") or [])
    return _unique(out)

def _swift_android_collect_swiftmodule_inputs(deps, own_values):
    out = []
    out.extend(own_values)
    for dep in deps:
        out.extend(dep.get("transitive_swiftmodule_inputs") or [])
        for path in [dep.get("swiftmodule") or "", dep.get("swiftdoc") or "", dep.get("swiftinterface") or ""]:
            if path:
                out.append(path)
    return _unique(out)

def _swift_android_collect_strings(deps, key, own_values):
    out = []
    out.extend(own_values)
    for dep in deps:
        out.extend(dep.get(key) or [])
    return _unique(out)

def _swift_android_collect_linkopts(deps, own_values):
    out = []
    for dep in deps:
        out.extend(dep.get("transitive_linkopts") or [])
    out.extend(own_values)
    return out

def _swift_android_library_impl(ctx):
    attrs = ctx["attr"]
    _swift_android_reject_unsupported_attrs(attrs, ctx["label"]["id"], ["always_include_developer_search_paths", "alwayslink", "generated_header_name", "generates_header", "linkstatic", "plugins", "private_deps"])
    module_name = _swift_android_attr(attrs, "module_name", "") or ctx["label"]["name"]
    package_name = _swift_android_attr(attrs, "package_name", "")
    android_abi = _swift_android_attr(attrs, "android_abi", "")
    android_api = _swift_android_attr(attrs, "android_api", 28)
    target = _swift_android_attr(attrs, "target", "")
    if not target:
        target = _swift_android_target_for_abi(android_abi, android_api)
    else:
        target = _swift_android_target_with_api(target, android_api)
    if not android_abi:
        android_abi = _swift_android_abi_from_target(target)
    if not android_abi:
        fail(ctx["label"]["id"] + ": set `android_abi` or use an Android target triple")

    swift_srcs = _swift_android_sources(ctx)
    if len(swift_srcs) == 0:
        fail("swift_android_library " + ctx["label"]["id"] + " has no Swift sources (.swift)")

    swiftc, swiftc_identity = _swift_android_tool(ctx, attrs)
    sdk, resource_dir, swift_sdk = _swift_android_sdk_config(ctx, attrs, swiftc, target)
    dep_defines = _swift_android_collect_strings(ctx["deps"], "transitive_swift_defines", [])
    swift_flags = []
    for define in dep_defines:
        swift_flags.extend(["-D", define])
    swift_flags.extend(_swift_android_swift_flags(attrs))
    linkopts = _swift_android_collect_linkopts(ctx["deps"], _swift_android_attr(attrs, "linkopts", []))

    library = declare_output("lib" + module_name + ".so")
    swiftmodule = declare_output(module_name + ".swiftmodule")
    swiftdoc = declare_output(module_name + ".swiftdoc")
    swiftinterface = ""
    if _swift_android_attr(attrs, "library_evolution", False):
        swiftinterface = declare_output(module_name + ".swiftinterface")

    dep_swiftmodule_dirs = []
    dep_swiftmodule_inputs = _swift_android_collect_swiftmodule_inputs(ctx["deps"], [])
    dep_native_libraries = []
    for dep in ctx["deps"]:
        for d in dep.get("transitive_swiftmodule_dirs") or []:
            if d and d != ctx["build_dir"] and d not in dep_swiftmodule_dirs:
                dep_swiftmodule_dirs.append(d)
        dep_native_libraries.extend(dep.get("transitive_android_native_libraries") or [])
        dep_native_libraries.extend(dep.get("android_native_libraries") or [])
    dep_native_libraries = _unique_native_libraries(dep_native_libraries)

    argv = [
        swiftc,
        "-emit-library",
        "-emit-module",
        "-module-name",
        module_name,
        "-emit-module-path",
        swiftmodule,
        "-target",
        target,
        "-parse-as-library",
        "-o",
        library,
    ]
    if sdk:
        argv.extend(["-sdk", sdk])
    if resource_dir:
        argv.extend(["-resource-dir", resource_dir])
    if package_name:
        argv.extend(["-package-name", package_name])
    if swiftinterface:
        argv.extend(["-enable-library-evolution", "-emit-module-interface-path", swiftinterface])
    argv.extend(_swift_android_managed_flags(ctx, attrs, target))
    for d in dep_swiftmodule_dirs:
        argv.extend(["-I", d])
    for flag in swift_flags:
        argv.append(flag)
    for src in swift_srcs:
        argv.append(src)
    for native in dep_native_libraries:
        path = native.get("path") or ""
        if path:
            argv.append(path)
    for opt in linkopts:
        argv.append(opt)

    inputs = _unique(swift_srcs + _swift_android_compile_inputs(attrs) + dep_swiftmodule_inputs)
    for native in dep_native_libraries:
        path = native.get("path") or ""
        if path and path not in inputs:
            inputs.append(path)

    run_action(
        argv = argv,
        inputs = inputs,
        outputs = [library, swiftmodule, swiftdoc] + ([swiftinterface] if swiftinterface else []),
        toolchain_identity = swiftc_identity + "\x00target\x00" + target + "\x00abi\x00" + android_abi + "\x00android_api\x00" + str(android_api) + "\x00swift_sdk\x00" + swift_sdk + "\x00sdk\x00" + sdk + "\x00resource_dir\x00" + resource_dir + "\x00package_name\x00" + package_name,
        identifier = "swift_android_compile:" + ctx["label"]["id"],
    )

    own_native = [{"abi": android_abi, "path": library}]
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "swift_android_library",
        "module_name": module_name,
        "target": target,
        "android_abi": android_abi,
        "swiftmodule_dir": ctx["build_dir"],
        "swiftmodule": swiftmodule,
        "swiftdoc": swiftdoc,
        "swiftinterface": swiftinterface,
        "dylib": library,
        "android_native_libraries": own_native,
        "transitive_android_native_libraries": _swift_android_collect_native_libraries(ctx["deps"], own_native),
        "transitive_swiftmodule_dirs": _swift_android_collect_swiftmodule_dirs(ctx["deps"], [ctx["build_dir"]]),
        "transitive_swiftmodule_inputs": _swift_android_collect_swiftmodule_inputs(ctx["deps"], [swiftmodule, swiftdoc] + ([swiftinterface] if swiftinterface else [])),
        "transitive_swift_defines": _swift_android_collect_strings(ctx["deps"], "transitive_swift_defines", _swift_android_attr(attrs, "defines", [])),
        "transitive_linkopts": linkopts,
        "transitive_data": _swift_android_collect_strings(ctx["deps"], "transitive_data", _swift_android_data_inputs(attrs)),
        "affected_inputs": _unique(swift_srcs + _swift_android_compile_inputs(attrs) + _swift_android_data_inputs(attrs)),
    }

swift_android_library = target_kind(
    docs = "Compiles Swift sources into an Android native shared library that Android APK targets package under the requested ABI.",
    attrs = [
        attr("android_abi", "string", docs = "Android ABI directory such as `arm64-v8a`, `armeabi-v7a`, `x86`, or `x86_64`. Inferred from common Android target triples when omitted.", configurable = False),
        attr("android_api", "int", default = "28", docs = "Android API level appended to API-less Android target triples. Defaults to the minimum API level in the bundled Swift Android SDK.", configurable = False),
        attr("target", "string", docs = "Swift target triple. Defaults from `android_abi` and `android_api`; API-less Android triples receive `android_api`.", configurable = False),
        attr("module_name", "string", docs = "Swift module name. Defaults to the target name.", configurable = False),
        attr("package_name", "string", docs = "Swift package name passed through `-package-name` when set.", configurable = False),
        attr("sdk", "string", docs = "Optional sysroot passed to swiftc as `-sdk`, typically from an Android NDK or Swift SDK bundle.", configurable = False),
        attr("resource_dir", "string", docs = "Optional Swift resource directory passed to swiftc as `-resource-dir`.", configurable = False),
        attr("swift_sdk", "string", docs = "Installed Swift SDK identifier used to discover default Android sysroot and Swift resource paths. Defaults to the first installed SDK whose identifier contains `android`.", configurable = False),
        attr("android_ndk", "string", docs = "Android NDK root used to find the LLVM tool directory. Defaults to ANDROID_NDK_HOME.", configurable = False),
        attr("tools_directory", "string", docs = "Directory containing Android clang and linker tools passed as `-tools-directory`. Defaults from android_ndk or ANDROID_NDK_HOME.", configurable = False),
        attr("swiftc", "string", docs = "Override swiftc path.", configurable = False),
        attr("swift_flags", "list<string>", default = "[]", docs = "Additional Swift compiler flags.", configurable = False),
        attr("copts", "list<string>", default = "[]", docs = "Bazel-compatible alias for additional Swift compiler flags.", configurable = False),
        attr("defines", "list<string>", default = "[]", docs = "Bazel-compatible conditional compilation symbols lowered to `-D` flags.", configurable = False),
        attr("linkopts", "list<string>", default = "[]", docs = "Additional linker flags appended after Once-managed flags.", configurable = False),
        attr("data", "list<string>", default = "[]", docs = "Package-relative runtime data file globs propagated to downstream consumers.", configurable = False),
        attr("swiftc_inputs", "list<string>", default = "[]", docs = "Bazel-compatible extra Swift compiler input globs.", configurable = False),
        attr("always_include_developer_search_paths", "bool", default = "false", docs = "Reserved for Bazel-compatible developer search path handling.", configurable = False),
        attr("alwayslink", "bool", default = "false", docs = "Reserved for Bazel-compatible always-link semantics.", configurable = False),
        attr("generated_header_name", "string", docs = "Reserved for generated Objective-C header naming.", configurable = False),
        attr("generates_header", "bool", default = "false", docs = "Reserved for generated Objective-C header emission.", configurable = False),
        attr("library_evolution", "bool", default = "false", docs = "Enable Swift library evolution and emit a textual module interface.", configurable = False),
        attr("linkstatic", "bool", default = "false", docs = "Reserved for Bazel-compatible static link selection.", configurable = False),
        attr("plugins", "list<string>", default = "[]", docs = "Reserved for Swift compiler plugins.", configurable = False),
        attr("private_deps", "list<string>", default = "[]", docs = "Reserved for Bazel-compatible private dependencies.", configurable = False),
    ],
    deps = [
        dep("deps", ["swift_module", "android_native_library"], "Swift modules and Android native libraries linked or packaged with this library."),
    ],
    providers = ["swift_module", "android_native_library", "native_linkable"],
    capabilities = [capability("build", ["default", "native_library", "swiftmodule"])],
    examples = [
        example(
            "swift-android-library-minimal",
            name = "Minimal Swift Android library",
            use_when = "Use this when shared Swift code should be packaged into an Android APK as a native library.",
        ),
        example(
            "native-mobile-shared-code-e2e",
            name = "Shared mobile code app graph",
            use_when = "Use this when an Android app should package Swift and Rust native libraries alongside an Apple app that embeds Kotlin and links Rust.",
        ),
    ],
    impl = _swift_android_library_impl,
)
