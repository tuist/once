_REACT_NATIVE_NODE_TOOL = tool("node", executables = ["node"])
_REACT_NATIVE_NPM_TOOL = tool("npm", executables = ["npm"])
_REACT_NATIVE_PNPM_TOOL = tool("pnpm", executables = ["pnpm"])
_REACT_NATIVE_YARN_TOOL = tool("yarn", executables = ["yarn"])
_REACT_NATIVE_BUN_TOOL = tool("bun", executables = ["bun"])
_REACT_NATIVE_XCODE_TOOL = tool("xcodebuild", executables = ["xcodebuild"])
_REACT_NATIVE_BUNDLE_TOOL = tool("bundle", executables = ["bundle"])
_REACT_NATIVE_JAVA_TOOL = tool("java", executables = ["java"])
_REACT_NATIVE_ADB_TOOL = tool("adb", executables = ["adb"])

_REACT_NATIVE_LOCKFILES = {
    "npm": "package-lock.json",
    "pnpm": "pnpm-lock.yaml",
    "yarn": "yarn.lock",
    "bun": "bun.lock",
}

def _react_native_attr(ctx, name, default):
    return _configured_attr(ctx, name, default)

def _react_native_resolver_file(ctx, path, description):
    key = path[2:] if path.startswith("./") else path
    value = (ctx.get("files") or {}).get(key)
    if value == None:
        fail(ctx["label"]["id"] + ": " + description + " `" + path + "` must be included in resolver_inputs")
    return value

def _react_native_safe_name(value):
    out = []
    for ch in value.elems():
        code = ord(ch)
        alpha = (code >= ord("a") and code <= ord("z")) or (code >= ord("A") and code <= ord("Z"))
        digit = code >= ord("0") and code <= ord("9")
        if alpha or digit or ch == "-" or ch == ".":
            out.append(ch)
        elif ch == "_":
            out.append("_u")
        else:
            out.append("_x" + str(code) + "_")
    return "".join(out)

def _react_native_locked_package(lock, name):
    packages = lock.get("packages") or {}
    direct = packages.get("node_modules/" + name) or {}
    if direct:
        return direct
    suffix = "/node_modules/" + name
    for path, package in packages.items():
        if path.endswith(suffix):
            return package
    return {}

def _react_native_package_manager_from_lockfile(path):
    name = _basename(path)
    for manager, lockfile in _REACT_NATIVE_LOCKFILES.items():
        if name == lockfile:
            return manager
    return ""

def _react_native_package_manager(ctx, attrs, package_json):
    requested = attrs.get("package_manager") or "auto"
    lockfile = attrs.get("lockfile") or ""
    if requested != "auto" and requested not in _REACT_NATIVE_LOCKFILES:
        fail(ctx["label"]["id"] + ": package_manager must be `auto`, `npm`, `pnpm`, `yarn`, or `bun`")
    manager = requested if requested != "auto" else ""
    if not manager and lockfile:
        manager = _react_native_package_manager_from_lockfile(lockfile)
    package_manager_field = package_json.get("packageManager") or ""
    if not manager and package_manager_field:
        manager = package_manager_field.split("@")[0]
        if manager not in _REACT_NATIVE_LOCKFILES:
            fail(ctx["label"]["id"] + ": package.json packageManager must select npm, pnpm, Yarn, or Bun")
    if not manager:
        candidates = []
        files = ctx.get("files") or {}
        for candidate_manager, candidate_lockfile in _REACT_NATIVE_LOCKFILES.items():
            if files.get(candidate_lockfile) != None:
                candidates.append(candidate_manager)
        if len(candidates) != 1:
            fail(ctx["label"]["id"] + ": declare one supported lockfile or set package_manager and lockfile explicitly")
        manager = candidates[0]
    if not lockfile:
        lockfile = _REACT_NATIVE_LOCKFILES[manager]
    inferred = _react_native_package_manager_from_lockfile(lockfile)
    if inferred and inferred != manager:
        fail(ctx["label"]["id"] + ": lockfile `" + lockfile + "` does not match package manager `" + manager + "`")
    return {"manager": manager, "lockfile": lockfile}

def _react_native_unquote_lock_value(value):
    value = value.strip()
    if value.endswith(":"):
        value = value[:-1]
    if len(value) >= 2 and ((value.startswith("\"") and value.endswith("\"")) or (value.startswith("'") and value.endswith("'"))):
        return value[1:-1]
    return value

def _react_native_text_lock_version(lockfile, manager, name):
    lines = lockfile.replace("\r", "").split("\n")
    if manager == "pnpm":
        prefix = "  " + name + "@"
        for raw in lines:
            if raw.startswith(prefix) and raw.endswith(":"):
                version = _react_native_unquote_lock_value(raw[len(prefix):])
                return version.split("(")[0]
    elif manager == "yarn":
        in_package = False
        for raw in lines:
            line = raw.strip()
            if raw and not raw.startswith(" ") and line.endswith(":"):
                header = _react_native_unquote_lock_value(line)
                in_package = header.startswith(name + "@")
            elif in_package and line.startswith("version:"):
                return _react_native_unquote_lock_value(line[len("version:"):])
            elif in_package and line.startswith("version "):
                return _react_native_unquote_lock_value(line[len("version "):])
    elif manager == "bun":
        prefix = "\"" + name + "\": [\"" + name + "@"
        for raw in lines:
            line = raw.strip()
            if line.startswith(prefix):
                return line[len(prefix):].split("\"")[0]
    return ""

def _react_native_locked_version(lock, lockfile, manager, name):
    if manager == "npm":
        return (_react_native_locked_package(lock, name).get("version") or "")
    return _react_native_text_lock_version(lockfile, manager, name)

def _react_native_ascii_digits(value):
    if not value:
        return False
    for ch in value.elems():
        if ch < "0" or ch > "9":
            return False
    return True

def _react_native_exact_version(value):
    core = value.split("+")[0].split("-")[0]
    parts = core.split(".")
    return len(parts) == 3 and all([_react_native_ascii_digits(part) for part in parts])

def _react_native_configuration_contains_credentials(value):
    lowered = value.lower()
    markers = [
        "_authtoken",
        "_password",
        "_auth=",
        "npmauthtoken",
        "npmauthident",
        "token =",
        "token=",
        "password =",
        "password=",
    ]
    return any([marker in lowered for marker in markers])

def _react_native_module_target_name(name, version):
    return "react-native-module-" + _react_native_safe_name(name + "@" + (version or "local"))

def _react_native_module_spec(module):
    name = module.get("name") or ""
    version = module.get("version") or ""
    platforms = module.get("platforms") or {}
    ios = platforms.get("ios") or {}
    android = platforms.get("android") or {}
    target_name = _react_native_module_target_name(name, version)
    return {
        "name": target_name,
        "kind": "react_native_module",
        "deps": [],
        "srcs": [],
        "attrs": {
            "module_name": name,
            "version": version,
            "source_root": module.get("root") or ("node_modules/" + name),
            "ios_podspec": ios.get("podspec") or "",
            "android_source_dir": android.get("source_dir") or "",
            "android_library_name": android.get("library_name") or "",
            "android_cmake_lists": android.get("cmake_lists") or "",
            "android_component_descriptors": android.get("component_descriptors") or [],
            "pure_cxx": android.get("pure_cxx") or False,
            "_react_native_locked": True,
        },
    }

