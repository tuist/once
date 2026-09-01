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
    info = json_decode(host_command([swift, "package", "dump-package", "--package-path", _swiftpm_absolute_package_path(ctx, attrs.get("package_path") or ".")], env = swiftc["env"]))
    package = {"identity": _basename(package_path) or info.get("name") or ctx["label"]["name"], "path": package_path, "info": info}
    packages = [package] + _swift_package_remote_infos(ctx)
    graph = _xcode_local_swift_package_specs(ctx, packages, attrs.get("platform") or "macos", attrs.get("minimum_os") or "13.0", attrs.get("sdk_variant") or "simulator")
    roots = []
    for product in info.get("products") or []:
        target_ids = graph["products"].get(package["identity"] + "\x1f" + (product.get("name") or ""))
        if target_ids and type(target_ids) != "list":
            target_ids = [target_ids]
        for target_id in target_ids or []:
            if target_id not in roots:
                roots.append(target_id)
    return {"targets": graph["specs"], "roots": roots}

def _swift_package_remote_infos(ctx):
    resolved = ctx["files"].get("Package.resolved")
    if resolved == None:
        return []
    infos = []
    for pin in _swiftpm_resolved_pins(json_decode(resolved)):
        if not _swiftpm_pin_requires_network(pin):
            continue
        if pin.get("kind") != "remoteSourceControl":
            fail("native Swift package integration supports locked source-control dependencies, but `" + pin["identity"] + "` has source kind `" + (pin.get("kind") or "unknown") + "`")
        raw_pin = {
            "identity": pin["identity"],
            "kind": pin.get("kind") or "",
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
        )
        if info:
            infos.append(info)
    return infos

def _swift_package_workspace_impl(ctx):
    return {"label_id": ctx["label"]["id"], "swift_package_workspace": True, "targets": ctx["deps"]}

swift_package_workspace = target_kind(
    docs = "Native Swift Package Manager workspace seed. Its resolver reads Package.swift, materializes locked source-control dependency sources, and lowers every library, executable, macro, binary, and test target into the existing Apple target kinds for direct compilation.",
    attrs = [attr("package_path", "string", default = ".", docs = "Package-relative directory containing Package.swift. Defaults to the native integration package.", configurable = False), attr("resolver_inputs", "list<string>", default = "[]", docs = "Package-relative source globs supplied to native integration resolution. Defaults to srcs when empty.", configurable = False), attr("platform", "string", default = "macos", docs = "Apple platform used when lowering the Swift package targets.", configurable = False), attr("minimum_os", "string", default = "13.0", docs = "Minimum Apple operating system version used when lowering package targets.", configurable = False), attr("sdk_variant", "string", default = "simulator", docs = "Simulator or device software development kit selection. Ignored for macOS.", configurable = False), attr("swift", "string", default = "swift", docs = "Swift Package Manager executable or workspace-relative executable path. The default selects the executable paired with the resolved Swift compiler.", configurable = False), attr("xcode_developer_dir", "string", docs = "Pin a specific Xcode developer directory for Swift and the Apple software development kit.", configurable = False)],
    resolver = _swift_package_workspace_resolver, deps = [dep("deps", ["apple_linkable", "apple_test_bundle", "native_linkable"], "First-party Swift package products emitted by native integration discovery.")], providers = ["swift_package_workspace"], capabilities = [capability("build", [])], tools = [tool("swift", ["swift", "swiftc"])], examples = [example("swift-package-workspace-native-project", name = "Swift Package Manager native integration seed", use_when = "Use this when a Swift Package Manager workspace should derive first-party build and test targets from Package.swift.", platforms = ["macos"])], impl = _swift_package_workspace_impl,
)

swift_package = native_project(target_kind = "swift_package_workspace", docs = "Recognizes a native Swift Package Manager workspace from Package.swift.", markers = ["Package.swift"], target_name = "swift_package", inputs = ["Package.resolved", "Sources/**/*", "Tests/**/*", "Plugins/**/*", "Macros/**/*", "**/Package.swift", "**/Package.resolved"], exclude = _native_project_generated_dirs() + [".build", ".swiftpm", "Pods", "Carthage", "DerivedData", "node_modules"], input_exclude = [".build", ".git"], on_match = "stop", requires_tools = ["swift", "swiftc"])
