def _xcode_swift_package_configuration(configuration):
    name = configuration.lower()
    return "debug" if "debug" in name or "development" in name else "release"

def _apple_build_tool_commands(manifest):
    commands = []
    command = None
    in_commands = False
    for line in manifest.split("\n"):
        if line == "commands:":
            in_commands = True
        elif in_commands and line and not line.startswith(" "):
            break
        elif in_commands and line.startswith("  ") and not line.startswith("   "):
            command = {"name": json_decode(line.strip()[:-1])}
            commands.append(command)
        elif command != None and line.startswith("    ") and not line.startswith("     "):
            key, separator, value = line.strip().partition(":")
            value = value.strip()
            if key in ["inputs", "outputs", "args", "description", "working-directory"]:
                command[key] = json_decode(value)
            elif key == "tool":
                command[key] = value
            elif key == "env":
                command["env"] = {}
        elif command != None and line.startswith("      ") and "env" in command:
            key, separator, value = line.strip().partition(": ")
            command["env"][json_decode(key)] = json_decode(value)
    return commands

def _apple_build_tool_local_path(path):
    if path == workspace_root():
        return "."
    root = workspace_root() + "/"
    if not path.startswith(root) or ".." in path.split("/"):
        fail("Build-tool path must stay inside the workspace: " + path)
    return path[len(root):]

def _apple_build_tool_output_root(commands, requested):
    if requested.endswith("/tools/") and not any([path.startswith(requested) for command in commands for path in (command.get("inputs") or []) + (command.get("outputs") or []) + (command.get("args") or [])]):
        return requested[:-len("tools/")] + "destination/"
    return requested

def apple_build_tool_plan(ctx):
    args = ctx["args"]
    commands = _apple_build_tool_commands(host_file_read(workspace_root() + "/" + args["manifest"]))
    output_root = _apple_build_tool_output_root(commands, workspace_root() + "/" + args["output_root"] + "/")
    pending = {}
    producers = {}
    for command in commands:
        outputs = command.get("outputs") or []
        argv = command.get("args") or []
        signature = (command.get("description") or "") + "-" + content_sha256("|".join(argv))
        if command.get("tool") != "shell" or not outputs:
            continue
        if not any([path.startswith(output_root) or path.startswith("<" + _apple_swift_module_name(args["target_name"]) + "-") for path in outputs]):
            continue
        target_root = workspace_root() + "/" + _parent_dir(args["output_root"]) + "/"
        role_paths = [path for path in argv + (command.get("inputs") or []) if path.startswith(target_root)]
        if role_paths and not any([path.startswith(output_root) for path in role_paths]):
            continue
        if command["name"] != signature:
            fail("Unsupported SwiftPM build-tool command format for " + command["name"] + "; select an Apple Swift 6.3.3 toolchain")
        pending[command["name"]] = command
        for path in outputs:
            producers[path] = command["name"]
    ordered = []
    for _ in range(len(pending)):
        ready = [command for command in pending.values() if not any([producers.get(path) in pending for path in command.get("inputs") or []])]
        if not ready:
            if pending:
                fail("Build-tool commands contain a dependency cycle: " + ", ".join(pending.keys()))
            break
        for command in ready:
            ordered.append(command)
            pending.pop(command["name"])
    for index in range(len(ordered)):
        command = ordered[index]
        outputs = command.get("outputs") or []
        local_outputs = [_apple_build_tool_local_path(path) for path in outputs if not path.startswith("<")]
        inputs = [_apple_build_tool_local_path(path) for path in command.get("inputs") or [] if path.startswith(workspace_root() + "/")]
        run_action(
            argv = command["args"],
            inputs = inputs,
            outputs = local_outputs,
            create_dirs = _unique([_parent_dir(path) for path in local_outputs]),
            cwd = _apple_build_tool_local_path(command.get("working-directory") or args["package_path"]),
            env = command.get("env") or {},
            cacheable = False,
            inherit_parent_env = True,
            sandbox = "off",
            depends_on_prior_actions = False,
            identifier = "build_tool_" + str(index),
            display_name = command.get("description") or "Run package build tool",
        )