def _react_native_dependencies_resolver(ctx):
    attrs = ctx.get("attrs") or {}
    package_json_path = attrs.get("package_json") or "package.json"
    npmrc_path = attrs.get("npmrc") or ""
    package_manager_files = attrs.get("package_manager_files") or []
    snapshot_path = attrs.get("modules_snapshot") or ""
    package_json = json_decode(_react_native_resolver_file(ctx, package_json_path, "JavaScript package manifest"))
    package_manager = _react_native_package_manager(ctx, attrs, package_json)
    manager = package_manager["manager"]
    lockfile_path = package_manager["lockfile"]
    lockfile = _react_native_resolver_file(ctx, lockfile_path, manager + " lockfile")
    lock = json_decode(lockfile) if manager == "npm" else {}
    react_native_version = _react_native_locked_version(lock, lockfile, manager, "react-native")
    if not react_native_version:
        fail(ctx["label"]["id"] + ": " + lockfile_path + " has no locked `react-native` package")
    hermes_version = _react_native_locked_version(lock, lockfile, manager, "hermes-compiler")
    if not hermes_version:
        fail(ctx["label"]["id"] + ": " + lockfile_path + " has no Hermes compiler paired with the locked React Native package")
    requested = (package_json.get("dependencies") or {}).get("react-native") or ""
    if requested and _react_native_exact_version(requested) and requested != react_native_version:
        fail(ctx["label"]["id"] + ": package.json requests React Native `" + requested + "`, but " + lockfile_path + " selects `" + react_native_version + "`")
    configuration_files = package_manager_files + ([npmrc_path] if npmrc_path else [])
    for path in configuration_files:
        configuration = _react_native_resolver_file(ctx, path, "package manager configuration")
        if _react_native_configuration_contains_credentials(configuration):
            fail(ctx["label"]["id"] + ": package manager configuration must not contain authentication tokens or passwords because dependency inputs may be stored in a shared cache")

    modules = []
    if snapshot_path:
        snapshot = json_decode(_react_native_resolver_file(ctx, snapshot_path, "React Native module snapshot"))
        if snapshot.get("schema") != "once.react_native.modules.v1":
            fail(ctx["label"]["id"] + ": React Native module snapshot must use schema `once.react_native.modules.v1`")
        snapshot_version = snapshot.get("react_native_version") or ""
        if snapshot_version != react_native_version:
            fail(ctx["label"]["id"] + ": React Native module snapshot selects `" + snapshot_version + "`, but " + lockfile_path + " selects `" + react_native_version + "`")
        modules = snapshot.get("modules") or []

    targets = []
    roots = []
    for module in modules:
        module_name = module.get("name") or ""
        if not module_name:
            fail(ctx["label"]["id"] + ": React Native module snapshot contains a module without a name")
        version = _react_native_locked_version(lock, lockfile, manager, module_name)
        if not version:
            fail(ctx["label"]["id"] + ": React Native module `" + module_name + "` is absent from " + lockfile_path)
        snapshot_module = dict(module)
        snapshot_module["version"] = version
        spec = _react_native_module_spec(snapshot_module)
        targets.append(spec)
        roots.append(spec["name"])

    return {
        "targets": targets,
        "roots": roots,
        "attrs": {
            "_react_native_resolved": True,
            "_package_manager": manager,
            "_lockfile": lockfile_path,
            "_react_native_version": react_native_version,
            "_hermes_version": hermes_version,
            "_react_native_module_targets": roots,
        },
    }

def _react_native_tools(ctx, include_package_manager = False):
    node_name = _react_native_attr(ctx, "node", "node")
    node = _resolve_host_executable(node_name)
    if not node:
        fail(ctx["label"]["id"] + ": Node.js executable `" + node_name + "` was not found")
    node_version = host_command([node, "--version"]).strip()
    tools = {
        "node": node,
        "node_version": node_version,
        "identity": "once.react-native.node.v1\x00" + node + "\x00" + node_version,
    }
    if include_package_manager:
        manager = _react_native_attr(ctx, "_package_manager", "npm")
        executable_name = _react_native_attr(ctx, "package_manager_executable", "") or manager
        executable = _resolve_host_executable(executable_name)
        if not executable:
            fail(ctx["label"]["id"] + ": " + manager + " executable `" + executable_name + "` was not found")
        version = host_command([executable, "--version"]).strip()
        tools["package_manager"] = executable
        tools["package_manager_name"] = manager
        tools["package_manager_version"] = version
        tools["identity"] = tools["identity"] + "\x00" + manager + "\x00" + executable + "\x00" + version
    return tools

def _react_native_path_env(tools, extra = []):
    dirs = []
    for path in [tools.get("node") or "", tools.get("package_manager") or ""] + extra:
        parent = _parent_dir(path)
        if parent and parent not in dirs:
            dirs.append(parent)
    dirs.extend(["/usr/bin", "/bin"])
    return ":".join(dirs)

def _react_native_install_plan(tools, allow_network, action_cache):
    manager = tools["package_manager_name"]
    executable = tools["package_manager"]
    version = tools["package_manager_version"]
    env = {}
    create_dirs = [action_cache]
    if manager == "npm":
        argv = [executable, "ci", "--no-audit", "--no-fund"]
        env["npm_config_cache"] = execution_path(action_cache)
        if not allow_network:
            argv.append("--offline")
    elif manager == "pnpm":
        argv = [
            executable,
            "install",
            "--frozen-lockfile",
            "--public-hoist-pattern",
            "@react-native/*",
            "--store-dir",
            execution_path(action_cache),
        ]
        if not allow_network:
            argv.append("--offline")
    elif manager == "yarn":
        major = version.split(".")[0]
        if major == "1":
            argv = [executable, "install", "--frozen-lockfile", "--cache-folder", execution_path(action_cache)]
            if not allow_network:
                argv.append("--offline")
        else:
            argv = [executable, "install", "--immutable"]
            env["YARN_CACHE_FOLDER"] = execution_path(action_cache)
            env["YARN_ENABLE_GLOBAL_CACHE"] = "0"
            env["YARN_NODE_LINKER"] = "node-modules"
            if not allow_network:
                env["YARN_ENABLE_NETWORK"] = "0"
                env["YARN_ENABLE_OFFLINE_MODE"] = "1"
    elif manager == "bun":
        argv = [executable, "install", "--frozen-lockfile", "--linker", "hoisted", "--cache-dir", execution_path(action_cache)]
        if not allow_network:
            argv.append("--prefer-offline")
    else:
        fail("unsupported React Native package manager `" + manager + "`")
    return {"argv": argv, "env": env, "create_dirs": create_dirs}

def _react_native_dependencies_impl(ctx):
    if not _react_native_attr(ctx, "_react_native_resolved", False):
        fail(ctx["label"]["id"] + ": react_native_dependencies must be expanded through its locked resolver")
    tools = _react_native_tools(ctx, True)
    package_json = _package_relative(ctx, _react_native_attr(ctx, "package_json", "package.json"))
    lockfile = _package_relative(ctx, _react_native_attr(ctx, "_lockfile", "package-lock.json"))
    npmrc_attr = _react_native_attr(ctx, "npmrc", "")
    snapshot_attr = _react_native_attr(ctx, "modules_snapshot", "")
    snapshot = _package_relative(ctx, snapshot_attr) if snapshot_attr else ""
    install_root = ctx["build_dir"] + "/javascript"
    node_modules = install_root + "/node_modules"
    installed_package_json = install_root + "/package.json"
    installed_lockfile = install_root + "/" + _basename(lockfile)
    prepare_path(install_root, kind = "remove", identifier = ctx["label"]["id"] + ":javascript-clean")
    prepare_path(install_root, kind = "directory", identifier = ctx["label"]["id"] + ":javascript-prepare")
    copy_path(package_json, installed_package_json, inputs = [package_json], identifier = ctx["label"]["id"] + ":package-json")
    copy_path(lockfile, installed_lockfile, inputs = [lockfile], identifier = ctx["label"]["id"] + ":package-lock")
    install_inputs = [installed_package_json, installed_lockfile]
    configuration_sources = []
    for configured in _react_native_attr(ctx, "package_manager_files", []) + ([npmrc_attr] if npmrc_attr else []):
        source = _package_relative(ctx, configured)
        if source in configuration_sources:
            continue
        configuration_sources.append(source)
        relative = _react_native_relative_path(ctx, source)
        installed_configuration = install_root + "/" + relative
        copy_path(
            source,
            installed_configuration,
            inputs = [source],
            identifier = ctx["label"]["id"] + ":package-manager-config:" + relative,
        )
        install_inputs.append(installed_configuration)
    for source in glob(ctx["srcs"]):
        if source in [package_json, lockfile, snapshot] + configuration_sources:
            continue
        relative = _react_native_relative_path(ctx, source)
        installed_source = install_root + "/" + relative
        copy_path(
            source,
            installed_source,
            inputs = [source],
            identifier = ctx["label"]["id"] + ":local-package:" + relative,
        )
        install_inputs.append(installed_source)
    action_home = ctx["scratch_dir"] + "/home"
    action_cache = ctx["scratch_dir"] + "/" + tools["package_manager_name"] + "-cache"
    install = _react_native_install_plan(tools, _react_native_attr(ctx, "allow_network", False), action_cache)
    argv = install["argv"]
    argv.extend(_react_native_attr(ctx, "install_args", []))
    env = {
        "CI": "1",
        "HOME": execution_path(action_home),
        "PATH": _react_native_path_env(tools),
    }
    env.update(install["env"])
    run_action(
        argv = argv,
        inputs = install_inputs,
        outputs = [node_modules],
        clean_paths = [node_modules],
        create_dirs = [action_home] + install["create_dirs"],
        cwd = install_root,
        env = env,
        toolchain_identity = tools["identity"] + "\x00react-native\x00" + _react_native_attr(ctx, "_react_native_version", "") + "\x00hermes\x00" + _react_native_attr(ctx, "_hermes_version", ""),
        identifier = ctx["label"]["id"] + ":package-install",
    )
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "react_native_dependencies",
        "javascript_root": install_root,
        "node_modules": node_modules,
        "package_json": package_json,
        "lockfile": lockfile,
        "package_manager": _react_native_attr(ctx, "_package_manager", "npm"),
        "react_native_version": _react_native_attr(ctx, "_react_native_version", ""),
        "hermes_version": _react_native_attr(ctx, "_hermes_version", ""),
        "module_targets": _react_native_attr(ctx, "_react_native_module_targets", []),
        "react_native_dependency_set": True,
        "javascript_dependency_set": True,
    }

