def attr(name, ty, required = False, default = None, docs = "", configurable = True):
    return {
        "name": name,
        "ty": ty,
        "required": required,
        "default": default,
        "docs": docs,
        "configurable": configurable,
    }

def dep(name, expected_providers, docs = ""):
    return {
        "name": name,
        "expected_providers": expected_providers,
        "docs": docs,
    }

def capability(name, output_groups, requires_outputs = []):
    return {
        "name": name,
        "output_groups": output_groups,
        "requires_outputs": requires_outputs,
    }

def rule(kind, docs, attrs = [], deps = [], providers = [], capabilities = [], examples = [], impl = None):
    return {
        "kind": kind,
        "docs": docs,
        "attrs": attrs,
        "deps": deps,
        "providers": providers,
        "capabilities": capabilities,
        "examples": examples,
        "impl": impl,
    }

# Generic host primitives provided by Rust:
#   host_arch()                -> "arm64" | "x86_64" | ...
#   host_os()                  -> "macos" | "linux" | ...
#   host_which(name)           -> absolute path to a binary on PATH, or fails
#   host_command(argv)         -> stdout string; fails on non-zero exit
#   glob(patterns)             -> sorted, deduplicated workspace-relative file paths
#                                 matching the patterns under the active package
#   declare_output(name)       -> workspace-relative output path under the active build_dir
#   run_action(argv=..., inputs=..., outputs=..., env={}, toolchain_identity=None, identifier=None)
#
# Each impl receives a `ctx` dict built by the Rust analysis pass with:
#   ctx["label"]      -> {"package", "name", "id"}
#   ctx["attr"]       -> typed attribute dict
#   ctx["srcs"]       -> raw glob patterns declared on the target (impl calls glob() to expand)
#   ctx["deps"]       -> list of provider records returned by analyzed deps
#   ctx["build_dir"]  -> workspace-relative output directory for this target
#
# The impl returns a provider dict. Conventional keys downstream rules read:
#   "swiftmodule_dir" -> directory holding the .swiftmodule (added to -I by consumers)
#   "archive"         -> workspace-relative path to the .a archive

# Apple-specific helpers implemented in starlark on top of the generic
# primitives. Everything platform-specific (SDK names, triple format,
# xcrun resolution, file-extension filtering) lives here, not in Rust.

def _apple_sdk_name(platform, sdk_variant):
    # macOS doesn't ship a simulator SDK; the variant is ignored.
    if platform == "macos" or platform == "macosx":
        return "macosx"
    # For every other platform pick the device SDK or the simulator
    # SDK based on `sdk_variant`. Defaulting to simulator preserves
    # the previous behavior for manifests that don't set it.
    if platform == "ios":
        if sdk_variant == "device":
            return "iphoneos"
        return "iphonesimulator"
    if platform == "tvos":
        if sdk_variant == "device":
            return "appletvos"
        return "appletvsimulator"
    if platform == "watchos":
        if sdk_variant == "device":
            return "watchos"
        return "watchsimulator"
    if platform == "visionos" or platform == "xros":
        if sdk_variant == "device":
            return "xros"
        return "xrsimulator"
    fail("unsupported apple platform `" + platform + "`")

def _apple_triple_os(platform):
    if platform == "macos" or platform == "macosx":
        return "macosx"
    if platform == "ios":
        return "ios"
    if platform == "tvos":
        return "tvos"
    if platform == "watchos":
        return "watchos"
    if platform == "visionos" or platform == "xros":
        return "xros"
    return platform

def _apple_triple_suffix(platform, sdk_variant):
    # macOS has no simulator. Device variants on other platforms render
    # an empty suffix; simulators keep the `-simulator` tag swiftc
    # expects.
    if platform == "macos" or platform == "macosx":
        return ""
    if sdk_variant == "device":
        return ""
    return "-simulator"

def _apple_triple(platform, minimum_os, sdk_variant):
    arch = host_arch()
    triple_os = _apple_triple_os(platform)
    suffix = _apple_triple_suffix(platform, sdk_variant)
    return arch + "-apple-" + triple_os + minimum_os + suffix