def _apple_build_tools_impl(ctx):
    if _apple_execution_toolchain():
        fail(ctx["label"]["id"] + ": SwiftPM build-tool plugins are not yet supported with the Linux execution toolchain; provide generated sources instead")
    attrs = ctx["attr"]
    platform = attrs.get("platform") or "macos"
    sdk_variant = attrs.get("sdk_variant") or "simulator"
    build_role = attrs.get("build_role") or "destination"
    swiftc = _resolve_swiftc(platform, sdk_variant, attrs.get("xcode_developer_dir") or "")
    swift = _parent_dir(swiftc["swiftc_path"]) + "/swift"
    scratch = declare_output("planning")
    roots = declare_output("Packages.xcworkspace/contents.xcworkspacedata")
    package_identity = _basename(attrs["package_path"]).lower()
    if package_identity.endswith(".git"):
        package_identity = package_identity[:-4]
    plugin_outputs = scratch + "/plugins/outputs"
    output_root = plugin_outputs + "/" + package_identity + "/" + attrs["target_name"] + "/" + build_role
    configuration = _xcode_swift_package_configuration(attrs.get("configuration") or "Debug")
    manifest = scratch + "/" + configuration + ".yaml"
    write_path(roots, '<Workspace version="1.0"><FileRef location="absolute:' + _xml_escape(attrs["package_path"]) + '"/></Workspace>')
    pins = []
    mirrors = []
    for identity, encoded in (attrs.get("dependencies") or {}).items():
        dependency = json_decode(encoded)
        pin = dependency["pin"]
        state = {key: value for key, value in pin["state"].items() if value}
        if pin["kind"] == "registry":
            pins.append({"identity": identity, "kind": "registry", "location": "", "state": state})
            scope, separator, name = identity.partition(".")
            source = _apple_build_tool_local_path(dependency["path"])
            destination = scratch + "/registry/downloads/" + scope + "/" + name + "/" + state["version"]
            copy_path(source, destination, kind = "tree", inputs = [source])
        else:
            path = dependency["path"]
            checkout_identity = _basename(path).lower()
            if checkout_identity.endswith(".git"):
                checkout_identity = checkout_identity[:-4]
            if checkout_identity != identity:
                fail("Build-tool dependency checkout must retain its package identity: " + identity + " at " + path)
            pins.append({"identity": identity, "kind": "localSourceControl", "location": path, "state": state})
            mirrors.extend([{"original": location, "mirror": path} for location in dependency["locations"]])
    resolved = declare_output("Packages.xcworkspace/xcshareddata/swiftpm/Package.resolved")
    mirror_config = declare_output("Packages.xcworkspace/xcshareddata/swiftpm/configuration/mirrors.json")
    write_path(resolved, _json_encode({"version": 2, "pins": pins}))
    write_path(mirror_config, _json_encode({"version": 1, "object": mirrors}))
    inputs = [roots, resolved, mirror_config]
    registry_config = attrs["package_path"] + "/.swiftpm/configuration/registries.json"
    if host_file_exists(registry_config):
        output = declare_output("Packages.xcworkspace/xcshareddata/swiftpm/configuration/registries.json")
        write_path(output, host_file_read(registry_config))
        inputs.append(output)
    argv = [
        swift, "build", "--package-path", attrs["package_path"],
        "--scratch-path", workspace_root() + "/" + scratch,
        "--multiroot-data-file", workspace_root() + "/" + _parent_dir(roots),
        "--build-system", "native", "-c", configuration,
        "--triple", _apple_triple(platform, attrs.get("minimum_os") or "13.0", sdk_variant, _apple_default_arch(platform, sdk_variant), False),
        "--sdk", swiftc["sdk_path"],
        "--disable-sandbox", "--disable-build-manifest-caching", "--disable-automatic-resolution",
        "--print-manifest-job-graph", "-j", "1",
        "--target", attrs["target_name"],
    ]
    if attrs.get("declares_traits"):
        argv.append("--disable-default-traits")
        if attrs.get("traits"):
            argv.extend(["--traits", ",".join(attrs["traits"])])
    env = dict(swiftc["env"])
    env["SWIFTPM_MIRROR_CONFIG"] = workspace_root() + "/" + mirror_config
    run_action(
        argv = argv,
        inputs = inputs,
        outputs = [manifest],
        clean_paths = [plugin_outputs],
        create_dirs = [scratch],
        cwd = _apple_build_tool_local_path(attrs["package_path"]),
        env = env,
        cacheable = False,
        inherit_parent_env = True,
        sandbox = "off",
        identifier = "plan_build_tools",
        display_name = "Plan package build tools · " + attrs["target_name"],
    )
    expand_actions(
        implementation = "apple_build_tool_plan",
        inputs = [manifest],
        outputs = [],
        args = {"manifest": manifest, "output_root": output_root, "package_path": attrs["package_path"], "target_name": attrs["target_name"]},
    )
    return {
        "label_id": ctx["label"]["id"],
        "generated_files_root": output_root,
        "generated_target_name": attrs["target_name"],
        "generated_build_role": build_role,
        "generated_clang_flags": attrs.get("per_language_clang_flags") or {},
        "build_tool_manifest": manifest,
    }

