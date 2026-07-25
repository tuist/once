_REACT_NATIVE_NODE_TOOL = tool("node", executables = ["node"])
_REACT_NATIVE_NPM_TOOL = tool("npm", executables = ["npm"])
_REACT_NATIVE_XCODE_TOOL = tool("xcodebuild", executables = ["xcodebuild"])
_REACT_NATIVE_BUNDLE_TOOL = tool("bundle", executables = ["bundle"])
_REACT_NATIVE_JAVA_TOOL = tool("java", executables = ["java"])
_REACT_NATIVE_ADB_TOOL = tool("adb", executables = ["adb"])

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
    return packages.get("node_modules/" + name) or {}

def _react_native_hermes_package(lock):
    packages = lock.get("packages") or {}
    for path in [
        "node_modules/hermes-compiler",
        "node_modules/react-native/node_modules/hermes-compiler",
    ]:
        package = packages.get(path) or {}
        if package:
            return {"path": path, "package": package}
    return {}

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

def _react_native_npmrc_contains_credentials(value):
    lowered = value.lower()
    return "_authtoken" in lowered or "_password" in lowered or "_auth=" in lowered

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
    lockfile_path = attrs.get("lockfile") or "package-lock.json"
    npmrc_path = attrs.get("npmrc") or ""
    snapshot_path = attrs.get("modules_snapshot") or ""
    package_json = json_decode(_react_native_resolver_file(ctx, package_json_path, "JavaScript package manifest"))
    lock = json_decode(_react_native_resolver_file(ctx, lockfile_path, "JavaScript package lockfile"))
    react_native_locked = _react_native_locked_package(lock, "react-native")
    hermes = _react_native_hermes_package(lock)
    hermes_locked = hermes.get("package") or {}
    react_native_version = react_native_locked.get("version") or ""
    if not react_native_version:
        fail(ctx["label"]["id"] + ": package-lock.json has no locked `react-native` package")
    hermes_version = hermes_locked.get("version") or ""
    if not hermes_version:
        fail(ctx["label"]["id"] + ": package-lock.json has no Hermes compiler paired with the locked React Native package")
    requested = (package_json.get("dependencies") or {}).get("react-native") or ""
    if requested and _react_native_exact_version(requested) and requested != react_native_version:
        fail(ctx["label"]["id"] + ": package.json requests React Native `" + requested + "`, but package-lock.json selects `" + react_native_version + "`")
    if npmrc_path:
        npmrc = _react_native_resolver_file(ctx, npmrc_path, "npm configuration")
        if _react_native_npmrc_contains_credentials(npmrc):
            fail(ctx["label"]["id"] + ": npmrc must not contain authentication tokens or passwords because dependency inputs may be stored in a shared cache")

    modules = []
    if snapshot_path:
        snapshot = json_decode(_react_native_resolver_file(ctx, snapshot_path, "React Native module snapshot"))
        if snapshot.get("schema") != "once.react_native.modules.v1":
            fail(ctx["label"]["id"] + ": React Native module snapshot must use schema `once.react_native.modules.v1`")
        snapshot_version = snapshot.get("react_native_version") or ""
        if snapshot_version != react_native_version:
            fail(ctx["label"]["id"] + ": React Native module snapshot selects `" + snapshot_version + "`, but package-lock.json selects `" + react_native_version + "`")
        modules = snapshot.get("modules") or []

    targets = []
    roots = []
    for module in modules:
        module_name = module.get("name") or ""
        if not module_name:
            fail(ctx["label"]["id"] + ": React Native module snapshot contains a module without a name")
        locked = _react_native_locked_package(lock, module_name)
        if not locked:
            fail(ctx["label"]["id"] + ": React Native module `" + module_name + "` is absent from package-lock.json")
        version = locked.get("version") or ""
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
            "_react_native_version": react_native_version,
            "_hermes_version": hermes_version,
            "_hermes_package_path": hermes.get("path") or "",
            "_react_native_module_targets": roots,
        },
    }