def _react_native_module_impl(ctx):
    if not _react_native_attr(ctx, "_react_native_locked", False):
        fail(ctx["label"]["id"] + ": react_native_module targets are generated by react_native_dependencies")
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "react_native_module",
        "module_name": _react_native_attr(ctx, "module_name", ""),
        "version": _react_native_attr(ctx, "version", ""),
        "source_root": _react_native_attr(ctx, "source_root", ""),
        "ios_podspec": _react_native_attr(ctx, "ios_podspec", ""),
        "android_source_dir": _react_native_attr(ctx, "android_source_dir", ""),
        "android_library_name": _react_native_attr(ctx, "android_library_name", ""),
        "android_cmake_lists": _react_native_attr(ctx, "android_cmake_lists", ""),
        "android_component_descriptors": _react_native_attr(ctx, "android_component_descriptors", []),
        "pure_cxx": _react_native_attr(ctx, "pure_cxx", False),
    }

def _react_native_dependency(ctx):
    found = []
    for dependency in ctx["deps"]:
        if dependency.get("react_native_dependency_set"):
            found.append(dependency)
    if len(found) != 1:
        fail(ctx["label"]["id"] + ": expected exactly one react_native_dependencies dependency")
    return found[0]

def _react_native_package_root(ctx):
    package = ctx["label"]["package"]
    return package if package else "."

def _react_native_relative_path(ctx, path):
    package = ctx["label"]["package"]
    prefix = package + "/" if package else ""
    if prefix and path.startswith(prefix):
        return path[len(prefix):]
    return path

def _react_native_stage_files(ctx, stage, files):
    for source in files:
        relative = _react_native_relative_path(ctx, source)
        copy_path(
            source,
            stage + "/" + relative,
            inputs = [source],
            identifier = ctx["label"]["id"] + ":stage:" + relative,
        )

def _react_native_source_files(ctx, default_excludes = []):
    excluded = {}
    for source in glob(default_excludes + _react_native_attr(ctx, "exclude_srcs", [])):
        excluded[source] = True
    return [source for source in glob(ctx["srcs"]) if source not in excluded]

def _react_native_stage_project(ctx, dependency, files):
    stage = ctx["build_dir"] + "/project"
    prepare_path(stage, kind = "remove", identifier = ctx["label"]["id"] + ":project-clean")
    prepare_path(stage, kind = "directory", identifier = ctx["label"]["id"] + ":project-prepare")
    _react_native_stage_files(ctx, stage, files)
    link_path(
        dependency["node_modules"],
        stage + "/node_modules",
        identifier = ctx["label"]["id"] + ":stage-node-modules",
    )
    return stage

def _react_native_hermes_runner_source():
    return """const path = require("node:path");
const {spawnSync} = require("node:child_process");
const {createRequire} = require("node:module");
const reactNativeManifest = require.resolve("react-native/package.json", {paths: [process.cwd()]});
const requireFromReactNative = createRequire(reactNativeManifest);
const hermesManifest = requireFromReactNative.resolve("hermes-compiler/package.json");
let relative;
if (process.platform === "darwin") {
  relative = "hermesc/osx-bin/hermesc";
} else if (process.platform === "linux" && process.arch === "x64") {
  relative = "hermesc/linux64-bin/hermesc";
} else if (process.platform === "win32" && process.arch === "x64") {
  relative = "hermesc/win64-bin/hermesc.exe";
} else {
  throw new Error(`Hermes compilation is not supported on ${process.platform}/${process.arch}`);
}
const executable = path.join(path.dirname(hermesManifest), relative);
const result = spawnSync(executable, process.argv.slice(2), {stdio: "inherit"});
if (result.error) throw result.error;
process.exit(result.status === null ? 1 : result.status);
"""

def _react_native_bundle_impl(ctx):
    tools = _react_native_tools(ctx)
    dependency = _react_native_dependency(ctx)
    platform = _react_native_attr(ctx, "platform", "")
    if platform not in ["ios", "android"]:
        fail(ctx["label"]["id"] + ": platform must be `ios` or `android`")
    files = glob(ctx["srcs"])
    entry = _package_relative(ctx, _react_native_attr(ctx, "entry", "index.js"))
    if entry not in files:
        files.append(entry)
    metro_config_attr = _react_native_attr(ctx, "metro_config", "")
    metro_config_source = _package_relative(ctx, metro_config_attr) if metro_config_attr else ""
    if metro_config_source and metro_config_source not in files:
        files.append(metro_config_source)
    stage = _react_native_stage_project(ctx, dependency, _unique(files + [dependency["package_json"], dependency["lockfile"]]))
    staged_entry = stage + "/" + _react_native_relative_path(ctx, entry)
    cli = stage + "/node_modules/react-native/cli.js"
    bundle_name = _react_native_attr(ctx, "bundle_name", "main.jsbundle")
    bundle_dir = ctx["build_dir"] + "/bundle"
    assets = ctx["build_dir"] + "/assets"
    packager_bundle = bundle_dir + "/" + bundle_name + ".packager"
    packager_map = bundle_dir + "/" + bundle_name + ".packager.map"
    final_bundle = bundle_dir + "/" + bundle_name
    source_map = bundle_dir + "/" + bundle_name + ".map"
    metro_wrapper = ctx["scratch_dir"] + "/metro.bundle.config.js"
    write_path(metro_wrapper, _react_native_metro_config_source())
    dev = _react_native_attr(ctx, "dev", False)
    minify = _react_native_attr(ctx, "minify", not dev)
    hermes = _react_native_attr(ctx, "hermes", True)
    prepare_path(bundle_dir, kind = "remove", identifier = ctx["label"]["id"] + ":bundle-clean")
    prepare_path(bundle_dir, kind = "directory", identifier = ctx["label"]["id"] + ":bundle-prepare")
    prepare_path(assets, kind = "remove", identifier = ctx["label"]["id"] + ":assets-clean")
    prepare_path(assets, kind = "directory", identifier = ctx["label"]["id"] + ":assets-prepare")
    argv = [
        tools["node"],
        execution_path(cli),
        "bundle",
        "--platform", platform,
        "--dev", "true" if dev else "false",
        "--minify", "true" if minify else "false",
        "--entry-file", execution_path(staged_entry),
        "--bundle-output", execution_path(packager_bundle if hermes else final_bundle),
        "--sourcemap-output", execution_path(packager_map if hermes else source_map),
        "--assets-dest", execution_path(assets),
        "--reset-cache",
        "--config", execution_path(metro_wrapper),
    ]
    metro_config = stage + "/" + _react_native_relative_path(ctx, metro_config_source) if metro_config_source else ""
    argv.extend(_react_native_attr(ctx, "bundle_args", []))
    metro_outputs = [assets]
    if hermes:
        metro_outputs.extend([packager_bundle, packager_map])
    else:
        metro_outputs.extend([final_bundle, source_map])
    run_action(
        argv = argv,
        inputs = _unique([stage, dependency["node_modules"], metro_wrapper]),
        outputs = metro_outputs,
        create_dirs = [ctx["scratch_dir"] + "/home"],
        cwd = stage,
        env = {
            "HOME": execution_path(ctx["scratch_dir"] + "/home"),
            "NODE_PATH": execution_path(dependency["node_modules"]),
            "ONCE_REACT_NATIVE_PROJECT_ROOT": execution_path(stage),
            "ONCE_REACT_NATIVE_NODE_MODULES": execution_path(dependency["node_modules"]),
            "ONCE_REACT_NATIVE_METRO_CONFIG": execution_path(metro_config) if metro_config else "",
            "PATH": _react_native_path_env(tools),
        },
        toolchain_identity = tools["identity"] + "\x00metro\x00" + dependency["react_native_version"] + "\x00platform\x00" + platform,
        identifier = ctx["label"]["id"] + ":metro-bundle",
    )
    if hermes:
        hermes_runner = ctx["scratch_dir"] + "/run-hermes.js"
        write_path(hermes_runner, _react_native_hermes_runner_source())
        compiler_map = final_bundle + ".compiler.map"
        run_action(
            argv = [
                tools["node"],
                execution_path(hermes_runner),
                "-w",
                "-emit-binary",
                "-max-diagnostic-width=80",
                "-out", execution_path(final_bundle),
                execution_path(packager_bundle),
                "-O",
                "-output-source-map",
            ],
            inputs = [hermes_runner, packager_bundle, dependency["node_modules"]],
            outputs = [final_bundle, final_bundle + ".map"],
            cwd = stage,
            env = {
                "HOME": execution_path(ctx["scratch_dir"] + "/home"),
                "PATH": _react_native_path_env(tools),
            },
            create_dirs = [ctx["scratch_dir"] + "/home"],
            toolchain_identity = tools["identity"] + "\x00hermes-compiler\x00" + dependency["hermes_version"],
            identifier = ctx["label"]["id"] + ":hermes-compile",
        )
        copy_path(
            final_bundle + ".map",
            compiler_map,
            inputs = [final_bundle + ".map"],
            identifier = ctx["label"]["id"] + ":hermes-map",
        )
        compose = stage + "/node_modules/react-native/scripts/compose-source-maps.js"
        run_action(
            argv = [
                tools["node"],
                execution_path(compose),
                execution_path(packager_map),
                execution_path(compiler_map),
                "-o", execution_path(source_map),
            ],
            inputs = [compose, packager_map, compiler_map, dependency["node_modules"]],
            outputs = [source_map],
            toolchain_identity = tools["identity"] + "\x00hermes-source-map\x00" + dependency["hermes_version"],
            identifier = ctx["label"]["id"] + ":compose-source-maps",
        )
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "react_native_bundle",
        "platform": platform,
        "sources": files,
        "bundle": final_bundle,
        "source_map": source_map,
        "assets": assets,
        "react_native_bundle": True,
    }

