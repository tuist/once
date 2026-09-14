def _apple_module_scan_argv(argv, cache_path):
    result = [argv[0], "-scan-dependencies"]
    skip = False
    remove_values = ["-o", "-emit-module-path", "-emit-module-doc-path", "-emit-objc-header-path", "-output-file-map", "-module-cache-path", "-emit-module-interface-path", "-emit-private-module-interface-path", "-Xlinker"]
    remove_flags = ["-c", "-emit-module", "-emit-library", "-emit-executable", "-emit-objc-header", "-static", "-incremental"]
    for arg in argv[1:]:
        if skip:
            skip = False
        elif arg in remove_values:
            skip = True
        elif arg in remove_flags:
            pass
        elif not arg.startswith("-") and (arg.endswith(".a") or arg.endswith(".o")):
            pass
        else:
            result.append(arg)
    return result + ["-disable-bridging-pch", "-module-cache-path", cache_path]

def _apple_module_host_roots(swiftc):
    if not swiftc.get("sdk_path"):
        return []
    return [
        swiftc["sdk_path"],
        _swift_toolchain_dir(swiftc["swiftc_path"]) + "/usr/lib",
        _parent_dir(_parent_dir(swiftc["sdk_path"])) + "/usr/lib/swift/host",
    ]

def _apple_module_toolchain_identity(swiftc):
    identities = [
        swiftc["identity"],
        host_file_sha256(swiftc["swiftc_path"]),
        host_file_sha256(_parent_dir(swiftc["swiftc_path"]) + "/swift-frontend"),
    ]
    for directory in _apple_module_host_roots(swiftc):
        if host_path_exists(directory):
            identities.append(host_tree_sha256(directory))
    for executable in [
        _parent_dir(swiftc["swiftc_path"]) + "/swift-plugin-server",
        _parent_dir(_parent_dir(swiftc["sdk_path"])) + "/usr/bin/swift-plugin-server",
    ]:
        if host_file_exists(executable):
            identities.append(host_file_sha256(executable))
    return "once.apple.modules.v1\x00" + content_sha256(_json_encode(identities))

def _apple_swift_action(ctx, attrs, swiftc, deps, **action):
    if attrs.get("dependency_check") not in [None, "off", "error"]:
        fail("dependency_check must be off or error")
    if attrs.get("dependency_check") == "error" and not attrs.get("explicit_modules"):
        fail("dependency_check = error requires explicit_modules = true")
    if not attrs.get("explicit_modules"):
        run_action(**action)
        return
    identity = _apple_module_toolchain_identity(swiftc)
    cache_path = ".once/out/modules/" + content_sha256(identity)
    scan = declare_output("ModuleScans/" + action["identifier"] + ".json")
    scan_cache = scan + ".cache"
    scan_argv = _apple_module_scan_argv(action["argv"], scan_cache) + ["-o", scan]
    run_action(
        argv = scan_argv,
        inputs = action["inputs"],
        outputs = [scan],
        clean_paths = [scan_cache],
        create_dirs = [scan_cache],
        env = action.get("env") or swiftc["env"],
        toolchain_identity = identity,
        identifier = "module_scan_" + action["identifier"],
    )
    direct_modules = {}
    declared_deps = attrs.get("_declared_deps")
    declared_ids = {_resolve_dep_ref(ref, ctx["label"]["package"]): True for ref in declared_deps} if declared_deps != None else None
    for dep in deps:
        if declared_ids != None and dep.get("label_id") not in declared_ids:
            continue
        name = dep.get("module_name") or dep.get("framework_module_name")
        if name:
            direct_modules[name] = dep.get("label_id") or name
    own_module = ""
    for index in range(len(action["argv"]) - 1):
        if action["argv"][index] == "-module-name":
            own_module = action["argv"][index + 1]
    expand_actions(
        implementation = "apple_explicit_module_plan",
        inputs = [scan],
        outputs = action["outputs"],
        args = {
            "scan": scan,
            "action": action,
            "compiler": swiftc["swiftc_path"],
            "identity": identity,
            "host_roots": _apple_module_host_roots(swiftc),
            "cache_path": cache_path,
            "scan_cache": scan_cache,
            "direct_modules": direct_modules,
            "module_name": own_module,
            "dependency_check": attrs.get("dependency_check") or "off",
        },
    )

def _apple_scan_key(reference):
    for kind in ["swift", "clang", "swiftPrebuiltExternal", "swiftPlaceholder"]:
        if kind in reference:
            return kind + ":" + reference[kind]
    fail("unsupported compiler module reference: " + _json_encode(reference))

