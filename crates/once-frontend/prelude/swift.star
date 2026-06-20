def _swift_android_sources(ctx):
    return _filter_by_extensions(glob(ctx["srcs"]), [".swift"])

def _swift_android_target_for_abi(abi):
    if abi == "arm64-v8a":
        return "aarch64-unknown-linux-android"
    if abi == "armeabi-v7a":
        return "armv7-unknown-linux-androideabi"
    if abi == "x86":
        return "i686-unknown-linux-android"
    if abi == "x86_64":
        return "x86_64-unknown-linux-android"
    fail("unsupported Android ABI `" + abi + "` for Swift")

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

def _swift_android_tool(ctx, attrs):
    swiftc = _swift_android_attr(attrs, "swiftc", "") or host_which("swiftc")
    version = host_command([swiftc, "--version"]).strip()
    identity = "once.swift.android.swiftc.v1\x00" + swiftc + "\x00" + version
    sdk = _swift_android_attr(attrs, "sdk", "")
    resource_dir = _swift_android_attr(attrs, "resource_dir", "")
    identity = identity + "\x00sdk\x00" + sdk + "\x00resource_dir\x00" + resource_dir
    return (swiftc, identity)

def _swift_android_native_key(record):
    return (record.get("abi") or "") + "\x00" + (record.get("path") or "")

def _swift_android_unique_native_libraries(libraries):
    seen = {}
    out = []
    for library in libraries:
        key = _swift_android_native_key(library)
        if key not in seen:
            seen[key] = True
            out.append(library)
    return out

def _swift_android_collect_native_libraries(deps, own):
    out = []
    out.extend(own)
    for dep in deps:
        out.extend(dep.get("transitive_android_native_libraries") or [])
        out.extend(dep.get("android_native_libraries") or [])
    return _swift_android_unique_native_libraries(out)

def _swift_android_collect_swiftmodule_dirs(deps, own_values):
    out = []
    out.extend(own_values)
    for dep in deps:
        out.extend(dep.get("transitive_swiftmodule_dirs") or [])
    return _unique(out)

def _swift_android_library_impl(ctx):
    attrs = ctx["attr"]
    module_name = _swift_android_attr(attrs, "module_name", "") or ctx["label"]["name"]
    android_abi = _swift_android_attr(attrs, "android_abi", "")
    target = _swift_android_attr(attrs, "target", "")
    if not target:
        target = _swift_android_target_for_abi(android_abi)
    if not android_abi:
        android_abi = _swift_android_abi_from_target(target)
    if not android_abi:
        fail(ctx["label"]["id"] + ": set `android_abi` or use an Android target triple")

    swift_srcs = _swift_android_sources(ctx)
    if len(swift_srcs) == 0:
        fail("swift_android_library " + ctx["label"]["id"] + " has no Swift sources (.swift)")

    swiftc, swiftc_identity = _swift_android_tool(ctx, attrs)
    sdk = _swift_android_attr(attrs, "sdk", "")
    resource_dir = _swift_android_attr(attrs, "resource_dir", "")
    swift_flags = _swift_android_attr(attrs, "swift_flags", [])
    linkopts = _swift_android_attr(attrs, "linkopts", [])

    library = declare_output("lib" + module_name + ".so")
    swiftmodule = declare_output(module_name + ".swiftmodule")
    swiftdoc = declare_output(module_name + ".swiftdoc")

    dep_swiftmodule_dirs = []
    dep_native_libraries = []
    for dep in ctx["deps"]:
        for d in dep.get("transitive_swiftmodule_dirs") or []:
            if d and d != ctx["build_dir"] and d not in dep_swiftmodule_dirs:
                dep_swiftmodule_dirs.append(d)
        dep_native_libraries.extend(dep.get("transitive_android_native_libraries") or [])
        dep_native_libraries.extend(dep.get("android_native_libraries") or [])
    dep_native_libraries = _swift_android_unique_native_libraries(dep_native_libraries)

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

    inputs = list(swift_srcs)
    for native in dep_native_libraries:
        path = native.get("path") or ""
        if path and path not in inputs:
            inputs.append(path)

    run_action(
        argv = argv,
        inputs = inputs,
        outputs = [library, swiftmodule, swiftdoc],
        toolchain_identity = swiftc_identity + "\x00target\x00" + target + "\x00abi\x00" + android_abi,
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
        "dylib": library,
        "android_native_libraries": own_native,
        "transitive_android_native_libraries": _swift_android_collect_native_libraries(ctx["deps"], own_native),
        "transitive_swiftmodule_dirs": _swift_android_collect_swiftmodule_dirs(ctx["deps"], [ctx["build_dir"]]),
        "affected_inputs": swift_srcs,
    }

swift_android_library = target_kind(
    docs = "Compiles Swift sources into an Android native shared library that Android APK targets package under the requested ABI.",
    attrs = [
        attr("android_abi", "string", docs = "Android ABI directory such as `arm64-v8a`, `armeabi-v7a`, `x86`, or `x86_64`. Inferred from common Android target triples when omitted.", configurable = False),
        attr("target", "string", docs = "Swift target triple. Defaults from `android_abi`; can be set directly when the ABI is inferable.", configurable = False),
        attr("module_name", "string", docs = "Swift module name. Defaults to the target name.", configurable = False),
        attr("sdk", "string", docs = "Optional sysroot passed to swiftc as `-sdk`, typically from an Android NDK or Swift SDK bundle.", configurable = False),
        attr("resource_dir", "string", docs = "Optional Swift resource directory passed to swiftc as `-resource-dir`.", configurable = False),
        attr("swiftc", "string", docs = "Override swiftc path.", configurable = False),
        attr("swift_flags", "list<string>", default = "[]", docs = "Additional Swift compiler flags.", configurable = False),
        attr("linkopts", "list<string>", default = "[]", docs = "Additional linker flags appended after Once-managed flags.", configurable = False),
    ],
    deps = [
        dep("deps", ["swift_module", "android_native_library", "native_linkable"], "Swift modules and Android native libraries linked or packaged with this library."),
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