def _react_native_codegen_impl(ctx):
    tools = _react_native_tools(ctx)
    dependency = _react_native_dependency(ctx)
    platform = _react_native_attr(ctx, "platform", "")
    if platform not in ["ios", "android", "all"]:
        fail(ctx["label"]["id"] + ": platform must be `ios`, `android`, or `all`")
    files = _unique(glob(ctx["srcs"]) + [dependency["package_json"], dependency["lockfile"]])
    stage = _react_native_stage_project(ctx, dependency, files)
    output = declare_output("generated")
    script = stage + "/node_modules/react-native/scripts/generate-codegen-artifacts.js"
    prepare_path(output, kind = "remove", identifier = ctx["label"]["id"] + ":codegen-clean")
    run_action(
        argv = [
            tools["node"],
            execution_path(script),
            "--path", execution_path(stage),
            "--targetPlatform", platform,
            "--outputPath", execution_path(output),
            "--source", "app",
            "--forceOutputPath",
        ],
        inputs = [stage, dependency["node_modules"]],
        outputs = [output],
        create_dirs = [output],
        cwd = stage,
        env = {
            "CI": "1",
            "PATH": _react_native_path_env(tools),
        },
        toolchain_identity = tools["identity"] + "\x00react-native-codegen\x00" + dependency["react_native_version"],
        identifier = ctx["label"]["id"] + ":codegen",
    )
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "react_native_codegen",
        "platform": platform,
        "generated_sources": output,
        "react_native_codegen": True,
    }

def _react_native_config_normalizer():
    return """const {spawnSync} = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const [cli, projectRoot, output, reactNativeVersion] = process.argv.slice(2);
const result = spawnSync(process.execPath, [cli, "config"], {
  cwd: projectRoot,
  encoding: "utf8",
  env: process.env,
});
if (result.status !== 0) {
  process.stderr.write(result.stderr || result.stdout);
  process.exit(result.status || 1);
}
const config = JSON.parse(result.stdout);
const realNodeModules = fs.realpathSync(path.join(projectRoot, "node_modules"));
const dependencyRoot = path.dirname(realNodeModules);
const portablePath = (root, prefix, value) => {
  const relative = path.relative(root, value);
  if (relative === "") return prefix;
  if (relative !== ".." && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative)) {
    return [prefix, relative].filter(Boolean).join("/").split(path.sep).join("/");
  }
  return null;
};
const normalizePath = value => {
  if (!value || typeof value !== "string" || !path.isAbsolute(value)) return value;
  const canonical = fs.existsSync(value) ? fs.realpathSync(value) : value;
  const installed = portablePath(realNodeModules, "node_modules", canonical);
  if (installed) return installed;
  const dependency = portablePath(dependencyRoot, "", canonical);
  if (dependency) return dependency;
  const project = portablePath(projectRoot, "", canonical);
  if (project) return project;
  throw new Error(`React Native config returned a non-portable path: ${value}`);
};
const modules = Object.keys(config.dependencies || {}).sort().map(name => {
  const dependency = config.dependencies[name] || {};
  const ios = dependency.platforms?.ios || {};
  const android = dependency.platforms?.android || {};
  return {
    name,
    root: normalizePath(dependency.root),
    platforms: {
      ios: {
        podspec: normalizePath(ios.podspecPath),
      },
      android: {
        source_dir: normalizePath(android.sourceDir),
        library_name: android.libraryName || "",
        cmake_lists: normalizePath(android.cmakeListsPath),
        component_descriptors: android.componentDescriptors || [],
        pure_cxx: Boolean(android.isPureCxxDependency),
      },
    },
  };
});
fs.mkdirSync(path.dirname(output), {recursive: true});
fs.writeFileSync(output, JSON.stringify({
  schema: "once.react_native.modules.v1",
  react_native_version: reactNativeVersion,
  modules,
}, null, 2) + "\\n");
"""

def _react_native_autolinking_impl(ctx):
    tools = _react_native_tools(ctx)
    dependency = _react_native_dependency(ctx)
    files = _unique(glob(ctx["srcs"]) + [dependency["package_json"], dependency["lockfile"]])
    stage = _react_native_stage_project(ctx, dependency, files)
    output = declare_output("react-native-modules.json")
    normalizer = ctx["scratch_dir"] + "/normalize-react-native-config.js"
    write_path(normalizer, _react_native_config_normalizer())
    cli = stage + "/node_modules/react-native/cli.js"
    run_action(
        argv = [
            tools["node"],
            execution_path(normalizer),
            execution_path(cli),
            execution_path(stage),
            execution_path(output),
            dependency["react_native_version"],
        ],
        inputs = [normalizer, stage, dependency["node_modules"]],
        outputs = [output],
        cwd = stage,
        env = {
            "CI": "1",
            "PATH": _react_native_path_env(tools),
        },
        toolchain_identity = tools["identity"] + "\x00react-native-autolinking\x00" + dependency["react_native_version"],
        identifier = ctx["label"]["id"] + ":autolinking-snapshot",
    )
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "react_native_autolinking",
        "modules_snapshot": output,
        "react_native_autolinking": True,
    }

def _react_native_native_deps(ctx):
    dependency = None
    codegen = []
    autolinking = []
    bundles = []
    for dep in ctx["deps"]:
        if dep.get("react_native_dependency_set"):
            if dependency != None:
                fail(ctx["label"]["id"] + ": expected exactly one react_native_dependencies dependency")
            dependency = dep
        if dep.get("react_native_codegen"):
            codegen.append(dep)
        if dep.get("react_native_autolinking"):
            autolinking.append(dep)
        if dep.get("react_native_bundle"):
            bundles.append(dep)
    if dependency == None:
        fail(ctx["label"]["id"] + ": expected a react_native_dependencies dependency")
    return {
        "dependency": dependency,
        "codegen": codegen,
        "autolinking": autolinking,
        "bundles": bundles,
    }

def _react_native_bundle_sources(deps, platform):
    sources = []
    for bundle in deps["bundles"]:
        if bundle.get("platform") == platform:
            sources.extend(bundle.get("sources") or [])
    return _unique(sources)

