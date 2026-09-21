def _swift_package_workspace_resolver(ctx):
    attrs = ctx["attrs"]
    package_path = attrs.get("package_path") or "."
    if package_path.startswith("./"):
        package_path = package_path[2:]
    if package_path == ".":
        package_path = ""
    manifest = _swiftpm_manifest_file(attrs)
    if not ctx["files"].get(manifest):
        fail(ctx["label"]["id"] + ": Swift package manifest `" + manifest + "` is missing; include it in resolver_inputs, or srcs when resolver_inputs is omitted")
    swiftc = _resolve_swiftc(attrs.get("platform") or "macos", attrs.get("sdk_variant") or "simulator", attrs.get("xcode_developer_dir") or "")
    swift = _swiftpm_swift_executable(attrs.get("swift") or "swift", attrs.get("xcode_developer_dir") or "", swiftc["swiftc_path"])
    absolute_package_path = _swiftpm_absolute_package_path(ctx, attrs.get("package_path") or ".")
    # Shared manifest-dedupe map: each `swift package dump-package` invocation
    # bootstraps Swift and can take a couple seconds, and the same package
    # directory is often visited more than once during resolution. Threading
    # this dict through every info call collapses duplicates to one subprocess.
    swift_info_cache = {}
    info = json_decode(host_command([swift, "package", "dump-package", "--package-path", absolute_package_path], env = swiftc["env"]))
    swift_info_cache[absolute_package_path] = {"info": info}
    package = {"identity": _basename(package_path) or info.get("name") or ctx["label"]["name"], "path": package_path, "info": info}
    local_packages = _xcode_expand_swift_package_infos(ctx, [package], cache = swift_info_cache, follow_resolved = False)
    remote_dependencies = any([dependency.get("sourceControl") or dependency.get("registry") for local in local_packages for dependency in local["info"].get("dependencies") or []])
    remote_packages = _swift_package_remote_infos(ctx, info, swift, swiftc["env"], absolute_package_path, package_path, cache = swift_info_cache, remote_dependencies = remote_dependencies)
    packages = _xcode_expand_swift_package_infos(ctx, local_packages + remote_packages, cache = swift_info_cache, follow_resolved = False)
    graph = _xcode_local_swift_package_specs(ctx, packages, attrs.get("platform") or "macos", attrs.get("minimum_os") or "13.0", attrs.get("sdk_variant") or "simulator", root_identities = [package["identity"]])
    for spec in graph["specs"]:
        if spec["kind"] in ["apple_library", "apple_framework", "apple_application", "apple_executable", "apple_test_bundle", "swift_macro"]:
            if attrs.get("explicit_modules"):
                spec["attrs"]["explicit_modules"] = True
            if attrs.get("dependency_check"):
                spec["attrs"]["dependency_check"] = attrs["dependency_check"]
    roots = []
    test_roots = []
    for product in info.get("products") or []:
        target_ids = graph["products"].get(package["identity"] + "\x1f" + (product.get("name") or ""))
        if target_ids and type(target_ids) != "list":
            target_ids = [target_ids]
        for target_id in target_ids or []:
            if target_id not in roots:
                roots.append(target_id)
    for target in info.get("targets") or []:
        if target.get("type") != "test":
            continue
        target_id = graph["modules"].get(target.get("name") or "")
        if target_id:
            test_roots.append(target_id)
    package_name = info.get("name") or ctx["label"]["name"]
    return {"targets": graph["specs"], "roots": roots, "attrs": {"package_name": package_name, "_display_name": package_name, "_default_test_roots": test_roots}}