def _apple_build_tool_files(ctx):
    files = {"sources": [], "headers": [], "resources": [], "structured_resources": []}
    for dependency in ctx.get("deps") or []:
        root = dependency.get("generated_files_root")
        target = dependency.get("generated_target_name")
        manifest = dependency.get("build_tool_manifest")
        if not root or not target or not manifest:
            continue
        commands = _apple_build_tool_commands(host_file_read(workspace_root() + "/" + manifest))
        absolute_root = _apple_build_tool_output_root(commands, workspace_root() + "/" + root + "/")
        build_dir = "/" + target + ("-tool" if absolute_root.endswith("/tools/") else "") + ".build/"
        for command in commands:
            inputs = [path for path in command.get("inputs") or [] if path.startswith(absolute_root)]
            outputs = command.get("outputs") or []
            if command.get("tool") == "copy-tool":
                for path in inputs:
                    kind = "structured_resources" if path.endswith("/") else "resources"
                    files[kind].append(_apple_build_tool_local_path(path.rstrip("/")))
            elif command.get("tool") == "clang" or (command.get("tool") == "shell" and any([build_dir in path for path in outputs])):
                files["sources"].extend([_apple_build_tool_local_path(path) for path in inputs if _filter_swift_sources([path]) or _filter_objc_sources([path]) or _filter_c_sources([path]) or _filter_cxx_sources([path]) or _filter_assembly_sources([path])])
            elif command.get("tool") == "shell":
                files["headers"].extend([_apple_build_tool_local_path(path) for path in outputs if path.startswith(absolute_root) and _filter_by_extensions([path], [".h", ".hh", ".hpp", ".hxx", ".inc", ".inl", ".ipp", ".tpp", ".def", ".modulemap"])])
    return {kind: _unique(paths) for kind, paths in files.items()}

def _apple_build_tool_attrs(ctx, attrs):
    files = _apple_build_tool_files(ctx)
    attrs = dict(attrs)
    for kind in ["resources", "structured_resources"]:
        attrs[kind] = (attrs.get(kind) or []) + files[kind]
    attrs["resources"] = attrs["resources"] + files["structured_resources"]
    attrs["private_headers"] = (attrs.get("private_headers") or []) + files["headers"]
    attrs["private_header_dirs"] = _unique((attrs.get("private_header_dirs") or []) + [_parent_dir(path) for path in files["headers"]])
    source_flags = {_package_relative(ctx, path): flags for path, flags in (attrs.get("per_source_clang_flags") or {}).items()}
    for dependency in ctx.get("deps") or []:
        root = dependency.get("generated_files_root")
        if not root:
            continue
        for source in files["sources"]:
            if source.startswith(_parent_dir(root) + "/") and (_filter_c_sources([source]) or _filter_cxx_sources([source]) or _filter_objc_sources([source])):
                language = "cxx" if source.endswith(".mm") or _filter_cxx_sources([source]) else "c"
                flags = (dependency.get("generated_clang_flags") or {}).get(language)
                if flags:
                    source_flags[source] = flags
    attrs["per_source_clang_flags"] = source_flags
    if _filter_cxx_sources(files["sources"]) or any([path.endswith(".mm") for path in files["sources"]]):
        attrs["linkopts"] = _apple_unique_linkopts((attrs.get("linkopts") or []) + ["-lc++"])
    return attrs