def _react_native_apple_application_impl(ctx):
    deps = _react_native_native_deps(ctx)
    dependency = deps["dependency"]
    product_name = _react_native_attr(ctx, "product_name", ctx["label"]["name"])
    bundle_id = _react_native_attr(ctx, "bundle_id", "")
    workspace_name = _react_native_attr(ctx, "workspace", product_name + ".xcworkspace")
    scheme = _react_native_attr(ctx, "scheme", product_name)
    configuration = _react_native_attr(ctx, "configuration", "Debug")
    sdk = _react_native_attr(ctx, "sdk", "iphonesimulator")
    if sdk not in ["iphonesimulator", "iphoneos"]:
        fail(ctx["label"]["id"] + ": sdk must be `iphonesimulator` or `iphoneos`")
    native_root = _react_native_attr(ctx, "native_root", "ios")
    app = ctx["build_dir"] + "/" + product_name + ".app"
    if ctx["capability"] == "run":
        if sdk != "iphonesimulator":
            fail(ctx["label"]["id"] + ": run requires sdk `iphonesimulator`; device products must be installed through a signing and distribution workflow")
        xcrun = host_which("xcrun")
        destination = _react_native_attr(ctx, "simulator", "booted")
        run_action(
            argv = [xcrun, "simctl", "install", destination, execution_path(app)],
            inputs = [app],
            outputs = [],
            cacheable = False,
            toolchain_identity = "once.react-native.apple.run.v1\x00" + xcrun,
            identifier = ctx["label"]["id"] + ":simulator-install",
        )
        run_action(
            argv = [xcrun, "simctl", "launch", destination, bundle_id],
            inputs = [app],
            outputs = [],
            cacheable = False,
            toolchain_identity = "once.react-native.apple.run.v1\x00" + xcrun,
            identifier = ctx["label"]["id"] + ":simulator-launch",
        )
        return {
            "label_id": ctx["label"]["id"],
            "target_kind": "react_native_apple_application",
            "app": app,
            "bundle_id": bundle_id,
        }
    bundle_sources = _react_native_bundle_sources(deps, "ios") if "debug" not in configuration.lower() else []
    files = _unique(_react_native_source_files(ctx, [
        native_root + "/Pods/**/*",
        native_root + "/build/**/*",
    ]) + bundle_sources + [dependency["package_json"], dependency["lockfile"]])
    stage = _react_native_stage_project(ctx, dependency, files)
    staged_native_root = stage + "/" + native_root
    tools = _react_native_tools(ctx)
    bundle = _resolve_host_executable(_react_native_attr(ctx, "bundle", "bundle"))
    if not bundle:
        fail(ctx["label"]["id"] + ": Bundler executable was not found")
    xcodebuild = _resolve_host_executable(_react_native_attr(ctx, "xcodebuild", "xcodebuild"))
    if not xcodebuild:
        fail(ctx["label"]["id"] + ": xcodebuild was not found")
    bundle_version = host_command([bundle, "--version"]).strip()
    xcode_version = host_command([xcodebuild, "-version"]).strip()
    mise = host_which_optional("mise") or ""
    ruby_path_tools = [bundle] + ([mise] if mise else [])
    vendor_bundle = stage + "/vendor/bundle"
    action_home = ctx["scratch_dir"] + "/home"
    bundle_argv = [bundle, "install"]
    if not _react_native_attr(ctx, "allow_network", False):
        bundle_argv.append("--local")
    run_action(
        argv = bundle_argv,
        inputs = [stage + "/Gemfile", stage + "/Gemfile.lock"],
        outputs = [vendor_bundle],
        clean_paths = [vendor_bundle],
        create_dirs = [action_home],
        cwd = stage,
        env = {
            "BUNDLE_GEMFILE": execution_path(stage + "/Gemfile"),
            "BUNDLE_FROZEN": "true",
            "BUNDLE_PATH": execution_path(vendor_bundle),
            "BUNDLE_VERSION": "system",
            "HOME": execution_path(action_home),
            "LANG": "en_US.UTF-8",
            "LC_ALL": "en_US.UTF-8",
            "PATH": _react_native_path_env(tools, ruby_path_tools),
        },
        toolchain_identity = tools["identity"] + "\x00bundler\x00" + bundle_version + "\x00mise\x00" + mise,
        identifier = ctx["label"]["id"] + ":bundle-install",
    )
    pod_argv = [bundle, "exec", "pod", "install", "--deployment"]
    if not _react_native_attr(ctx, "allow_network", False):
        pod_argv.append("--no-repo-update")
    pod_outputs = [
        staged_native_root + "/Pods",
        staged_native_root + "/" + workspace_name,
        staged_native_root + "/build/generated",
    ]
    run_action(
        argv = pod_argv,
        inputs = [stage, vendor_bundle, dependency["node_modules"]],
        outputs = pod_outputs,
        cwd = staged_native_root,
        env = {
            "BUNDLE_GEMFILE": execution_path(stage + "/Gemfile"),
            "BUNDLE_FROZEN": "true",
            "BUNDLE_PATH": execution_path(vendor_bundle),
            "BUNDLE_VERSION": "system",
            "CI": "1",
            "HOME": execution_path(action_home),
            "LANG": "en_US.UTF-8",
            "LC_ALL": "en_US.UTF-8",
            "NODE_BINARY": tools["node"],
            "PATH": _react_native_path_env(tools, ruby_path_tools),
        },
        toolchain_identity = tools["identity"] + "\x00cocoapods\x00" + bundle_version + "\x00react-native\x00" + dependency["react_native_version"],
        identifier = ctx["label"]["id"] + ":pod-install",
    )
    derived_data = ctx["build_dir"] + "/DerivedData"
    built_app = derived_data + "/Build/Products/" + configuration + "-" + sdk + "/" + product_name + ".app"
    log = declare_output("xcodebuild.log")
    native_inputs = _unique([stage, vendor_bundle] + [item["generated_sources"] for item in deps["codegen"]] + [item["modules_snapshot"] for item in deps["autolinking"]])
    xcode_argv = [
        xcodebuild,
        "-workspace", execution_path(staged_native_root + "/" + workspace_name),
        "-scheme", scheme,
        "-configuration", configuration,
        "-sdk", sdk,
        "-destination", "generic/platform=iOS Simulator" if sdk == "iphonesimulator" else "generic/platform=iOS",
        "-derivedDataPath", execution_path(derived_data),
    ]
    if sdk == "iphonesimulator":
        xcode_argv.append("CODE_SIGNING_ALLOWED=NO")
    xcode_argv.append("build")
    run_action(
        argv = xcode_argv,
        inputs = native_inputs,
        outputs = [built_app, log],
        clean_paths = [derived_data],
        env = {
            "CI": "1",
            "HOME": execution_path(action_home),
            "LANG": "en_US.UTF-8",
            "LC_ALL": "en_US.UTF-8",
            "NODE_BINARY": tools["node"],
            "PATH": _react_native_path_env(tools, [bundle, xcodebuild]),
        },
        stdout = log,
        stderr = log,
        toolchain_identity = tools["identity"] + "\x00xcodebuild\x00" + xcode_version + "\x00react-native\x00" + dependency["react_native_version"] + "\x00hermes\x00" + dependency["hermes_version"],
        identifier = ctx["label"]["id"] + ":xcodebuild",
    )
    copy_path(
        built_app,
        app,
        kind = "tree",
        inputs = [built_app],
        identifier = ctx["label"]["id"] + ":application-product",
    )
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "react_native_apple_application",
        "app": app,
        "bundle_id": bundle_id,
        "logs": [log],
        "react_native_apple_application": True,
    }