def _xcrun_env(xcode_developer_dir):
    env = {}
    if xcode_developer_dir:
        env["DEVELOPER_DIR"] = xcode_developer_dir
    return env

def _xcrun_swiftc(platform, sdk_variant, xcode_developer_dir):
    xcrun = host_which("xcrun")
    sdk = _apple_sdk_name(platform, sdk_variant)
    env = _xcrun_env(xcode_developer_dir)
    swiftc_path = host_command([xcrun, "--sdk", sdk, "--find", "swiftc"], env = env).strip()
    version = host_command([xcrun, "--sdk", sdk, "swiftc", "--version"], env = env).strip()
    # Identity also folds in the developer dir override so different
    # Xcode installations partition the action cache cleanly.
    identity = "once.apple.swiftc.v1\x00" + swiftc_path + "\x00" + version + "\x00" + (xcode_developer_dir or "")
    return (xcrun, sdk, identity, env)

def _ends_with(value, suffix):
    if len(value) < len(suffix):
        return False
    return value[len(value) - len(suffix):] == suffix

def _filter_by_extensions(paths, extensions):
    out = []
    for path in paths:
        for ext in extensions:
            if _ends_with(path, ext):
                out.append(path)
                break
    return out

def _filter_swift_sources(paths):
    return _filter_by_extensions(paths, [".swift"])

def _filter_objc_sources(paths):
    return _filter_by_extensions(paths, [".m", ".mm"])

def _filter_c_sources(paths):
    return _filter_by_extensions(paths, [".c"])

def _filter_cxx_sources(paths):
    return _filter_by_extensions(paths, [".cc", ".cpp", ".cxx"])

def _xcrun_clang(platform, sdk_variant, xcode_developer_dir):
    xcrun = host_which("xcrun")
    sdk = _apple_sdk_name(platform, sdk_variant)
    env = _xcrun_env(xcode_developer_dir)
    clang_path = host_command([xcrun, "--sdk", sdk, "--find", "clang"], env = env).strip()
    sdk_path = host_command([xcrun, "--sdk", sdk, "--show-sdk-path"], env = env).strip()
    version = host_command([xcrun, "--sdk", sdk, "clang", "--version"], env = env).strip()
    identity = "once.apple.clang.v1\x00" + clang_path + "\x00" + version + "\x00" + (xcode_developer_dir or "")
    return (xcrun, sdk, sdk_path, identity, env)

def _xcrun_libtool(platform, sdk_variant, xcode_developer_dir):
    xcrun = host_which("xcrun")
    sdk = _apple_sdk_name(platform, sdk_variant)
    env = _xcrun_env(xcode_developer_dir)
    libtool_path = host_command([xcrun, "--sdk", sdk, "--find", "libtool"], env = env).strip()
    identity = "once.apple.libtool.v1\x00" + libtool_path + "\x00" + (xcode_developer_dir or "")
    return (xcrun, sdk, identity, env)

def _package_relative(ctx, path):
    if not path:
        return path
    # Absolute / already-workspace-rooted paths (rare in once.toml)
    # are returned untouched.
    if path.startswith("/") or path.startswith("."):
        return path
    package = ctx["label"]["package"]
    if package:
        return package + "/" + path
    return path

def _parent_dir(path):
    idx = -1
    for i in range(len(path)):
        if path[i] == "/":
            idx = i
    if idx < 0:
        return ""
    return path[:idx]

def _unique_dirs(paths):
    seen = {}
    out = []
    for path in paths:
        directory = _parent_dir(path)
        if directory and directory not in seen:
            seen[directory] = True
            out.append(directory)
    return out