def _react_native_tools(ctx, include_npm = False):
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
    if include_npm:
        npm_name = _react_native_attr(ctx, "npm", "npm")
        npm = _resolve_host_executable(npm_name)
        if not npm:
            fail(ctx["label"]["id"] + ": package installer `" + npm_name + "` was not found")
        npm_version = host_command([npm, "--version"]).strip()
        tools["npm"] = npm
        tools["npm_version"] = npm_version
        tools["identity"] = tools["identity"] + "\x00npm\x00" + npm + "\x00" + npm_version
    return tools

def _react_native_path_env(tools, extra = []):
    dirs = []
    for path in [tools.get("node") or "", tools.get("npm") or ""] + extra:
        parent = _parent_dir(path)
        if parent and parent not in dirs:
            dirs.append(parent)
    dirs.extend(["/usr/bin", "/bin"])
    return ":".join(dirs)

def _react_native_dependencies_impl(ctx):
    if not _react_native_attr(ctx, "_react_native_resolved", False):
        fail(ctx["label"]["id"] + ": react_native_dependencies must be expanded through its locked resolver")
    tools = _react_native_tools(ctx, True)
    package_json = _package_relative(ctx, _react_native_attr(ctx, "package_json", "package.json"))
    lockfile = _package_relative(ctx, _react_native_attr(ctx, "lockfile", "package-lock.json"))
    npmrc_attr = _react_native_attr(ctx, "npmrc", "")
    npmrc = _package_relative(ctx, npmrc_attr) if npmrc_attr else ""
    snapshot_attr = _react_native_attr(ctx, "modules_snapshot", "")
    snapshot = _package_relative(ctx, snapshot_attr) if snapshot_attr else ""
    install_root = ctx["build_dir"] + "/javascript"
    node_modules = install_root + "/node_modules"
    installed_package_json = install_root + "/package.json"
    installed_lockfile = install_root + "/package-lock.json"
    prepare_path(install_root, kind = "remove", identifier = ctx["label"]["id"] + ":javascript-clean")
    prepare_path(install_root, kind = "directory", identifier = ctx["label"]["id"] + ":javascript-prepare")
    copy_path(package_json, installed_package_json, inputs = [package_json], identifier = ctx["label"]["id"] + ":package-json")
    copy_path(lockfile, installed_lockfile, inputs = [lockfile], identifier = ctx["label"]["id"] + ":package-lock")
    install_inputs = [installed_package_json, installed_lockfile]
    if npmrc:
        installed_npmrc = install_root + "/.npmrc"
        copy_path(npmrc, installed_npmrc, inputs = [npmrc], identifier = ctx["label"]["id"] + ":npmrc")
        install_inputs.append(installed_npmrc)
    for source in glob(ctx["srcs"]):
        if source in [package_json, lockfile, npmrc, snapshot]:
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
    argv = [tools["npm"], "ci", "--no-audit", "--no-fund"]
    if not _react_native_attr(ctx, "allow_network", False):
        argv.append("--offline")
    argv.extend(_react_native_attr(ctx, "install_args", []))
    action_home = ctx["scratch_dir"] + "/home"
    action_cache = ctx["scratch_dir"] + "/npm-cache"
    run_action(
        argv = argv,
        inputs = install_inputs,
        outputs = [node_modules],
        clean_paths = [node_modules],
        create_dirs = [action_home, action_cache],
        cwd = install_root,
        env = {
            "CI": "1",
            "HOME": execution_path(action_home),
            "PATH": _react_native_path_env(tools),
            "npm_config_cache": execution_path(action_cache),
        },
        toolchain_identity = tools["identity"] + "\x00react-native\x00" + _react_native_attr(ctx, "_react_native_version", "") + "\x00hermes\x00" + _react_native_attr(ctx, "_hermes_version", ""),
        identifier = ctx["label"]["id"] + ":npm-ci",
    )
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "react_native_dependencies",
        "javascript_root": install_root,
        "node_modules": node_modules,
        "package_json": package_json,
        "lockfile": lockfile,
        "react_native_version": _react_native_attr(ctx, "_react_native_version", ""),
        "hermes_version": _react_native_attr(ctx, "_hermes_version", ""),
        "hermes_package_path": _react_native_attr(ctx, "_hermes_package_path", ""),
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
        host = host_os()
        arch = host_arch()
        if host == "macos":
            host_bin = "osx-bin/hermesc"
        elif host == "linux" and arch == "x86_64":
            host_bin = "linux64-bin/hermesc"
        elif host == "windows" and arch == "x86_64":
            host_bin = "win64-bin/hermesc.exe"
        else:
            fail(ctx["label"]["id"] + ": Hermes compilation is not supported on host `" + host + "/" + arch + "`")
        hermesc = stage + "/" + dependency["hermes_package_path"] + "/hermesc/" + host_bin
        compiler_map = final_bundle + ".compiler.map"
        run_action(
            argv = [
                execution_path(hermesc),
                "-w",
                "-emit-binary",
                "-max-diagnostic-width=80",
                "-out", execution_path(final_bundle),
                execution_path(packager_bundle),
                "-O",
                "-output-source-map",
            ],
            inputs = [hermesc, packager_bundle, dependency["node_modules"]],
            outputs = [final_bundle, final_bundle + ".map"],
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
    files = _unique(_react_native_source_files(ctx, [
        native_root + "/Pods/**/*",
        native_root + "/build/**/*",
    ]) + [dependency["package_json"], dependency["lockfile"]])
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
    files = _unique(_react_native_source_files(ctx, [
        native_root + "/.gradle/**/*",
        native_root + "/build/**/*",
        native_root + "/**/build/**/*",
    ]) + [dependency["package_json"], dependency["lockfile"]])
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
]