def _react_native_android_application_impl(ctx):
    deps = _react_native_native_deps(ctx)
    dependency = deps["dependency"]
    application_id = _react_native_attr(ctx, "application_id", "")
    configuration = _react_native_attr(ctx, "configuration", "debug")
    native_root = _react_native_attr(ctx, "native_root", "android")
    module = _react_native_attr(ctx, "module", "app")
    task_suffix = configuration[0:1].upper() + configuration[1:]
    apk = ctx["build_dir"] + "/" + module + "-" + configuration + ".apk"
    if ctx["capability"] == "run":
        adb = _resolve_host_executable(_react_native_attr(ctx, "adb", "adb"))
        if not adb:
            fail(ctx["label"]["id"] + ": Android Debug Bridge executable was not found")
        adb_version = host_command([adb, "version"]).strip()
        adb_argv = [adb]
        adb_serial = _react_native_attr(ctx, "adb_serial", "")
        if adb_serial:
            adb_argv.extend(["-s", adb_serial])
        run_identity = "once.react-native.android.run.v2\x00" + adb_version
        device_ready = declare_output("run/device-ready")
        installed = declare_output("run/installed")
        reversed_port = declare_output("run/metro-reversed")
        run_action(
            argv = adb_argv + ["wait-for-device"],
            outputs = [],
            cacheable = False,
            toolchain_identity = run_identity,
            identifier = ctx["label"]["id"] + ":android-wait",
        )
        run_action(
            argv = adb_argv + ["shell", _android_device_ready_script()],
            outputs = [],
            cacheable = False,
            toolchain_identity = run_identity,
            identifier = ctx["label"]["id"] + ":android-ready",
        )
        write_path(device_ready, "")
        run_action(
            argv = adb_argv + ["install", "-r", "-d", execution_path(apk)],
            inputs = [apk, device_ready],
            outputs = [],
            cacheable = False,
            toolchain_identity = run_identity,
            identifier = ctx["label"]["id"] + ":android-install",
        )
        write_path(installed, "")
        metro_port = str(_react_native_attr(ctx, "metro_port", 8081))
        run_action(
            argv = adb_argv + ["reverse", "tcp:" + metro_port, "tcp:" + metro_port],
            inputs = [installed],
            outputs = [],
            cacheable = False,
            toolchain_identity = run_identity,
            identifier = ctx["label"]["id"] + ":android-metro-reverse",
        )
        write_path(reversed_port, "")
        activity = _react_native_attr(ctx, "launch_activity", "")
        component = _android_launch_component(application_id, activity)
        launch = adb_argv + ["shell", "am", "start", "-n", component] if component else adb_argv + ["shell", _android_launcher_script(application_id)]
        run_action(
            argv = launch,
            inputs = [reversed_port],
            outputs = [],
            cacheable = False,
            toolchain_identity = run_identity,
            identifier = ctx["label"]["id"] + ":android-launch",
        )
        return {
            "label_id": ctx["label"]["id"],
            "target_kind": "react_native_android_application",
            "apk": apk,
            "application_id": application_id,
        }
    bundle_sources = _react_native_bundle_sources(deps, "android") if "debug" not in configuration.lower() else []
    files = _unique(_react_native_source_files(ctx, [
        native_root + "/.gradle/**/*",
        native_root + "/build/**/*",
        native_root + "/**/build/**/*",
    ]) + bundle_sources + [dependency["package_json"], dependency["lockfile"]])
    stage = _react_native_stage_project(ctx, dependency, files)
    staged_native_root = stage + "/" + native_root
    apk_path_attr = _react_native_attr(ctx, "apk_path", "")
    built_apk = stage + "/" + _react_native_relative_path(ctx, _package_relative(ctx, apk_path_attr)) if apk_path_attr else staged_native_root + "/" + module + "/build/outputs/apk/" + configuration + "/" + module + "-" + configuration + ".apk"
    tools = _react_native_tools(ctx)
    java = _resolve_host_executable(_react_native_attr(ctx, "java", "java"))
    if not java:
        fail(ctx["label"]["id"] + ": Java executable was not found")
    java_version = host_command([java, "-version"], merge_stderr = True).strip()
    wrapper_jar = staged_native_root + "/gradle/wrapper/gradle-wrapper.jar"
    android_home = _react_native_attr(ctx, "android_sdk", "") or host_env("ANDROID_HOME")
    if not android_home:
        fail(ctx["label"]["id"] + ": Android Software Development Kit root was not found; set android_sdk or ANDROID_HOME")
    android_ndk = _react_native_attr(ctx, "android_ndk", "") or host_env("ANDROID_NDK_HOME")
    gradle_user_home = host_env("GRADLE_USER_HOME") or (host_env("HOME") + "/.gradle")
    log = declare_output("gradle.log")
    argv = [
        java,
        "-classpath", execution_path(wrapper_jar),
        "org.gradle.wrapper.GradleWrapperMain",
        module + ":assemble" + task_suffix,
        "--stacktrace",
    ]
    architectures = _react_native_attr(ctx, "architectures", [])
    if architectures:
        argv.append("-PreactNativeArchitectures=" + ",".join(architectures))
    if not _react_native_attr(ctx, "allow_network", False):
        argv.append("--offline")
    argv.extend(_react_native_attr(ctx, "gradle_args", []))
    native_inputs = _unique([stage, dependency["node_modules"]] + [item["generated_sources"] for item in deps["codegen"]] + [item["modules_snapshot"] for item in deps["autolinking"]])
    run_action(
        argv = argv,
        inputs = native_inputs,
        outputs = [built_apk, log],
        cwd = staged_native_root,
        env = {
            "ANDROID_HOME": android_home,
            "ANDROID_NDK_HOME": android_ndk,
            "CI": "1",
            "GRADLE_USER_HOME": gradle_user_home,
            "HOME": host_env("HOME"),
            "NODE_BINARY": tools["node"],
            "PATH": _react_native_path_env(tools, [java]),
        },
        stdout = log,
        stderr = log,
        toolchain_identity = tools["identity"] + "\x00java\x00" + java_version + "\x00android-sdk\x00" + android_home + "\x00android-ndk\x00" + android_ndk + "\x00react-native\x00" + dependency["react_native_version"] + "\x00hermes\x00" + dependency["hermes_version"],
        identifier = ctx["label"]["id"] + ":gradle",
    )
    copy_path(
        built_apk,
        apk,
        inputs = [built_apk],
        identifier = ctx["label"]["id"] + ":application-package",
    )
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "react_native_android_application",
        "apk": apk,
        "application_id": application_id,
        "logs": [log],
        "react_native_android_application": True,
    }

def _react_native_metro_config_source():
    return """const path = require("node:path");
const fs = require("node:fs");
const {createRequire} = require("node:module");
const projectRoot = path.resolve(process.cwd(), process.env.ONCE_REACT_NATIVE_PROJECT_ROOT);
const nodeModules = fs.realpathSync(path.resolve(process.cwd(), process.env.ONCE_REACT_NATIVE_NODE_MODULES));
const dependencyRoot = path.dirname(nodeModules);
const requireFromDependencies = createRequire(path.join(nodeModules, "package.json"));
const {getDefaultConfig, mergeConfig} = requireFromDependencies("@react-native/metro-config");
const userConfigPath = process.env.ONCE_REACT_NATIVE_METRO_CONFIG;
const userConfig = userConfigPath ? require(path.resolve(projectRoot, userConfigPath)) : {};
module.exports = mergeConfig(getDefaultConfig(projectRoot), userConfig, {
  projectRoot,
  watchFolders: [projectRoot, dependencyRoot],
  resolver: {
    ...(userConfig.resolver || {}),
    nodeModulesPaths: [nodeModules, ...((userConfig.resolver || {}).nodeModulesPaths || [])],
  },
});
"""

def _react_native_metro_impl(ctx):
    tools = _react_native_tools(ctx)
    dependency = _react_native_dependency(ctx)
    if ctx["capability"] != "run":
        return {
            "label_id": ctx["label"]["id"],
            "target_kind": "react_native_metro",
            "project_root": _react_native_package_root(ctx),
            "react_native_metro": True,
        }
    config = ctx["scratch_dir"] + "/metro.once.config.js"
    write_path(config, _react_native_metro_config_source())
    stage = ctx["build_dir"] + "/project"
    metro_config_attr = _react_native_attr(ctx, "metro_config", "")
    metro_config = _package_relative(ctx, metro_config_attr) if metro_config_attr else ""
    prepare_path(stage, kind = "remove", identifier = ctx["label"]["id"] + ":project-clean")
    prepare_path(stage, kind = "directory", identifier = ctx["label"]["id"] + ":project-prepare")
    _react_native_stage_files(ctx, stage, [dependency["package_json"], dependency["lockfile"]])
    dependency_stage = ctx["build_dir"] + "/dependencies"
    stable_node_modules = dependency_stage + "/node_modules"
    # Keep the live server on an immutable dependency snapshot if another
    # target refreshes the shared dependency output while Metro is running.
    copy_path(
        dependency["javascript_root"],
        dependency_stage,
        kind = "tree",
        inputs = [dependency["javascript_root"]],
        identifier = ctx["label"]["id"] + ":snapshot-dependencies",
    )
    link_path(
        stable_node_modules,
        stage + "/node_modules",
        identifier = ctx["label"]["id"] + ":stage-node-modules",
    )
    cli = stable_node_modules + "/react-native/cli.js"
    project_root = _react_native_package_root(ctx)
    argv = [
        tools["node"],
        execution_path(cli),
        "start",
        "--config", execution_path(config),
        "--projectRoot", execution_path(project_root),
        "--port", str(_react_native_attr(ctx, "port", 8081)),
        "--host", _react_native_attr(ctx, "host", "127.0.0.1"),
        "--no-interactive",
    ]
    if _react_native_attr(ctx, "reset_cache", False):
        argv.append("--reset-cache")
    run_action(
        argv = argv,
        inputs = _unique(glob(ctx["srcs"]) + [config, stage] + ([metro_config] if metro_config else [])),
        outputs = [],
        create_dirs = [ctx["scratch_dir"] + "/home"],
        cwd = stage,
        env = {
            "HOME": execution_path(ctx["scratch_dir"] + "/home"),
            "NODE_PATH": execution_path(stable_node_modules),
            "ONCE_REACT_NATIVE_PROJECT_ROOT": execution_path(project_root),
            "ONCE_REACT_NATIVE_NODE_MODULES": execution_path(stable_node_modules),
            "ONCE_REACT_NATIVE_METRO_CONFIG": execution_path(metro_config) if metro_config else "",
            "PATH": _react_native_path_env(tools),
        },
        cacheable = False,
        toolchain_identity = tools["identity"] + "\x00metro-server\x00" + dependency["react_native_version"],
        identifier = ctx["label"]["id"] + ":metro",
    )
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "react_native_metro",
        "project_root": project_root,
        "react_native_metro": True,
    }