def _apple_scan_records(scan):
    entries = scan.get("modules") or []
    if len(entries) % 2:
        fail("compiler dependency scan contains an incomplete module record")
    records = {}
    for index in range(0, len(entries), 2):
        reference = entries[index]
        key = _apple_scan_key(reference)
        if key in records:
            fail("compiler dependency scan contains duplicate module " + key)
        records[key] = {"reference": reference, "record": entries[index + 1]}
    return records

def _apple_module_input(path):
    if ".." in path.split("/"):
        fail("compiler module path escaped the workspace: " + path)
    root = workspace_root() + "/"
    if path.startswith(root):
        return path[len(root):]
    if path.startswith("/"):
        return ""
    return path

def _apple_module_record_details(entry):
    kind = _apple_scan_key(entry["reference"]).split(":")[0]
    return (entry["record"].get("details") or {}).get(kind) or {}

def _apple_check_module_dependencies(ctx, records, main):
    args = ctx["args"]
    mode = args["dependency_check"]
    if mode not in ["off", "error"]:
        fail("dependency_check must be off or error")
    if mode == "off":
        return
    imported = _apple_module_record_details(main).get("sourceImportedDependencies")
    if imported == None:
        fail("selected compiler does not report source imports for dependency checking")
    for reference in imported:
        key = _apple_scan_key(reference)
        entry = records.get(key)
        if not entry:
            fail("dependency scan did not describe imported module " + key)
        name = key.split(":")[1]
        record = entry["record"]
        details = _apple_module_record_details(entry)
        path = details.get("moduleInterfacePath") or details.get("compiledModulePath") or details.get("moduleMapPath") or record.get("modulePath") or ""
        if _apple_module_input(path) and name not in args["direct_modules"] and name != ctx["args"].get("module_name"):
            return {
                "code": "undeclared_module_dependency",
                "message": ctx["label"]["id"] + " imports undeclared module " + name,
                "target": ctx["label"]["id"],
                "attribute": "deps",
                "repairs": ["Add " + name + " to this target's dependencies in its native manifest or once.toml"],
            }

def _apple_check_module_inputs(ctx, records):
    for entry in records.values():
        record = entry["record"]
        details = _apple_module_record_details(entry)
        paths = list(record.get("sourceFiles") or [])
        paths.extend((details.get("bridgingHeader") or {}).get("sourceFiles") or [])
        for field in ["moduleInterfacePath", "moduleMapPath", "compiledModulePath"]:
            if details.get(field):
                paths.append(details[field])
        for path in paths:
            if ctx["args"].get("scan_cache") and _apple_module_input(path).startswith(ctx["args"]["scan_cache"] + "/"):
                return _apple_scan_artifact_diagnostic(ctx, path)
        if record.get("modulePath"):
            paths.append(record["modulePath"])
        for path in paths:
            if not _apple_module_input(path) and path.startswith("/"):
                if not any([path.startswith(root + "/") or (host_path_exists(path) and host_path_exists(root) and host_path_is_within(path, root)) for root in ctx["args"].get("host_roots") or []]):
                    return {
                        "code": "untracked_host_module_input",
                        "message": "Explicit module input is outside the workspace and identified toolchain: " + path,
                        "target": ctx["label"]["id"],
                        "attribute": "explicit_modules",
                        "repairs": ["Move the dependency into the workspace or disable explicit_modules for this target"],
                    }

def _apple_scan_artifact_diagnostic(ctx, path):
    return {
        "code": "unsupported_module_scan_artifact",
        "message": "Compiler module planning requires an untracked scan-cache artifact: " + path,
        "target": ctx["label"]["id"],
        "attribute": "explicit_modules",
        "repairs": ["Use workspace-owned module interfaces and headers, or disable explicit_modules for this target"],
    }

