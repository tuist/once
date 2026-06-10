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

def _apple_sdk_name(platform):
    if platform == "ios":
        return "iphonesimulator"
    if platform == "macos" or platform == "macosx":
        return "macosx"
    if platform == "tvos":
        return "appletvsimulator"
    if platform == "watchos":
        return "watchsimulator"
    if platform == "visionos" or platform == "xros":
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

def _apple_triple_suffix(platform):
    if platform == "macos" or platform == "macosx":
        return ""
    return "-simulator"

def _apple_triple(platform, minimum_os):
    arch = host_arch()
    triple_os = _apple_triple_os(platform)
    suffix = _apple_triple_suffix(platform)
    return arch + "-apple-" + triple_os + minimum_os + suffix

def _xcrun_swiftc(platform):
    xcrun = host_which("xcrun")
    sdk = _apple_sdk_name(platform)
    swiftc_path = host_command([xcrun, "--sdk", sdk, "--find", "swiftc"]).strip()
    version = host_command([xcrun, "--sdk", sdk, "swiftc", "--version"]).strip()
    identity = "once.apple.swiftc.v1\x00" + swiftc_path + "\x00" + version
    return (xcrun, sdk, identity)

def _ends_with(value, suffix):
    if len(value) < len(suffix):
        return False
    return value[len(value) - len(suffix):] == suffix

def _filter_swift_sources(paths):
    out = []
    for path in paths:
        if _ends_with(path, ".swift"):
            out.append(path)
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
    module_name = attrs.get("module_name") or ctx["label"]["name"]
    sdk_frameworks = attrs.get("sdk_frameworks") or []
    weak_sdk_frameworks = attrs.get("weak_sdk_frameworks") or []
    sdk_dylibs = attrs.get("sdk_dylibs") or []
    linkopts = attrs.get("linkopts") or []
    swift_flags = attrs.get("swift_flags") or []
    defines = attrs.get("defines") or []
    enable_testing = attrs.get("enable_testing") or False
    library_evolution = attrs.get("library_evolution") or False
    emit_dsym = attrs.get("emit_dsym") or False

    swift_srcs = _filter_swift_sources(glob(ctx["srcs"]))
    if len(swift_srcs) == 0:
        fail("apple_library " + ctx["label"]["id"] + " has no Swift sources; add `.swift` files matching `srcs`")

    xcrun, sdk, swiftc_identity = _xcrun_swiftc(platform)
    # target_sdk_version drives the triple's deployment-target component
    # so manifests can pin a build-time SDK that differs from the
    # runtime minimum (Buck2 splits these explicitly; we follow suit).
    triple = _apple_triple(platform, target_sdk_version)

    archive = declare_output(module_name + ".a")
    swiftmodule = declare_output(module_name + ".swiftmodule")
    swiftdoc = declare_output(module_name + ".swiftdoc")
    objc_header = declare_output(module_name + "-Swift.h")

    # Walk dep providers once to compose the transitive views every
    # downstream rule (the parent library, a future linker, a future
    # bundler) needs in a single place.
    deps = ctx["deps"]
    transitive_swiftmodule_dirs = _collect_transitive(deps, "transitive_swiftmodule_dirs", [ctx["build_dir"]])
    transitive_archives = _collect_transitive(deps, "transitive_archives", [archive])
    transitive_sdk_frameworks = _collect_transitive(deps, "transitive_sdk_frameworks", sdk_frameworks)
    transitive_weak_sdk_frameworks = _collect_transitive(deps, "transitive_weak_sdk_frameworks", weak_sdk_frameworks)
    transitive_sdk_dylibs = _collect_transitive(deps, "transitive_sdk_dylibs", sdk_dylibs)
    transitive_linkopts = _collect_transitive(deps, "transitive_linkopts", linkopts)
    transitive_defines = _collect_transitive(deps, "transitive_defines", defines)
    # Each dep already exposes its own swiftmodule_dir through its
    # transitive list; pulling it via the transitive accumulator avoids
    # duplicating it as a single-value field.
    dep_swiftmodule_dirs = []
    for dep in deps:
        for dir in dep.get("transitive_swiftmodule_dirs") or []:
            if dir != ctx["build_dir"] and dir not in dep_swiftmodule_dirs:
                dep_swiftmodule_dirs.append(dir)

    argv = [
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
        objc_header,
        "-target",
        triple,
        "-parse-as-library",
    ]
    if emit_dsym:
        # `-g` emits DWARF into the object stream; downstream a
        # dedicated dsymutil step would extract the .dSYM bundle. The
        # `-debug-info-format=dwarf` flag is the default for static
        # archives so we don't repeat it.
        argv.append("-g")
    if enable_testing:
        argv.append("-enable-testing")
    if library_evolution:
        argv.append("-enable-library-evolution")
    for framework in sdk_frameworks:
        argv.extend(["-framework", framework])
    for framework in weak_sdk_frameworks:
        argv.extend(["-weak_framework", framework])
    for dep_dir in dep_swiftmodule_dirs:
        argv.extend(["-I", dep_dir])
    for define in defines:
        # Swift wants -D<NAME>=<VALUE> wrapped in -Xfrontend? Actually
        # `swiftc -D` accepts a single identifier; values get filtered
        # to Clang via -Xcc. We pass identifiers through directly.
        argv.append("-D")
        argv.append(define)
    for flag in swift_flags:
        argv.append(flag)
    argv.extend(["-o", archive])
    for src in swift_srcs:
        argv.append(src)

    run_action(
        argv = argv,
        inputs = swift_srcs,
        outputs = [archive, swiftmodule, swiftdoc, objc_header],
        toolchain_identity = swiftc_identity,
        identifier = "swift_compile_" + module_name,
    )

    return {
        # Direct outputs the parent library reads.
        "swiftmodule_dir": ctx["build_dir"],
        "archive": archive,
        "objc_header": objc_header,
        # SwiftInfo / CcInfo-style transitive views downstream rules
        # (linker, bundler, test) walk to compose their own commands.
        "transitive_swiftmodule_dirs": transitive_swiftmodule_dirs,
        "transitive_archives": transitive_archives,
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