# Accumulate a transitive list of strings from a field that every dep
# provider exposes. Preserves order while removing duplicates: the
# first occurrence wins. Mirrors the rules_swift / Buck2 convention of
# propagating SwiftInfo / CcInfo fields up the graph.
def _collect_transitive(deps, key, own_values):
    seen = {}
    out = []
    for value in own_values:
        if value not in seen:
            seen[value] = True
            out.append(value)
    for dep in deps:
        for value in dep.get(key) or []:
            if value not in seen:
                seen[value] = True
                out.append(value)
    return out

def _apple_library_impl(ctx):
    attrs = ctx["attr"]
    platform = attrs["platform"]
    minimum_os = attrs.get("minimum_os") or "13.0"
    target_sdk_version = attrs.get("target_sdk_version") or minimum_os
    sdk_variant = attrs.get("sdk_variant") or "simulator"
    xcode_developer_dir = attrs.get("xcode_developer_dir") or ""
    module_name = attrs.get("module_name") or ctx["label"]["name"]
    sdk_frameworks = attrs.get("sdk_frameworks") or []
    weak_sdk_frameworks = attrs.get("weak_sdk_frameworks") or []
    sdk_dylibs = attrs.get("sdk_dylibs") or []
    linkopts = attrs.get("linkopts") or []
    swift_flags = attrs.get("swift_flags") or []
    clang_flags = attrs.get("clang_flags") or []
    defines = attrs.get("defines") or []
    enable_testing = attrs.get("enable_testing") or False
    library_evolution = attrs.get("library_evolution") or False
    emit_dsym = attrs.get("emit_dsym") or False
    alwayslink = attrs.get("alwayslink") or False
    exported_deps = attrs.get("exported_deps") or []
    bridging_header = attrs.get("bridging_header") or ""
    exported_headers = attrs.get("exported_headers") or []
    enable_modules = attrs.get("enable_modules") or False

    all_srcs = glob(ctx["srcs"])
    swift_srcs = _filter_swift_sources(all_srcs)
    objc_srcs = _filter_objc_sources(all_srcs)
    c_srcs = _filter_c_sources(all_srcs)
    cxx_srcs = _filter_cxx_sources(all_srcs)
    if len(swift_srcs) == 0 and len(objc_srcs) == 0 and len(c_srcs) == 0 and len(cxx_srcs) == 0:
        fail("apple_library " + ctx["label"]["id"] + " has no compilable sources (.swift/.m/.mm/.c/.cc)")

    xcrun, sdk, swiftc_identity, xcrun_env = _xcrun_swiftc(platform, sdk_variant, xcode_developer_dir)
    triple = _apple_triple(platform, target_sdk_version, sdk_variant)
    archive = declare_output(module_name + ".a")

    deps = ctx["deps"]
    # Split deps into compile-visible (exported) and link-only.
    exported_dep_indices = []
    for index, dep in enumerate(deps):
        dep_label = dep.get("label_id")
        if dep_label and dep_label in exported_deps:
            exported_dep_indices.append(index)

    compile_swiftmodule_dirs = []
    for dep in deps:
        for dir in dep.get("transitive_swiftmodule_dirs") or []:
            if dir != ctx["build_dir"] and dir not in compile_swiftmodule_dirs:
                compile_swiftmodule_dirs.append(dir)

    # Compile-visible header dirs: direct deps' exported headers'
    # parent directories. Used as `-I` flags for both clang and
    # swiftc's `-Xcc -I` (so Swift can see ObjC types via the
    # bridging header or import path).
    compile_header_dirs = []
    for dep in deps:
        for h in dep.get("transitive_exported_header_dirs") or []:
            if h not in compile_header_dirs:
                compile_header_dirs.append(h)
    dep_modulemaps = []
    for dep in deps:
        for m in dep.get("transitive_modulemaps") or []:
            if m not in dep_modulemaps:
                dep_modulemaps.append(m)

    # Own exported headers as workspace-relative paths, plus the
    # dirs we expose to consumers.
    own_exported_headers = [_package_relative(ctx, h) for h in exported_headers]
    own_exported_header_dirs = _unique_dirs(own_exported_headers)

    # Modulemap generation: if the target exports headers AND opts into
    # clang modules, write a modulemap so consumers can `import` the
    # module without listing each header on the command line. This is
    # the minimum Buck2 / rules_apple do; framework modules and umbrella
    # headers can layer on later.
    modulemap_path = ""
    if enable_modules and len(own_exported_headers) > 0:
        modulemap_path = declare_output("module.modulemap")
        modulemap_lines = ["module " + module_name + " {"]
        for header in own_exported_headers:
            # Header paths in modulemaps are relative to the modulemap's
            # location. We write the modulemap into `build_dir/` and
            # reference each header by its workspace-relative path
            # prefixed with the relative escape back to workspace root.
            depth = len(ctx["build_dir"].split("/"))
            relative = ("../" * depth) + header
            modulemap_lines.append("    header \"" + relative + "\"")
        modulemap_lines.append("    export *")
        modulemap_lines.append("}")
        modulemap_lines.append("")
        write_file(modulemap_path, "\n".join(modulemap_lines))

    exported_deps_records = [deps[i] for i in exported_dep_indices]
    transitive_swiftmodule_dirs = _collect_transitive(
        exported_deps_records,
        "transitive_swiftmodule_dirs",
        [ctx["build_dir"]],
    )
    transitive_exported_header_dirs = _collect_transitive(
        exported_deps_records,
        "transitive_exported_header_dirs",
        own_exported_header_dirs,
    )
    transitive_exported_headers = _collect_transitive(
        exported_deps_records,
        "transitive_exported_headers",
        own_exported_headers,
    )
    transitive_modulemaps = _collect_transitive(
        exported_deps_records,
        "transitive_modulemaps",
        [modulemap_path] if modulemap_path else [],
    )
    transitive_archives = _collect_transitive(deps, "transitive_archives", [archive])
    transitive_sdk_frameworks = _collect_transitive(deps, "transitive_sdk_frameworks", sdk_frameworks)
    transitive_weak_sdk_frameworks = _collect_transitive(deps, "transitive_weak_sdk_frameworks", weak_sdk_frameworks)
    transitive_sdk_dylibs = _collect_transitive(deps, "transitive_sdk_dylibs", sdk_dylibs)
    transitive_linkopts = _collect_transitive(deps, "transitive_linkopts", linkopts)
    transitive_defines = _collect_transitive(deps, "transitive_defines", defines)
    transitive_alwayslink_archives = _collect_transitive(deps, "transitive_alwayslink_archives", [archive] if alwayslink else [])

    # --- Swift compile -----------------------------------------------
    # When there are no Swift sources, skip the swiftc action entirely
    # and emit the archive from the clang side via libtool.
    swift_objc_header = declare_output(module_name + "-Swift.h") if len(swift_srcs) > 0 else ""
    swiftmodule = declare_output(module_name + ".swiftmodule") if len(swift_srcs) > 0 else ""
    swiftdoc = declare_output(module_name + ".swiftdoc") if len(swift_srcs) > 0 else ""

    # Swift output: either the final archive (Swift-only) or an
    # intermediate that libtool will merge with the clang objects.
    swift_only = len(objc_srcs) == 0 and len(c_srcs) == 0 and len(cxx_srcs) == 0
    swift_archive = archive if swift_only else (declare_output(module_name + "-swift.a") if len(swift_srcs) > 0 else "")

    if len(swift_srcs) > 0:
        swift_argv = [
            xcrun,
            "--sdk",
            sdk,
            "swiftc",
            "-emit-library",
            "-static",
            "-emit-module",
            "-module-name",
            module_name,
            "-emit-module-path",
            swiftmodule,
            "-emit-objc-header",
            "-emit-objc-header-path",
            swift_objc_header,
            "-target",
            triple,
            "-parse-as-library",
        ]
        if emit_dsym:
            swift_argv.append("-g")
        if enable_testing:
            swift_argv.append("-enable-testing")
        if library_evolution:
            swift_argv.append("-enable-library-evolution")
        if bridging_header:
            swift_argv.extend(["-import-objc-header", _package_relative(ctx, bridging_header)])
        for framework in sdk_frameworks:
            swift_argv.extend(["-framework", framework])
        for framework in weak_sdk_frameworks:
            swift_argv.extend(["-weak_framework", framework])
        for dep_dir in compile_swiftmodule_dirs:
            swift_argv.extend(["-I", dep_dir])
        # Header search paths flow through `-Xcc -I` so swiftc's
        # underlying Clang invocation (for bridging headers + ObjC
        # interop) can locate dep headers.
        for hdir in compile_header_dirs:
            swift_argv.extend(["-Xcc", "-I", "-Xcc", hdir])
        # Feed each dep's modulemap to swiftc's underlying Clang so
        # `import` of a clang-module dep resolves without manual
        # `-fmodule-map-file` from the user.
        for mmap in dep_modulemaps:
            swift_argv.extend(["-Xcc", "-fmodule-map-file=" + mmap])
        if enable_modules:
            swift_argv.extend(["-Xcc", "-fmodules"])
        for define in defines:
            swift_argv.extend(["-D", define])
        for flag in swift_flags:
            swift_argv.append(flag)
        swift_argv.extend(["-o", swift_archive])
        for src in swift_srcs:
            swift_argv.append(src)

        swift_inputs = list(swift_srcs)
        if bridging_header:
            swift_inputs.append(_package_relative(ctx, bridging_header))
        # The bridging header may #include other headers, so feed
        # each exported header through as an action input too.
        for h in own_exported_headers:
            if h not in swift_inputs:
                swift_inputs.append(h)

        run_action(
            argv = swift_argv,
            inputs = swift_inputs,
            outputs = [swift_archive, swiftmodule, swiftdoc, swift_objc_header],
            env = xcrun_env,
            toolchain_identity = swiftc_identity,
            identifier = "swift_compile_" + module_name,
        )

    # --- C / ObjC / C++ compile --------------------------------------
    clang_objects = []
    if len(objc_srcs) > 0 or len(c_srcs) > 0 or len(cxx_srcs) > 0:
        clang_xcrun, clang_sdk, sdk_path, clang_identity, clang_env = _xcrun_clang(platform, sdk_variant, xcode_developer_dir)

        def clang_object_name(src):
            # Sanitise the source path into a stable .o filename under
            # the build dir: `apps/ios/AppCore/Sources/A.m` →
            # `apps_ios_AppCore_Sources_A.m.o`. The full path keeps
            # collisions impossible without forcing a tree mkdir.
            sanitised = src.replace("/", "_")
            return declare_output(sanitised + ".o")

        def compile_with_clang(src, language):
            obj = clang_object_name(src)
            argv = [
                clang_xcrun,
                "--sdk",
                clang_sdk,
                "clang" if language != "c++" else "clang++",
                "-c",
                "-x",
                language,
                "-arch",
                host_arch(),
                "-isysroot",
                sdk_path,
                "-target",
                triple,
                "-o",
                obj,
            ]
            if language == "objective-c" or language == "objective-c++":
                argv.append("-fobjc-arc")
            if emit_dsym:
                argv.append("-g")
            if enable_modules:
                argv.append("-fmodules")
            for framework in sdk_frameworks:
                argv.extend(["-framework", framework])
            for hdir in own_exported_header_dirs:
                argv.extend(["-I", hdir])
            for hdir in compile_header_dirs:
                argv.extend(["-I", hdir])
            for mmap in dep_modulemaps:
                argv.append("-fmodule-map-file=" + mmap)
            for define in defines:
                argv.append("-D" + define)
            for flag in clang_flags:
                argv.append(flag)
            argv.append(src)
            inputs = [src]
            for h in own_exported_headers:
                if h not in inputs:
                    inputs.append(h)
            run_action(
                argv = argv,
                inputs = inputs,
                outputs = [obj],
                env = clang_env,
                toolchain_identity = clang_identity,
                identifier = "clang_compile_" + module_name + "_" + src.replace("/", "_"),
            )
            clang_objects.append(obj)

        for src in objc_srcs:
            compile_with_clang(src, "objective-c")
        for src in c_srcs:
            compile_with_clang(src, "c")
        for src in cxx_srcs:
            compile_with_clang(src, "c++")

    # --- libtool merge ----------------------------------------------
    # Combine the swift-only archive plus clang objects into the final
    # archive. Only needed when there is at least one non-Swift input
    # alongside Swift; Swift-only and C-only libraries produce the
    # final archive directly.
    if not swift_only and len(swift_srcs) > 0:
        libtool_xcrun, libtool_sdk, libtool_identity, libtool_env = _xcrun_libtool(platform, sdk_variant, xcode_developer_dir)
        libtool_argv = [
            libtool_xcrun,
            "--sdk",
            libtool_sdk,
            "libtool",
            "-static",
            "-o",
            archive,
            swift_archive,
        ]
        libtool_argv.extend(clang_objects)
        libtool_inputs = [swift_archive]
        libtool_inputs.extend(clang_objects)
        run_action(
            argv = libtool_argv,
            inputs = libtool_inputs,
            outputs = [archive],
            env = libtool_env,
            toolchain_identity = libtool_identity,
            identifier = "libtool_merge_" + module_name,
        )
    elif len(swift_srcs) == 0 and len(clang_objects) > 0:
        # No Swift at all: ar the clang objects directly into the
        # final archive.
        libtool_xcrun, libtool_sdk, libtool_identity, libtool_env = _xcrun_libtool(platform, sdk_variant, xcode_developer_dir)
        libtool_argv = [libtool_xcrun, "--sdk", libtool_sdk, "libtool", "-static", "-o", archive]
        libtool_argv.extend(clang_objects)
        run_action(
            argv = libtool_argv,
            inputs = list(clang_objects),
            outputs = [archive],
            env = libtool_env,
            toolchain_identity = libtool_identity,
            identifier = "libtool_archive_" + module_name,
        )

    return {
        "label_id": ctx["label"]["id"],
        "swiftmodule_dir": ctx["build_dir"] if len(swift_srcs) > 0 else "",
        "archive": archive,
        "objc_header": swift_objc_header,
        "alwayslink": alwayslink,
        "exported_headers": own_exported_headers,
        "exported_header_dirs": own_exported_header_dirs,
        "modulemap": modulemap_path,
        "transitive_swiftmodule_dirs": transitive_swiftmodule_dirs,
        "transitive_exported_headers": transitive_exported_headers,
        "transitive_exported_header_dirs": transitive_exported_header_dirs,
        "transitive_modulemaps": transitive_modulemaps,
        "transitive_archives": transitive_archives,
        "transitive_alwayslink_archives": transitive_alwayslink_archives,
        "transitive_sdk_frameworks": transitive_sdk_frameworks,
        "transitive_weak_sdk_frameworks": transitive_weak_sdk_frameworks,
        "transitive_sdk_dylibs": transitive_sdk_dylibs,
        "transitive_linkopts": transitive_linkopts,
        "transitive_defines": transitive_defines,
    }

