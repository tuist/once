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

# Rule implementations.
#
# Each impl receives a `ctx` dict built by the Rust analysis pass with:
#   ctx["label"]      -> {"package", "name", "id"}
#   ctx["attr"]       -> typed attribute dict
#   ctx["srcs"]       -> list of workspace-relative source paths (glob-expanded)
#   ctx["deps"]       -> list of provider records returned by analyzed deps
#   ctx["build_dir"]  -> workspace-relative output directory for this target
#
# Globals provided by Rust to impl callbacks:
#   xcrun_swiftc(platform)        -> {"xcrun", "sdk", "identity"}
#   apple_triple(platform, min_os) -> "<arch>-apple-<os><min><suffix>"
#   declare_output(name)           -> workspace-relative output path
#   run_action(argv=..., inputs=..., outputs=..., env={}, toolchain_identity=None, identifier=None)
#
# The impl returns a provider dict. Conventional keys downstream rules read:
#   "swiftmodule_dir" -> directory holding the .swiftmodule (added to -I by consumers)
#   "archive"         -> workspace-relative path to the .a archive

def _apple_library_impl(ctx):
    attrs = ctx["attr"]
    platform = attrs["platform"]
    minimum_os = attrs.get("minimum_os") or "13.0"
    module_name = attrs.get("module_name") or ctx["label"]["name"]
    sdk_frameworks = attrs.get("sdk_frameworks") or []
    weak_sdk_frameworks = attrs.get("weak_sdk_frameworks") or []
    swift_flags = attrs.get("swift_flags") or []
    enable_testing = attrs.get("enable_testing") or False
    library_evolution = attrs.get("library_evolution") or False

    xcrun, sdk, swiftc_identity = xcrun_swiftc(platform)
    triple = apple_triple(platform, minimum_os)

    archive = declare_output(module_name + ".a")
    swiftmodule = declare_output(module_name + ".swiftmodule")
    swiftdoc = declare_output(module_name + ".swiftdoc")

    dep_swiftmodule_dirs = []
    for dep_record in ctx["deps"]:
        dir = dep_record.get("swiftmodule_dir")
        if dir:
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
        "-target",
        triple,
        "-parse-as-library",
    ]
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
    for flag in swift_flags:
        argv.append(flag)
    argv.extend(["-o", archive])
    for src in ctx["srcs"]:
        argv.append(src)

    run_action(
        argv = argv,
        inputs = ctx["srcs"],
        outputs = [archive, swiftmodule, swiftdoc],
        toolchain_identity = swiftc_identity,
        identifier = "swift_compile_" + module_name,
    )

    return {
        "swiftmodule_dir": ctx["build_dir"],
        "archive": archive,
    }

APPLE_RULES = [
    rule(
        kind = "apple_library",
        docs = "Compiles Swift, Objective-C, C, and C++ sources into a linkable Apple module.",
        impl = _apple_library_impl,
        attrs = [
            attr("platform", "string", required = True, docs = "Apple platform such as ios, macos, tvos, watchos, or visionos"),
            attr("minimum_os", "string", docs = "Minimum supported OS version"),
            attr("module_name", "string", docs = "Compiled module name. Defaults to the target name", configurable = False),
            attr("headers", "list<string>", default = "[]", docs = "Public or private C-family headers compiled with this target"),
            attr("exported_headers", "list<string>", default = "[]", docs = "Headers made available to dependent targets"),
            attr("sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked by name, such as UIKit or Foundation"),
            attr("weak_sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked weakly"),
            attr("sdk_dylibs", "list<string>", default = "[]", docs = "Apple SDK dynamic libraries linked by name"),
            attr("linkopts", "list<string>", default = "[]", docs = "Extra linker flags"),
            attr("swift_flags", "list<string>", default = "[]", docs = "Extra Swift compiler flags"),
            attr("clang_flags", "list<string>", default = "[]", docs = "Extra Clang compiler flags"),
            attr("enable_testing", "bool", default = "false", docs = "Compile Swift with testability enabled for dependent tests"),
            attr("library_evolution", "bool", default = "false", docs = "Emit stable Swift module interfaces for binary compatibility"),
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