def _swift_package_remote_infos(ctx, package_info, swift, env, absolute_package_path, package_path, cache = None, remote_dependencies = None):
    resolved = ctx["files"].get("Package.resolved")
    if remote_dependencies == None:
        remote_dependencies = any([dependency.get("sourceControl") or dependency.get("registry") for dependency in package_info.get("dependencies") or []])
    if resolved == None and remote_dependencies:
        host_command([swift, "package", "resolve", "--package-path", absolute_package_path], env = env)
        resolved_path = absolute_package_path + "/Package.resolved"
        if not host_file_exists(resolved_path):
            fail("Swift Package Manager did not produce Package.resolved for package dependencies")
        resolved = host_file_read(resolved_path)
    if resolved == None:
        return []
    infos = []
    registry_resolve_attempted = False
    for pin in _swiftpm_resolved_pins(json_decode(resolved)):
        if not _swiftpm_pin_requires_network(pin):
            continue
        kind = pin.get("kind") or ""
        if kind == "remoteSourceControl":
            raw_pin = {
                "identity": pin["identity"],
                "kind": kind,
                "location": pin.get("location") or "",
                "state": {
                    "revision": pin.get("revision") or "",
                    "version": pin.get("version") or "",
                    "branch": pin.get("branch") or "",
                },
            }
            info = _xcode_remote_swift_package_info(
                ctx,
                pin["identity"],
                raw_pin,
                checkout_root = ".once/swift-package-packages",
                cache = cache,
            )
        elif _swiftpm_pin_is_registry(pin):
            registry_relative = _swiftpm_registry_relative_dir(pin)
            if not registry_relative:
                fail("registry pin `" + (pin.get("raw_identity") or pin["identity"]) + "` is missing an identity or version")
            # Swift Package Manager unpacks registry downloads into the
            # `.build/registry/downloads/...` tree of the package it resolved,
            # not the Once workspace root. For a nested `swift_package_workspace`
            # (package_path = "tools/Tool") that tree lives under
            # `tools/Tool/.build/...`, so the workspace-relative path to the
            # unpacked package concatenates the resolver's own package_path
            # with the registry-relative subpath.
            package_dir = package_path + "/" + registry_relative if package_path else registry_relative
            absolute = _xcode_abs(package_dir)
            if not host_file_exists(absolute + "/Package.swift") and not registry_resolve_attempted:
                # SwiftPM fetches registry archives on `swift package resolve`
                # and unpacks them under `.build/registry/downloads`. If they
                # are not present yet, resolve once and retry every pending
                # entry against the freshly populated cache.
                host_command([swift, "package", "resolve", "--package-path", absolute_package_path], env = env)
                registry_resolve_attempted = True
            if not host_file_exists(absolute + "/Package.swift"):
                fail("registry package `" + (pin.get("raw_identity") or pin["identity"]) + "@" + (pin.get("version") or "") + "` is not present at `" + package_dir + "`; run `swift package resolve` in " + absolute_package_path)
            info = _xcode_swift_package_info(ctx, package_dir, pin.get("raw_identity") or pin["identity"], cache = cache)
        else:
            fail("native Swift package integration supports source-control and registry dependencies, but `" + (pin.get("raw_identity") or pin["identity"]) + "` has source kind `" + (kind or "unknown") + "`")
        if info:
            infos.append(info)
    return infos

def _swift_package_workspace_impl(ctx):
    return {"label_id": ctx["label"]["id"], "swift_package_workspace": True, "targets": ctx["deps"]}

swift_package_workspace = target_kind(
    docs = "Native Swift Package Manager workspace seed. Its resolver reads Package.swift, materializes locked source-control dependency sources, and lowers every library, executable, macro, binary, and test target into the existing Apple target kinds for direct compilation.",
    attrs = [attr("explicit_modules", "bool", default = "false", docs = "Discover and cache module dependencies for package targets.", configurable = False), attr("dependency_check", "string", default = "off", docs = "Use error to reject undeclared imports with explicit modules.", configurable = False, allowed_values = ["off", "error"]), attr("package_path", "string", default = ".", docs = "Package-relative directory containing Package.swift. Defaults to the native integration package.", configurable = False), attr("resolver_inputs", "list<string>", default = "[]", docs = "Package-relative source globs supplied to native integration resolution. Defaults to srcs when empty.", configurable = False), attr("platform", "string", default = "macos", docs = "Apple platform used when lowering the Swift package targets.", configurable = False), attr("minimum_os", "string", default = "13.0", docs = "Minimum Apple operating system version used when lowering package targets.", configurable = False), attr("sdk_variant", "string", default = "simulator", docs = "Simulator or device software development kit selection. Ignored for macOS.", configurable = False), attr("swift", "string", default = "swift", docs = "Swift Package Manager executable or workspace-relative executable path. The default selects the executable paired with the resolved Swift compiler.", configurable = False), attr("xcode_developer_dir", "string", docs = "Pin a specific Xcode developer directory for Swift and the Apple software development kit.", configurable = False), attr("package_name", "string", docs = "Package display name read from Package.swift during resolution. This value is resolver-generated and must not be set in a manifest.", configurable = False), attr("_display_name", "string", docs = "Resolver-owned display name for generic run reporting.", configurable = False), attr("_default_test_roots", "list<string>", default = "[]", docs = "Resolver-owned first-party test target names used by targetless test selection.", configurable = False)],
    resolver = _swift_package_workspace_resolver, deps = [dep("deps", ["apple_application", "apple_executable", "apple_linkable", "apple_test_bundle", "native_linkable"], "First-party Swift package products emitted by native integration discovery, including command-line tool executables lowered as `apple_executable`.")], providers = ["swift_package_workspace"], capabilities = [capability("build", [])], tools = [tool("swift", ["swift", "swiftc"])], examples = [example("swift-package-workspace-native-project", name = "Swift Package Manager native integration seed", use_when = "Use this when a Swift Package Manager workspace should derive first-party build and test targets from Package.swift.", platforms = ["macos"])], impl = _swift_package_workspace_impl,
)

swift_package = native_project(target_kind = "swift_package_workspace", docs = "Recognizes a native Swift Package Manager workspace from Package.swift.", markers = ["Package.swift"], target_name = "swift_package", inputs = ["Package.resolved", "Sources/**/*", "Tests/**/*", "Plugins/**/*", "Macros/**/*", "**/Package.swift", "**/Package.resolved"], exclude = _native_project_generated_dirs() + [".build", ".swiftpm", "Pods", "Carthage", "DerivedData", "node_modules"], input_exclude = [".build", ".git"], on_match = "stop", requires_tools = ["swift", "swiftc"])