def apple_explicit_module_plan(ctx):
    args = ctx["args"]
    scan = json_decode(host_file_read(workspace_root() + "/" + args["scan"]))
    records = _apple_scan_records(scan)
    main_key = "swift:" + scan["mainModuleName"]
    main = records.get(main_key)
    if not main:
        fail("compiler dependency scan is missing its main module")
    diagnostic = _apple_check_module_inputs(ctx, records)
    if diagnostic:
        return diagnostic
    diagnostic = _apple_check_module_dependencies(ctx, records, main)
    if diagnostic:
        return diagnostic
    remaining = dict(records)
    remaining.pop(main_key)
    completed = {}
    module_identities = {}
    replacements = {}
    bridging_modules = (_apple_module_record_details(main).get("bridgingHeader") or {}).get("moduleDependencies") or []
    descriptions = []
    module_inputs = []
    for _ in range(len(remaining) + 1):
        ready = []
        for key, entry in remaining.items():
            dependencies = [_apple_scan_key(ref) for ref in entry["record"].get("directDependencies") or []]
            if all([dep in completed for dep in dependencies]):
                ready.append(key)
        if not ready:
            break
        for key in ready:
            entry = remaining.pop(key)
            record = entry["record"]
            details = _apple_module_record_details(entry)
            kind, name = key.split(":")
            path = record.get("modulePath") or details.get("compiledModulePath")
            if not path:
                fail("compiler dependency scan has no module path for " + key)
            inputs = []
            dependency_identities = []
            for reference in record.get("directDependencies") or []:
                inputs.extend(completed[_apple_scan_key(reference)])
                dependency_identities.append(module_identities[_apple_scan_key(reference)])
            dependency_modules = list(inputs)
            source_identities = []
            for source in (record.get("sourceFiles") or []) + ([details["moduleInterfacePath"]] if details.get("moduleInterfacePath") else []):
                local = _apple_module_input(source)
                if local:
                    inputs.append(local)
                    source_identities.append([local, host_file_sha256(workspace_root() + "/" + local)])
            command = details.get("commandLine") or []
            if kind in ["swift", "clang"] and command:
                path = _apple_module_input(path)
                if not path.startswith(args["scan_cache"] + "/"):
                    fail("compiler module output escaped its declared cache directory: " + path)
                normalized = []
                for arg in command:
                    arg = arg.replace(workspace_root() + "/" + args["scan_cache"], args["scan_cache"])
                    for old, new in replacements.items():
                        arg = arg.replace(old, new)
                    arg = arg.replace(path, "{module_output}")
                    if args["scan_cache"] in arg:
                        return _apple_scan_artifact_diagnostic(ctx, arg)
                    normalized.append(arg)
                module_identity = content_sha256(_json_encode([args["identity"], normalized, source_identities, dependency_identities, args["action"].get("env") or {}]))
                module_dir = args["cache_path"] + "/" + module_identity
                output = module_dir + "/" + name + (".pcm" if kind == "clang" else ".swiftmodule")
                replacements[path] = output
                command = [arg.replace("{module_output}", output) for arg in normalized]
                path = output
                run_action(
                    argv = [args["compiler"]] + command,
                    inputs = _unique(inputs),
                    outputs = [path],
                    clean_paths = [module_dir + "/scratch"],
                    create_dirs = [module_dir, module_dir + "/scratch"],
                    env = args["action"].get("env") or {},
                    toolchain_identity = args["identity"],
                    depends_on_prior_actions = False,
                    identifier = "explicit_module_" + _basename(path),
                )
            else:
                local = _apple_module_input(path)
                module_identity = content_sha256(_json_encode([args["identity"], path, host_file_sha256(workspace_root() + "/" + local) if local else ""]))
            module_identities[key] = module_identity
            local = _apple_module_input(path)
            if local:
                inputs.append(local)
                module_inputs.append(local)
            completed[key] = _unique(dependency_modules + ([local] if local else []))
            description = {"moduleName": name, "isFramework": details.get("isFramework") or False}
            if kind == "clang":
                description["clangModulePath"] = path
                description["isBridgingHeaderDependency"] = details.get("isBridgingHeaderDependency") or name in bridging_modules or {"clang": name} in bridging_modules
                modulemap = details.get("moduleMapPath") or ""
                if _apple_module_input(modulemap):
                    description["clangModuleMapPath"] = _apple_module_input(modulemap)
            else:
                description["modulePath"] = path
            descriptions.append(description)
    if remaining:
        fail("compiler dependency scan has cyclic or unresolved modules: " + ", ".join(sorted(remaining.keys())))
    module_map = declare_output("ModuleScans/" + args["action"]["identifier"] + "-modules.json")
    write_path(module_map, _json_encode(descriptions))
    action = dict(args["action"])
    bridging_header = _apple_module_record_details(main).get("bridgingHeader") or {}
    bridge_inputs = [_apple_module_input(path) for path in bridging_header.get("sourceFiles") or []]
    action["argv"] = action["argv"] + [
        "-disable-bridging-pch",
        "-Xfrontend", "-explicit-swift-module-map-file", "-Xfrontend", module_map,
        "-Xfrontend", "-disable-implicit-swift-modules",
        "-Xcc", "-fno-implicit-modules", "-Xcc", "-fno-implicit-module-maps",
    ]
    action["inputs"] = _unique(action["inputs"] + module_inputs + [path for path in bridge_inputs if path] + [module_map])
    action["toolchain_identity"] = args["identity"]
    run_action(**action)