APPLE_RULES = [
    rule(
        kind = "apple_library",
        docs = "Compiles Swift, Objective-C, C, and C++ sources into a linkable Apple module.",
        impl = _apple_library_impl,
        attrs = [
            attr("platform", "string", required = True, docs = "Apple platform such as ios, macos, tvos, watchos, or visionos"),
            attr("minimum_os", "string", docs = "Minimum supported OS version (deployment target)"),
            attr("target_sdk_version", "string", docs = "Build-time SDK version baked into the triple. Defaults to `minimum_os`"),
            attr("module_name", "string", docs = "Compiled module name. Defaults to the target name", configurable = False),
            attr("headers", "list<string>", default = "[]", docs = "Public or private C-family headers compiled with this target"),
            attr("exported_headers", "list<string>", default = "[]", docs = "Headers made available to dependent targets"),
            attr("sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked by name, such as UIKit or Foundation"),
            attr("weak_sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked weakly"),
            attr("sdk_dylibs", "list<string>", default = "[]", docs = "Apple SDK dynamic libraries linked by name"),
            attr("linkopts", "list<string>", default = "[]", docs = "Extra linker flags, propagated transitively to consumers"),
            attr("swift_flags", "list<string>", default = "[]", docs = "Extra Swift compiler flags"),
            attr("clang_flags", "list<string>", default = "[]", docs = "Extra Clang compiler flags"),
            attr("defines", "list<string>", default = "[]", docs = "`-D` preprocessor / Swift conditional compilation flags, propagated transitively"),
            attr("enable_testing", "bool", default = "false", docs = "Compile Swift with testability enabled for dependent tests"),
            attr("library_evolution", "bool", default = "false", docs = "Emit stable Swift module interfaces for binary compatibility"),
            attr("emit_dsym", "bool", default = "false", docs = "Emit DWARF debug info so downstream rules can extract a `.dSYM` bundle"),
            attr("sdk_variant", "string", default = "\"simulator\"", docs = "`simulator` or `device` SDK selection. Ignored on macOS (always uses macosx)"),
            attr("xcode_developer_dir", "string", docs = "Pin a specific Xcode by overriding `DEVELOPER_DIR`. Folded into the action cache key"),
            attr("alwayslink", "bool", default = "false", docs = "Hint to downstream linker rules to force-load this archive (`-Wl,-force_load`)"),
            attr("exported_deps", "list<string>", default = "[]", docs = "Target IDs from `deps` whose module interface flows through to consumers' compile path"),
            attr("bridging_header", "string", docs = "ObjC bridging header that lets Swift sources see ObjC symbols (`-import-objc-header`)"),
            attr("enable_modules", "bool", default = "false", docs = "Emit a `module.modulemap` for `exported_headers` and pass `-fmodules` to Clang so consumers can `import` the module instead of #importing each header"),
        ],
        deps = [
            dep("deps", ["apple_linkable", "apple_resource"], "Libraries, frameworks, or resources consumed by this library"),
        ],
        providers = ["apple_linkable", "apple_module"],
        capabilities = [
            capability("build", ["default", "binary", "swiftmodule", "generated_sources"]),
        ],
        examples = [
            "[[target]]\nname = \"AppKit\"\nkind = \"apple_library\"\nsrcs = [\"Sources/**/*.swift\"]\n\n[target.attrs]\nplatform = \"ios\"\nminimum_os = \"17.0\"",
        ],
    ),
    rule(
        kind = "apple_framework",
        docs = "Builds an Apple framework product with module metadata and optional resources.",
        attrs = [
            attr("platform", "string", required = True, docs = "Apple platform for the framework"),
            attr("minimum_os", "string", docs = "Minimum supported OS version"),
            attr("bundle_id", "string", docs = "Framework bundle identifier"),
            attr("product_name", "string", docs = "Framework product name. Defaults to the target name", configurable = False),
            attr("headers", "list<string>", default = "[]", docs = "Headers packaged with the framework"),
            attr("exported_headers", "list<string>", default = "[]", docs = "Headers exported to downstream consumers"),
            attr("resources", "list<string>", default = "[]", docs = "Resource glob patterns bundled into the framework"),
            attr("asset_catalogs", "list<string>", default = "[]", docs = "Asset catalog paths compiled into the framework bundle"),
            attr("privacy_manifest", "string", docs = "Privacy manifest placed in the framework bundle"),
            attr("sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked by name"),
            attr("weak_sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked weakly"),
        ],
        deps = [
            dep("deps", ["apple_linkable", "apple_resource"], "Libraries and resources linked or embedded by the framework"),
        ],
        providers = ["apple_linkable", "apple_framework", "apple_bundle"],
        capabilities = [
            capability("build", ["default", "framework", "dsyms", "swiftmodule"]),
        ],
        examples = [
            "[[target]]\nname = \"AuthFramework\"\nkind = \"apple_framework\"\ndeps = [\"./AuthCore\"]\n\n[target.attrs]\nplatform = \"ios\"\nbundle_id = \"dev.once.Auth\"",
        ],
    ),
    rule(
        kind = "apple_application",
        docs = "Builds an Apple application bundle with resources, Info.plist metadata, and signing inputs.",
        attrs = [
            attr("platform", "string", required = True, docs = "Apple platform for the application"),
            attr("bundle_id", "string", required = True, docs = "Application bundle identifier"),
            attr("minimum_os", "string", docs = "Minimum supported OS version"),
            attr("families", "list<string>", default = "[]", docs = "Supported device families, such as iphone or ipad"),
            attr("product_name", "string", docs = "Application product name. Defaults to the target name", configurable = False),
            attr("resources", "list<string>", default = "[]", docs = "Resource and asset catalog glob patterns"),
            attr("asset_catalogs", "list<string>", default = "[]", docs = "Asset catalog paths compiled into the application bundle"),
            attr("info_plist", "string", docs = "Info.plist template path"),
            attr("info_plist_substitutions", "map<string,string>", default = "{}", docs = "Values substituted into the generated Info.plist"),
            attr("entitlements", "string", docs = "Entitlements plist path"),
            attr("provisioning_profile", "string", docs = "Provisioning profile label or path used for signing"),
            attr("signing", "string", default = "ad_hoc", docs = "Signing mode or policy name"),
            attr("sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked by name"),
            attr("weak_sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked weakly"),
            attr("sdk_dylibs", "list<string>", default = "[]", docs = "Apple SDK dynamic libraries linked by name"),
        ],
        deps = [
            dep("deps", ["apple_linkable", "apple_framework", "apple_resource"], "Libraries, frameworks, and resources embedded in the app"),
        ],
        providers = ["apple_application", "apple_bundle"],
        capabilities = [
            capability("build", ["default", "bundle", "dsyms"]),
            capability("run", ["default"], requires_outputs = ["bundle"]),
        ],
        examples = [
            "[[target]]\nname = \"App\"\nkind = \"apple_application\"\ndeps = [\"./AppKit\"]\n\n[target.attrs]\nplatform = \"ios\"\nbundle_id = \"dev.once.App\"\nminimum_os = \"17.0\"",
        ],
    ),
    rule(
        kind = "apple_test_bundle",
        docs = "Builds and runs XCTest-style test bundles, including app-hosted tests when declared.",
        attrs = [
            attr("platform", "string", required = True, docs = "Apple platform for the tests"),
            attr("minimum_os", "string", docs = "Minimum supported OS version"),
            attr("test_host", "target", docs = "Application target hosting the test bundle"),
            attr("resources", "list<string>", default = "[]", docs = "Resource glob patterns bundled into the test bundle"),
            attr("asset_catalogs", "list<string>", default = "[]", docs = "Asset catalog paths compiled into the test bundle"),
            attr("info_plist", "string", docs = "Info.plist template path"),
            attr("entitlements", "string", docs = "Entitlements plist path"),
            attr("destination", "string", docs = "Simulator, device, or local destination selector"),
            attr("test_plan", "string", docs = "XCTest plan path"),
            attr("test_env", "map<string,string>", default = "{}", docs = "Environment variables passed to the test runner"),
        ],
        deps = [
            dep("deps", ["apple_linkable", "apple_application"], "Code under test and optional host application"),
        ],
        providers = ["apple_test_bundle", "apple_bundle"],
        capabilities = [
            capability("build", ["default", "bundle", "dsyms"]),
            capability("test", ["default", "test_results", "coverage"], requires_outputs = ["bundle"]),
        ],
        examples = [
            "[[target]]\nname = \"AppTests\"\nkind = \"apple_test_bundle\"\ndeps = [\"./AppKit\"]\n\n[target.attrs]\nplatform = \"ios\"\ntest_host = \"./App\"",
        ],
    ),
]