react_native_dependencies = target_kind(
    docs = "Installs the exact JavaScript dependency graph from package-lock.json and exposes locked React Native and Hermes identities.",
    attrs = [
        attr("package_json", "string", default = "\"package.json\"", docs = "Package-relative JavaScript package manifest.", configurable = False),
        attr("lockfile", "string", default = "\"package-lock.json\"", docs = "Package-relative package lockfile consumed by npm ci.", configurable = False),
        attr("npmrc", "string", docs = "Optional package-relative npm configuration file without authentication tokens or passwords.", configurable = False),
        attr("modules_snapshot", "string", docs = "Optional checked-in normalized react-native config snapshot.", configurable = False),
        attr("resolver_inputs", "list<string>", default = "[]", docs = "Manifest, lockfile, npm configuration, and module snapshot text supplied to the resolver.", configurable = False),
        attr("node", "string", default = "\"node\"", docs = "Node.js executable.", configurable = False),
        attr("npm", "string", default = "\"npm\"", docs = "npm executable.", configurable = False),
        attr("allow_network", "bool", default = "false", docs = "Allow npm to fetch packages missing from its local cache.", configurable = False),
        attr("install_args", "list<string>", default = "[]", docs = "Additional npm ci arguments.", configurable = False),
        attr("_react_native_resolved", "bool", default = "false", docs = "Resolver-owned locked-graph marker.", configurable = False),
        attr("_react_native_version", "string", default = "\"\"", docs = "Resolver-owned React Native version.", configurable = False),
        attr("_hermes_version", "string", default = "\"\"", docs = "Resolver-owned Hermes compiler and runtime version.", configurable = False),
        attr("_hermes_package_path", "string", default = "\"\"", docs = "Resolver-owned installed Hermes compiler package path.", configurable = False),
        attr("_react_native_module_targets", "list<string>", default = "[]", docs = "Resolver-owned synthetic module target names.", configurable = False),
    ],
    deps = [dep("deps", ["react_native_module"], "Locked native modules emitted by the resolver.")],
    providers = ["javascript_dependency_set", "react_native_dependency_set"],
    capabilities = [capability("build", ["node_modules"])],
    tools = [_REACT_NATIVE_NODE_TOOL, _REACT_NATIVE_NPM_TOOL],
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
        attr("platform", "string", required = True, docs = "`ios` or `android`.", configurable = False),
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
        attr("platform", "string", required = True, docs = "`ios`, `android`, or `all`.", configurable = False),
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
        attr("sdk", "string", default = "\"iphonesimulator\"", docs = "Apple software development kit identifier.", configurable = False),
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