_REACT_NATIVE_EXAMPLE = [
    example(
        "react-native-application-minimal",
        name = "React Native New Architecture application",
        use_when = "You want a current React Native application with Hermes, Fabric, iOS, Android, and Metro development mode.",
    ),
]

_REACT_NATIVE_DEPENDENCY_REFERENCES = [
    source_reference(
        "React Native Community command-line tool",
        "autolinking",
        "https://github.com/react-native-community/cli/blob/main/docs/autolinking.md",
        "Import React Native module discovery and platform metadata.",
    ),
    source_reference(
        "React Native",
        "Codegen",
        "https://reactnative.dev/docs/the-new-architecture/what-is-codegen",
        "Generate New Architecture native interfaces from JavaScript and TypeScript specifications.",
    ),
    source_reference(
        "npm",
        "Clean installs",
        "https://docs.npmjs.com/cli/commands/npm-ci",
        "Install npm dependencies without changing the lockfile.",
    ),
    source_reference(
        "pnpm",
        "Frozen installs",
        "https://pnpm.io/cli/install#--frozen-lockfile",
        "Install pnpm dependencies without changing the lockfile.",
    ),
    source_reference(
        "Yarn",
        "Immutable installs",
        "https://yarnpkg.com/cli/install",
        "Install Yarn dependencies without changing the lockfile.",
    ),
    source_reference(
        "Bun",
        "Frozen installs",
        "https://bun.sh/docs/pm/cli/install",
        "Install Bun dependencies without changing the lockfile.",
    ),
]

react_native_dependencies = target_kind(
    docs = "Installs the exact JavaScript dependency graph from npm, pnpm, Yarn, or Bun lockfiles and exposes locked React Native and Hermes identities.",
    attrs = [
        attr("package_json", "string", default = "\"package.json\"", docs = "Package-relative JavaScript package manifest.", configurable = False),
        attr("package_manager", "string", default = "\"auto\"", docs = "`auto`, `npm`, `pnpm`, `yarn`, or `bun`. Auto uses packageManager, the lockfile name, or the only declared supported lockfile.", configurable = False),
        attr("package_manager_executable", "string", default = "\"\"", docs = "Optional package manager executable override.", configurable = False),
        attr("lockfile", "string", default = "\"\"", docs = "Package-relative lockfile. Auto-detected when omitted.", configurable = False),
        attr("npmrc", "string", docs = "Optional package-relative npm configuration file without authentication tokens or passwords.", configurable = False),
        attr("package_manager_files", "list<string>", default = "[]", docs = "Additional package manager configuration files without authentication tokens or passwords.", configurable = False),
        attr("modules_snapshot", "string", docs = "Optional checked-in normalized react-native config snapshot.", configurable = False),
        attr("resolver_inputs", "list<string>", default = "[]", docs = "Manifest, lockfile, package manager configuration, and module snapshot text supplied to the resolver.", configurable = False),
        attr("node", "string", default = "\"node\"", docs = "Node.js executable.", configurable = False),
        attr("allow_network", "bool", default = "false", docs = "Allow the package manager to fetch packages missing from its local cache.", configurable = False),
        attr("install_args", "list<string>", default = "[]", docs = "Additional frozen-install arguments.", configurable = False),
        attr("_react_native_resolved", "bool", default = "false", docs = "Resolver-owned locked-graph marker.", configurable = False),
        attr("_package_manager", "string", default = "\"npm\"", docs = "Resolver-owned package manager.", configurable = False),
        attr("_lockfile", "string", default = "\"package-lock.json\"", docs = "Resolver-owned lockfile path.", configurable = False),
        attr("_react_native_version", "string", default = "\"\"", docs = "Resolver-owned React Native version.", configurable = False),
        attr("_hermes_version", "string", default = "\"\"", docs = "Resolver-owned Hermes compiler and runtime version.", configurable = False),
        attr("_react_native_module_targets", "list<string>", default = "[]", docs = "Resolver-owned synthetic module target names.", configurable = False),
    ],
    deps = [dep("deps", ["react_native_module"], "Locked native modules emitted by the resolver.")],
    providers = ["javascript_dependency_set", "react_native_dependency_set"],
    capabilities = [capability("build", ["node_modules"])],
    tools = [
        _REACT_NATIVE_NODE_TOOL,
        _REACT_NATIVE_NPM_TOOL,
        _REACT_NATIVE_PNPM_TOOL,
        _REACT_NATIVE_YARN_TOOL,
        _REACT_NATIVE_BUN_TOOL,
    ],
    examples = _REACT_NATIVE_EXAMPLE,
    source_references = _REACT_NATIVE_DEPENDENCY_REFERENCES,
    resolver = _react_native_dependencies_resolver,
    impl = _react_native_dependencies_impl,
)

react_native_module = target_kind(
    docs = "One locked React Native native module generated from the checked-in autolinking snapshot.",
    attrs = [
        attr("module_name", "string", required = True, docs = "JavaScript package name.", configurable = False),
        attr("version", "string", required = True, docs = "Locked package version.", configurable = False),
        attr("source_root", "string", required = True, docs = "Installed package source root.", configurable = False),
        attr("ios_podspec", "string", docs = "iOS podspec path reported by autolinking.", configurable = False),
        attr("android_source_dir", "string", docs = "Android project directory reported by autolinking.", configurable = False),
        attr("android_library_name", "string", docs = "New Architecture Android library name.", configurable = False),
        attr("android_cmake_lists", "string", docs = "CMake project path reported by autolinking.", configurable = False),
        attr("android_component_descriptors", "list<string>", default = "[]", docs = "Fabric component descriptor names.", configurable = False),
        attr("pure_cxx", "bool", default = "false", docs = "Whether the module is implemented entirely in C++.", configurable = False),
        attr("_react_native_locked", "bool", default = "false", docs = "Resolver-owned locked-module marker.", configurable = False),
    ],
    providers = ["react_native_module"],
    capabilities = [capability("build", [])],
    examples = _REACT_NATIVE_EXAMPLE,
    source_references = _REACT_NATIVE_DEPENDENCY_REFERENCES,
    impl = _react_native_module_impl,
)

react_native_bundle = target_kind(
    docs = "Bundles JavaScript and platform assets with Metro, then compiles the release bundle with the Hermes compiler paired to React Native.",
    attrs = [
        attr("platform", "string", required = True, docs = "`ios` or `android`.", configurable = False, allowed_values = ["ios", "android"]),
        attr("entry", "string", default = "\"index.js\"", docs = "Package-relative JavaScript entry file.", configurable = False),
        attr("bundle_name", "string", default = "\"main.jsbundle\"", docs = "Final bundle filename.", configurable = False),
        attr("metro_config", "string", docs = "Package-relative Metro configuration.", configurable = False),
        attr("dev", "bool", default = "false", docs = "Generate a development bundle.", configurable = True),
        attr("minify", "bool", default = "true", docs = "Minify the Metro bundle.", configurable = True),
        attr("hermes", "bool", default = "true", docs = "Compile the Metro output to Hermes bytecode.", configurable = False),
        attr("bundle_args", "list<string>", default = "[]", docs = "Additional Metro bundle arguments.", configurable = True),
        attr("node", "string", default = "\"node\"", docs = "Node.js executable.", configurable = False),
    ],
    deps = [dep("deps", ["react_native_dependency_set"], "Locked JavaScript dependencies used by Metro and Hermes.")],
    providers = ["react_native_bundle"],
    capabilities = [capability("build", ["default", "bundle", "source_map", "assets"])],
    tools = [_REACT_NATIVE_NODE_TOOL],
    examples = _REACT_NATIVE_EXAMPLE,
    impl = _react_native_bundle_impl,
)

react_native_codegen = target_kind(
    docs = "Runs the React Native New Architecture code generator for application and native-module specifications.",
    attrs = [
        attr("platform", "string", required = True, docs = "`ios`, `android`, or `all`.", configurable = False, allowed_values = ["ios", "android", "all"]),
        attr("node", "string", default = "\"node\"", docs = "Node.js executable.", configurable = False),
    ],
    deps = [dep("deps", ["react_native_dependency_set"], "Locked React Native packages containing the code generator.")],
    providers = ["react_native_codegen"],
    capabilities = [capability("build", ["default", "generated_sources"])],
    tools = [_REACT_NATIVE_NODE_TOOL],
    examples = _REACT_NATIVE_EXAMPLE,
    source_references = _REACT_NATIVE_DEPENDENCY_REFERENCES,
    impl = _react_native_codegen_impl,
)

react_native_autolinking = target_kind(
    docs = "Executes React Native module discovery as an explicit action and emits a normalized app-scoped module snapshot.",
    attrs = [
        attr("node", "string", default = "\"node\"", docs = "Node.js executable.", configurable = False),
    ],
    deps = [dep("deps", ["react_native_dependency_set"], "Locked packages inspected for native modules.")],
    providers = ["react_native_autolinking"],
    capabilities = [capability("build", ["default", "modules_snapshot"])],
    tools = [_REACT_NATIVE_NODE_TOOL],
    examples = _REACT_NATIVE_EXAMPLE,
    source_references = _REACT_NATIVE_DEPENDENCY_REFERENCES,
    impl = _react_native_autolinking_impl,
)

react_native_apple_application = target_kind(
    docs = "Builds and runs a current React Native iOS native project with CocoaPods, New Architecture code generation, and Hermes.",
    attrs = [
        attr("bundle_id", "string", required = True, docs = "Application bundle identifier.", configurable = False),
        attr("product_name", "string", docs = "Apple product name. Defaults to the target name.", configurable = False),
        attr("native_root", "string", default = "\"ios\"", docs = "Package-relative iOS project directory.", configurable = False),
        attr("workspace", "string", docs = "Workspace filename. Defaults to product_name.xcworkspace.", configurable = False),
        attr("scheme", "string", docs = "Xcode scheme. Defaults to product_name.", configurable = False),
        attr("configuration", "string", default = "\"Debug\"", docs = "Xcode build configuration.", configurable = True),
        attr("sdk", "string", default = "\"iphonesimulator\"", docs = "Apple software development kit identifier.", configurable = False, allowed_values = ["iphonesimulator", "iphoneos"]),
        attr("simulator", "string", default = "\"booted\"", docs = "Simulator identifier used by the run capability.", configurable = False),
        attr("exclude_srcs", "list<string>", default = "[]", docs = "Additional source patterns excluded while staging the native project.", configurable = False),
        attr("node", "string", default = "\"node\"", docs = "Node.js executable.", configurable = False),
        attr("bundle", "string", default = "\"bundle\"", docs = "Ruby Bundler executable.", configurable = False),
        attr("xcodebuild", "string", default = "\"xcodebuild\"", docs = "Xcode build executable.", configurable = False),
        attr("allow_network", "bool", default = "false", docs = "Allow Bundler and CocoaPods to fetch missing dependencies.", configurable = False),
    ],
    deps = [dep("deps", ["react_native_dependency_set", "react_native_codegen", "react_native_autolinking", "react_native_bundle"], "React Native packages, generated sources, module discovery, and optional release bundles.")],
    providers = ["react_native_apple_application"],
    capabilities = [
        capability("build", ["default", "app", "logs"]),
        capability("run", ["default"], ["app"]),
    ],
    tools = [_REACT_NATIVE_NODE_TOOL, _REACT_NATIVE_XCODE_TOOL, _REACT_NATIVE_BUNDLE_TOOL],
    examples = _REACT_NATIVE_EXAMPLE,
    impl = _react_native_apple_application_impl,
)

react_native_android_application = target_kind(
    docs = "Builds and runs a current React Native Android native project through its pinned Gradle wrapper, New Architecture code generation, CMake, Prefab, and Hermes.",
    attrs = [
        attr("application_id", "string", required = True, docs = "Android application identifier.", configurable = False),
        attr("native_root", "string", default = "\"android\"", docs = "Package-relative Android project directory.", configurable = False),
        attr("module", "string", default = "\"app\"", docs = "Gradle application module.", configurable = False),
        attr("configuration", "string", default = "\"debug\"", docs = "Android build configuration.", configurable = True),
        attr("apk_path", "string", docs = "Optional package-relative Gradle application package output path for flavors or unsigned builds.", configurable = True),
        attr("architectures", "list<string>", default = "[]", docs = "Android native architectures built by CMake.", configurable = False),
        attr("exclude_srcs", "list<string>", default = "[]", docs = "Additional source patterns excluded while staging the native project.", configurable = False),
        attr("launch_activity", "string", docs = "Optional fully qualified activity launched by the run capability.", configurable = False),
        attr("metro_port", "int", default = "8081", docs = "Metro server port forwarded to the Android device by the run capability.", configurable = False),
        attr("node", "string", default = "\"node\"", docs = "Node.js executable.", configurable = False),
        attr("java", "string", default = "\"java\"", docs = "Java executable used to launch the pinned Gradle wrapper.", configurable = False),
        attr("adb", "string", default = "\"adb\"", docs = "Android Debug Bridge executable.", configurable = False),
        attr("adb_serial", "string", docs = "Optional Android Debug Bridge device serial selected by the run capability.", configurable = False),
        attr("android_sdk", "string", docs = "Android Software Development Kit root. Defaults to ANDROID_HOME.", configurable = False),
        attr("android_ndk", "string", docs = "Android Native Development Kit root. Defaults to ANDROID_NDK_HOME.", configurable = False),
        attr("allow_network", "bool", default = "false", docs = "Allow Gradle to fetch missing locked dependencies.", configurable = False),
        attr("gradle_args", "list<string>", default = "[]", docs = "Additional Gradle arguments.", configurable = True),
    ],
    deps = [dep("deps", ["react_native_dependency_set", "react_native_codegen", "react_native_autolinking", "react_native_bundle"], "React Native packages, generated sources, module discovery, and optional release bundles.")],
    providers = ["react_native_android_application"],
    capabilities = [
        capability("build", ["default", "apk", "logs"]),
        capability("run", ["default"], ["apk"]),
    ],
    tools = [_REACT_NATIVE_NODE_TOOL, _REACT_NATIVE_JAVA_TOOL, _REACT_NATIVE_ADB_TOOL],
    examples = _REACT_NATIVE_EXAMPLE,
    impl = _react_native_android_application_impl,
)

react_native_metro = target_kind(
    docs = "Runs the Metro development server against live workspace sources and a locked dependency installation so Fast Refresh observes JavaScript edits without rebuilding native applications.",
    attrs = [
        attr("port", "int", default = "8081", docs = "Metro server port.", configurable = False),
        attr("host", "string", default = "\"127.0.0.1\"", docs = "Metro server host.", configurable = False),
        attr("metro_config", "string", docs = "Optional package-relative Metro configuration merged with Once dependency paths.", configurable = False),
        attr("reset_cache", "bool", default = "false", docs = "Clear Metro's transform cache before starting.", configurable = False),
        attr("node", "string", default = "\"node\"", docs = "Node.js executable.", configurable = False),
    ],
    deps = [dep("deps", ["react_native_dependency_set"], "Locked packages served by Metro.")],
    providers = ["react_native_metro"],
    capabilities = [capability("run", ["default"])],
    tools = [_REACT_NATIVE_NODE_TOOL],
    examples = _REACT_NATIVE_EXAMPLE,
    impl = _react_native_metro_impl,
)
