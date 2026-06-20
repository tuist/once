def _android_shell_words(values):
    return " ".join([_shell_quote(value) for value in values])

def _android_shell_env(root):
    return "ANDROID_HOME=" + _shell_quote(root) + " ANDROID_SDK_ROOT=" + _shell_quote(root)

def _android_host_shell(label_id):
    if host_os() == "windows":
        fail(label_id + ": Android build actions currently require a POSIX-compatible host shell")
    return host_which("sh")

def _android_is_select_shape(value):
    if type(value) != type({}):
        return False
    if len(value) != 1:
        return False
    inner = value.get("select")
    return type(inner) == type({})

def _android_config_tokens(attrs, label_id):
    for key in ["compile_sdk", "min_sdk_version", "build_type"]:
        if _android_is_select_shape(attrs.get(key)):
            fail(label_id + ": attribute `" + key + "` cannot use select() because the Android configuration depends on it")
    tokens = ["android"]
    compile_sdk = attrs.get("compile_sdk")
    if compile_sdk:
        tokens.append("compile_sdk_" + str(compile_sdk))
        tokens.append("api_" + str(compile_sdk))
    min_sdk = attrs.get("min_sdk_version")
    if min_sdk:
        tokens.append("min_sdk_" + str(min_sdk))
    tokens.append(attrs.get("build_type") or "debug")
    tokens.append("default")
    return _unique(tokens)

def _android_select_branch(branches, tokens, label_id, attr_name):
    for token in tokens:
        if token in branches:
            return token
    fail(label_id + ": select() on `" + attr_name + "` has no branch matching the Android configuration and no `default`")

def _android_resolve_select(value, tokens, label_id, attr_name):
    if _android_is_select_shape(value):
        branches = value["select"]
        key = _android_select_branch(branches, tokens, label_id, attr_name)
        return _android_resolve_select(branches[key], tokens, label_id, attr_name)
    if type(value) == type([]):
        return [_android_resolve_select(item, tokens, label_id, attr_name) for item in value]
    if type(value) == type({}):
        return {k: _android_resolve_select(v, tokens, label_id, attr_name) for k, v in value.items()}
    return value

def _android_resolve_attrs(ctx, non_configurable):
    attrs = ctx["attr"]
    label_id = ctx["label"]["id"]
    tokens = _android_config_tokens(attrs, label_id)
    out = {}
    for key, value in attrs.items():
        if key in non_configurable and _android_is_select_shape(value):
            fail(label_id + ": attribute `" + key + "` is not configurable but uses select()")
        out[key] = _android_resolve_select(value, tokens, label_id, key)
    return out

def _android_attr(attrs, key, default):
    value = attrs.get(key)
    if value == None:
        return default
    return value

def _android_reject_unsupported_attrs(attrs, label_id, keys):
    for key in keys:
        value = attrs.get(key)
        if value == None:
            continue
        if type(value) == type("") and value == "":
            continue
        if type(value) == type([]) and len(value) == 0:
            continue
        if type(value) == type({}) and len(value) == 0:
            continue
        fail(label_id + ": attribute `" + key + "` is declared but not implemented by this target kind yet")

def _android_source_inputs(ctx, attrs, include_implicit_resources):
    srcs = glob(ctx["srcs"])
    resources = _android_resource_files(attrs, include_implicit_resources)
    assets = glob(_android_attr(attrs, "assets", []))
    return _unique(srcs + resources + assets)

def _android_java_sources(ctx):
    return _filter_by_extensions(glob(ctx["srcs"]), [".java"])

def _android_kotlin_sources(ctx):
    return _filter_by_extensions(glob(ctx["srcs"]), [".kt"])

def _android_resource_files(attrs, include_implicit_resources):
    patterns = attrs.get("resource_files")
    if patterns != None:
        return _android_file_globs(patterns)
    if include_implicit_resources:
        return _android_file_globs(["res/**"])
    return []

def _android_asset_files(attrs):
    return _android_file_globs(_android_attr(attrs, "assets", []))

def _android_workspace_inputs(paths):
    return [path for path in paths if not path.startswith("/")]

def _android_abs_path(path):
    if path.startswith("/"):
        return path
    root = workspace_root()
    if root:
        return root + "/" + path
    return path

def _android_basename(path):
    idx = -1
    for i in range(len(path)):
        if path[i] == "/":
            idx = i
    if idx < 0:
        return path
    return path[idx + 1:]

def _android_existing_file(label_id, path):
    if not path:
        return ""
    sh = _android_host_shell(label_id)
    script = "set -eu\nif [ -f " + _shell_quote(path) + " ]; then printf '%s' " + _shell_quote(path) + "; fi\n"
    return host_command([sh, "-c", script]).strip()

def _android_file_sha256(label_id, path):
    sh = _android_host_shell(label_id)
    script = """set -eu
file={path}
if [ ! -f "$file" ]; then
  echo "missing file: $file" >&2
  exit 1
fi
if command -v shasum >/dev/null 2>&1; then
  shasum -a 256 "$file" | awk '{{print $1}}'
else
  sha256sum "$file" | awk '{{print $1}}'
fi
""".format(
        path = _shell_quote(path),
    )
    return host_command([sh, "-c", script]).strip()

def _android_default_debug_keystore(label_id):
    candidates = []
    configured = host_env("ANDROID_DEBUG_KEYSTORE")
    if configured:
        candidates.append(configured)
    home = host_env("HOME")
    if home:
        candidates.append(home + "/.android/debug.keystore")
    for candidate in candidates:
        found = _android_existing_file(label_id, candidate)
        if found:
            return found
    fail(label_id + ": set `debug_keystore`, set ANDROID_DEBUG_KEYSTORE, create ~/.android/debug.keystore, or set `signing = \"none\"`")

def _android_apk_filename(ctx):
    return ctx["label"]["name"] + ".apk"

def _android_apk_output(ctx):
    return declare_output(_android_apk_filename(ctx))

def _android_built_apk(ctx):
    return ctx["build_dir"] + "/" + _android_apk_filename(ctx)

def _android_file_globs(patterns):
    expanded = []
    for pattern in patterns:
        expanded.append(pattern)
        if _ends_with(pattern, "/**"):
            expanded.append(pattern + "/*")
    return glob(expanded)

def _android_hash_tree_script(root, output, find_args):
    return """rm -f {output}
: > {output}
if [ -d {root} ]; then
  find {root} -type f {find_args} | sort | while IFS= read -r file; do
    if command -v shasum >/dev/null 2>&1; then
      shasum -a 256 "$file"
    else
      sha256sum "$file"
    fi
  done > {output}
fi
""".format(
        root = _shell_quote(root),
        output = _shell_quote(output),
        find_args = find_args,
    )

def _android_resource_dirs(ctx, attrs, resource_files):
    declared = _android_attr(attrs, "resource_dirs", ["res"])
    out = []
    for raw_dir in declared:
        resource_dir = _package_relative(ctx, raw_dir)
        prefix = resource_dir + "/"
        for resource in resource_files:
            if resource.startswith(prefix):
                out.append(resource_dir)
                break
    return _unique(out)

def _android_asset_roots(ctx, attrs, asset_files):
    roots = _android_attr(attrs, "assets_dir", "")
    if roots:
        roots = [roots]
    else:
        roots = _android_attr(attrs, "asset_dirs", ["assets"])
    out = []
    for raw_dir in roots:
        asset_dir = _package_relative(ctx, raw_dir)
        prefix = asset_dir + "/"
        for asset in asset_files:
            if asset.startswith(prefix):
                out.append(asset_dir)
                break
    return _unique(out)

def _android_manifest(ctx, attrs):
    return _package_relative(ctx, _android_attr(attrs, "manifest", "AndroidManifest.xml"))

def _android_namespace(attrs):
    return _android_attr(attrs, "custom_package", "") or _android_attr(attrs, "namespace", "") or _android_attr(attrs, "package", "")

def _android_sdk_root(attrs, label_id):
    configured = _android_attr(attrs, "android_sdk", "")
    if configured:
        return configured
    root = host_env("ANDROID_HOME") or host_env("ANDROID_SDK_ROOT")
    if not root:
        fail(label_id + ": set `android_sdk` or ANDROID_HOME/ANDROID_SDK_ROOT so Once can find Android SDK tools")
    return root

def _android_highest_platform(sdk_root, label_id):
    sh = _android_host_shell(label_id)
    script = "set -eu\nfor p in " + _shell_quote(sdk_root + "/platforms") + "/android-*; do\n  [ -d \"$p\" ] || continue\n  v=${p##*/android-}\n  case \"$v\" in *[!0-9]*|'') continue ;; esac\n  printf '%s\\n' \"$v\"\ndone | sort -n | tail -n 1\n"
    value = host_command([sh, "-c", script]).strip()
    if not value:
        fail(label_id + ": no Android SDK platform was found under `" + sdk_root + "`; install one or set `compile_sdk`")
    return value

def _android_highest_build_tools(sdk_root, label_id):
    sh = _android_host_shell(label_id)
    script = "set -eu\nfor p in " + _shell_quote(sdk_root + "/build-tools") + "/*; do\n  [ -d \"$p\" ] || continue\n  printf '%s\\n' \"${p##*/}\"\ndone | sort | tail -n 1\n"
    value = host_command([sh, "-c", script]).strip()
    if not value:
        fail(label_id + ": no Android SDK build-tools version was found under `" + sdk_root + "`; install build-tools or set `build_tools_version`")
    return value

def _android_tool_first_line(label_id, argv):
    sh = _android_host_shell(label_id)
    script = "set -eu\nout=$(" + _android_shell_words(argv) + " 2>&1)\nif [ -z \"$out\" ]; then echo 'tool version command produced no output' >&2; exit 1; fi\nprintf '%s\\n' \"$out\" | sed -n '1p'\n"
    return host_command([sh, "-c", script]).strip()

def _android_tool_version(tool, label_id):
    return _android_tool_first_line(label_id, [tool, "--version"])

def _android_adb_version(adb):
    return host_command([adb, "version"]).strip()

def _android_javac_version(javac, label_id):
    return _android_tool_first_line(label_id, [javac, "-version"])

def _android_host_env(name):
    return host_env(name)

def _android_kotlin_home(kotlinc):
    bin_dir = _parent_dir(kotlinc)
    if _ends_with(bin_dir, "/bin"):
        return _parent_dir(bin_dir)
    return ""

def _android_kotlin_stdlib(attrs, kotlinc):
    configured = _android_attr(attrs, "kotlin_stdlib", "")
    if configured:
        return configured
    kotlin_home = _android_attr(attrs, "kotlin_home", "") or _android_kotlin_home(kotlinc)
    if not kotlin_home:
        return ""
    return kotlin_home + "/lib/kotlin-stdlib.jar"

def _android_tools(ctx, attrs, include_java, include_apk, include_kotlin = False):
    label_id = ctx["label"]["id"]
    sdk_root = _android_sdk_root(attrs, label_id)
    compile_sdk = str(_android_attr(attrs, "compile_sdk", "") or _android_highest_platform(sdk_root, label_id))
    build_tools_version = _android_attr(attrs, "build_tools_version", "") or _android_highest_build_tools(sdk_root, label_id)
    build_tools = sdk_root + "/build-tools/" + build_tools_version
    tools = {
        "sdk_root": sdk_root,
        "compile_sdk": compile_sdk,
        "build_tools_version": build_tools_version,
        "android_jar": sdk_root + "/platforms/android-" + compile_sdk + "/android.jar",
        "aapt2": _android_attr(attrs, "aapt2", "") or build_tools + "/aapt2",
    }
    identity = [
        "once.android.tools.v1",
        "sdk",
        sdk_root,
        "compile_sdk",
        compile_sdk,
        "build_tools",
        build_tools_version,
        "aapt2",
        tools["aapt2"],
        _android_tool_first_line(label_id, [tools["aapt2"], "version"]),
    ]
    if include_java:
        javac = _android_attr(attrs, "javac", "") or host_which("javac")
        jar = _android_attr(attrs, "jar", "") or host_which("jar")
        java = _android_attr(attrs, "java", "") or host_which("java")
        java_home = _android_attr(attrs, "java_home", "") or _android_host_env("JAVA_HOME")
        tools["javac"] = javac
        tools["jar"] = jar
        tools["java"] = java
        if java_home:
            tools["java_home"] = java_home
        identity.extend(["javac", javac, _android_javac_version(javac, label_id), "jar", jar, "java", java, "java_home", java_home])
    if include_kotlin:
        kotlinc = _android_attr(attrs, "kotlinc", "") or host_which("kotlinc")
        kotlin_stdlib = _android_kotlin_stdlib(attrs, kotlinc)
        if not kotlin_stdlib:
            fail(label_id + ": set `kotlin_stdlib` or `kotlin_home` so Once can find kotlin-stdlib.jar")
        tools["kotlinc"] = kotlinc
        tools["kotlin_stdlib"] = kotlin_stdlib
        identity.extend(["kotlinc", kotlinc, _android_tool_first_line(label_id, [kotlinc, "-version"]), "kotlin_stdlib", kotlin_stdlib])
    if include_apk:
        tools["d8"] = _android_attr(attrs, "d8", "") or build_tools + "/d8"
        tools["apksigner"] = _android_attr(attrs, "apksigner", "") or build_tools + "/apksigner"
        tools["zipalign"] = _android_attr(attrs, "zipalign", "") or build_tools + "/zipalign"
        identity.extend(["d8", tools["d8"], _android_tool_version(tools["d8"], label_id), "apksigner", tools["apksigner"], "zipalign", tools["zipalign"]])
    tools["identity"] = "\x00".join(identity)
    return tools

def _android_adb_tools(ctx, attrs):
    label_id = ctx["label"]["id"]
    sdk_root = _android_sdk_root(attrs, label_id)
    adb = _android_attr(attrs, "adb", "") or sdk_root + "/platform-tools/adb"
    return {
        "sdk_root": sdk_root,
        "adb": adb,
        "identity": "\x00".join(["once.android.adb.v1", "sdk", sdk_root, "adb", adb, _android_adb_version(adb)]),
    }

def _android_env(tools):
    env = {"ANDROID_HOME": tools["sdk_root"], "ANDROID_SDK_ROOT": tools["sdk_root"]}
    java = tools.get("java") or ""
    java_home = tools.get("java_home") or ""
    if java_home:
        env["JAVA_HOME"] = java_home
    if java:
        env["PATH"] = _parent_dir(java) + ":/usr/bin:/bin:/usr/sbin:/sbin"
    return env

def _android_classpath_sep():
    if host_os() == "windows":
        return ";"
    return ":"

def _android_contains(value, needle):
    for i in range(len(value)):
        if value[i:i + len(needle)] == needle:
            return True
    return False

def _android_adb_argv(tools, attrs):
    argv = [tools["adb"]]
    serial = _android_attr(attrs, "adb_serial", "")
    if serial:
        argv.extend(["-s", serial])
    return argv

def _android_launch_component(application_id, launch_activity):
    if not launch_activity:
        return ""
    if _android_contains(launch_activity, "/"):
        return launch_activity
    return application_id + "/" + launch_activity

def _android_launcher_script(application_id):
    return """set -eu
component=$(cmd package resolve-activity --brief -a android.intent.action.MAIN -c android.intent.category.LAUNCHER {application_id} | tail -n 1)
case "$component" in
  */*) exec am start -n "$component" ;;
  *) echo {error_message} >&2; exit 1 ;;
esac
""".format(
        application_id = _shell_quote(application_id),
        error_message = _shell_quote("unable to resolve launcher activity for " + application_id),
    )

def _android_compile_jars(deps):
    jars = []
    for dep in deps:
        jars.extend(dep.get("transitive_compile_jars") or [])
    return _unique(jars)

def _android_runtime_jars(deps):
    jars = []
    for dep in deps:
        jars.extend(dep.get("transitive_runtime_jars") or [])
    return _unique(jars)

def _android_resource_apks(deps):
    apks = []
    for dep in deps:
        apks.extend(dep.get("transitive_resource_apks") or [])
    return _unique(apks)

def _android_compiled_resource_zips(deps):
    zips = []
    for dep in deps:
        zips.extend(dep.get("transitive_compiled_resource_zips") or [])
    return _unique(zips)

def _android_asset_roots_from_deps(deps):
    roots = []
    for dep in deps:
        roots.extend(dep.get("transitive_asset_roots") or [])
    return _unique(roots)

def _android_asset_files_from_deps(deps):
    files = []
    for dep in deps:
        files.extend(dep.get("transitive_asset_files") or [])
    return _unique(files)

def _android_native_library_key(library):
    return (library.get("abi") or "") + "\x00" + (library.get("path") or "")

def _android_unique_native_libraries(libraries):
    seen = {}
    out = []
    for library in libraries:
        abi = library.get("abi") or ""
        path = library.get("path") or ""
        if not abi or not path:
            continue
        key = _android_native_library_key(library)
        if key not in seen:
            seen[key] = True
            out.append({"abi": abi, "path": path})
    return out

def _android_native_libraries(deps):
    libraries = []
    for dep in deps:
        libraries.extend(dep.get("transitive_android_native_libraries") or [])
        libraries.extend(dep.get("android_native_libraries") or [])
    return _android_unique_native_libraries(libraries)

def _android_native_library_paths(libraries):
    out = []
    for library in libraries:
        path = library.get("path") or ""
        if path and path not in out:
            out.append(path)
    return out

def _android_compile_resources(ctx, attrs, tools, resource_files, resource_dirs):
    if len(resource_files) == 0:
        return []
    compiled_zips = []
    compile_lines = []
    sh = _android_host_shell(ctx["label"]["id"])
    i = 0
    for resource_dir in resource_dirs:
        i = i + 1
        compiled_zip = declare_output("compiled_resources/resources_" + str(i) + ".zip")
        compiled_zips.append(compiled_zip)
        compile_lines.append("mkdir -p " + _shell_quote(_parent_dir(compiled_zip)) + "\n" + _shell_quote(tools["aapt2"]) + " compile --dir " + _shell_quote(resource_dir) + " -o " + _shell_quote(compiled_zip))
    script = """set -eu
{compile_lines}
""".format(
        compile_lines = "\n".join(compile_lines),
    )
    run_action(
        argv = [sh, "-c", script],
        inputs = resource_files,
        outputs = compiled_zips,
        env = _android_env(tools),
        toolchain_identity = tools["identity"] + "\x00aapt2_compile",
        identifier = "android_resource_compile:" + ctx["label"]["id"],
    )
    return compiled_zips

def _android_link_resources(ctx, attrs, tools, manifest, compiled_zips, dep_compiled_zips, static_lib, asset_roots, asset_files):
    label_id = ctx["label"]["id"]
    sh = _android_host_shell(label_id)
    resource_apk = declare_output("resources.apk")
    r_src_dir = declare_output("generated/r")
    r_src_hash = declare_output("generated/r_sources.sha256")
    r_txt = declare_output("R.txt")
    min_sdk = str(_android_attr(attrs, "min_sdk_version", 23))
    target_sdk = str(_android_attr(attrs, "target_sdk_version", tools["compile_sdk"]))
    namespace = _android_namespace(attrs) or _android_attr(attrs, "application_id", "")
    base_args = [
        tools["aapt2"],
        "link",
        "--manifest", manifest,
        "-I", tools["android_jar"],
        "--java", r_src_dir,
        "--output-text-symbols", r_txt,
        "--min-sdk-version", min_sdk,
        "--target-sdk-version", target_sdk,
        "--auto-add-overlay",
        "-o", resource_apk,
    ]
    if static_lib:
        base_args.append("--static-lib")
    if namespace:
        base_args.extend(["--custom-package", namespace])
    if not static_lib:
        version_code = _android_attr(attrs, "version_code", 1)
        version_name = _android_attr(attrs, "version_name", "1.0")
        base_args.extend(["--version-code", str(version_code), "--version-name", version_name])
    script = """set -eu
rm -rf {r_src_dir}
mkdir -p {r_src_dir}
set -- {base_args}
for zip in {compiled_zips}; do
  [ -f "$zip" ] || continue
  set -- "$@" -R "$zip"
done
for dir in {asset_roots}; do
  [ -d "$dir" ] || continue
  set -- "$@" -A "$dir"
done
"$@"
{hash_r_sources}
""".format(
        r_src_dir = _shell_quote(r_src_dir),
        base_args = _android_shell_words(base_args),
        compiled_zips = _android_shell_words(_unique(compiled_zips + dep_compiled_zips)),
        asset_roots = _android_shell_words(asset_roots),
        hash_r_sources = _android_hash_tree_script(r_src_dir, r_src_hash, "-name '*.java'"),
    )
    run_action(
        argv = [sh, "-c", script],
        inputs = _unique([manifest] + compiled_zips + dep_compiled_zips + asset_files),
        outputs = [resource_apk, r_src_dir, r_src_hash, r_txt],
        env = _android_env(tools),
        toolchain_identity = tools["identity"] + "\x00aapt2_link\x00static\x00" + str(static_lib),
        identifier = "android_resource_link:" + label_id,
    )
    return (resource_apk, r_src_dir, r_src_hash, r_txt)

def _android_compile_java(ctx, attrs, tools, java_sources, r_src_dir, r_src_hash, dep_jars):
    sh = _android_host_shell(ctx["label"]["id"])
    classes_dir = declare_output("java_classes")
    classes_hash = declare_output("classes.sha256")
    source_list = ctx["build_dir"] + "/java_sources.list"
    classpath_entries = _unique([tools["android_jar"]] + dep_jars)
    classpath = _android_classpath_sep().join(classpath_entries)
    source_level = str(_android_attr(attrs, "java_language_level", "17"))
    javac_opts = _android_attr(attrs, "javac_opts", [])
    base_args = [
        tools["javac"],
        "-encoding", "UTF-8",
        "-source", source_level,
        "-target", source_level,
        "-classpath", classpath,
        "-d", classes_dir,
    ] + javac_opts
    script = """set -eu
rm -rf {classes_dir}
mkdir -p {classes_dir}
: > {source_list}
for src in {java_sources}; do
  [ -f "$src" ] || continue
  printf '%s\\n' "$src" >> {source_list}
done
if [ -d {r_src_dir} ]; then
  find {r_src_dir} -name '*.java' | sort >> {source_list}
fi
if [ -s {source_list} ]; then
  set -- {base_args} @{source_list}
  "$@"
fi
{hash_classes}
""".format(
        classes_dir = _shell_quote(classes_dir),
        source_list = _shell_quote(source_list),
        java_sources = _android_shell_words(java_sources),
        r_src_dir = _shell_quote(r_src_dir),
        base_args = _android_shell_words(base_args),
        hash_classes = _android_hash_tree_script(classes_dir, classes_hash, ""),
    )
    run_action(
        argv = [sh, "-c", script],
        inputs = _unique(java_sources + [r_src_hash] + _android_workspace_inputs(dep_jars)),
        outputs = [classes_dir, classes_hash],
        env = _android_env(tools),
        toolchain_identity = tools["identity"] + "\x00javac\x00level\x00" + source_level,
        identifier = "android_java_compile:" + ctx["label"]["id"],
    )
    return (classes_dir, classes_hash)

def _android_compile_kotlin(ctx, attrs, tools, kotlin_sources, classes_dir, classes_hash, dep_jars):
    if len(kotlin_sources) == 0:
        return (classes_dir, classes_hash)
    sh = _android_host_shell(ctx["label"]["id"])
    merged_classes_dir = declare_output("classes")
    kotlin_hash = declare_output("classes.kotlin.sha256")
    source_list = ctx["build_dir"] + "/kotlin_sources.list"
    classpath_entries = _unique([tools["android_jar"], tools["kotlin_stdlib"], classes_dir] + dep_jars)
    classpath = _android_classpath_sep().join(classpath_entries)
    kotlinc_opts = _android_attr(attrs, "kotlinc_opts", [])
    base_args = [
        tools["kotlinc"],
        "-classpath", classpath,
        "-d", merged_classes_dir,
    ] + kotlinc_opts
    script = """set -eu
rm -rf {merged_classes_dir}
mkdir -p {merged_classes_dir}
if [ -d {classes_dir} ]; then
  cp -R {classes_dir}/. {merged_classes_dir}/
fi
: > {source_list}
for src in {kotlin_sources}; do
  [ -f "$src" ] || continue
  printf '%s\\n' "$src" >> {source_list}
done
if [ -s {source_list} ]; then
  set -- {base_args} @{source_list}
  "$@"
fi
{hash_classes}
""".format(
        classes_dir = _shell_quote(classes_dir),
        merged_classes_dir = _shell_quote(merged_classes_dir),
        source_list = _shell_quote(source_list),
        kotlin_sources = _android_shell_words(kotlin_sources),
        base_args = _android_shell_words(base_args),
        hash_classes = _android_hash_tree_script(merged_classes_dir, kotlin_hash, ""),
    )
    run_action(
        argv = [sh, "-c", script],
        inputs = _unique(kotlin_sources + [classes_hash] + _android_workspace_inputs(dep_jars)),
        outputs = [merged_classes_dir, kotlin_hash],
        env = _android_env(tools),
        toolchain_identity = tools["identity"] + "\x00kotlinc",
        identifier = "android_kotlin_compile:" + ctx["label"]["id"],
    )
    return (merged_classes_dir, kotlin_hash)

def _android_jar_classes(ctx, tools, classes_dir, classes_hash):
    sh = _android_host_shell(ctx["label"]["id"])
    classes_jar = declare_output(ctx["label"]["name"] + ".jar")
    script = """set -eu
rm -f {classes_jar}
{jar} cf {classes_jar} -C {classes_dir} .
""".format(
        classes_jar = _shell_quote(classes_jar),
        jar = _shell_quote(tools["jar"]),
        classes_dir = _shell_quote(classes_dir),
    )
    run_action(
        argv = [sh, "-c", script],
        inputs = [classes_hash],
        outputs = [classes_jar],
        env = _android_env(tools),
        toolchain_identity = tools["identity"] + "\x00jar_classes",
        identifier = "android_classes_jar:" + ctx["label"]["id"],
    )
    return classes_jar

def _android_package_aar(ctx, attrs, tools, manifest, classes_jar, r_txt, resource_dirs, asset_roots, resource_files, asset_files):
    sh = _android_host_shell(ctx["label"]["id"])
    aar = declare_output(ctx["label"]["name"] + ".aar")
    staging = ctx["build_dir"] + "/aar_staging"
    script = """set -eu
rm -rf {staging}
mkdir -p {staging}
cp {manifest} {staging}/AndroidManifest.xml
cp {classes_jar} {staging}/classes.jar
if [ -f {r_txt} ]; then cp {r_txt} {staging}/R.txt; else : > {staging}/R.txt; fi
if [ -n {resource_dirs_joined} ]; then
  mkdir -p {staging}/res
  for dir in {resource_dirs}; do
    [ -d "$dir" ] || continue
    cp -R "$dir"/. {staging}/res/
  done
fi
if [ -n {asset_roots_joined} ]; then
  mkdir -p {staging}/assets
  for dir in {asset_roots}; do
    [ -d "$dir" ] || continue
    cp -R "$dir"/. {staging}/assets/
  done
fi
rm -f {aar}
{jar} cf {aar} -C {staging} .
""".format(
        staging = _shell_quote(staging),
        manifest = _shell_quote(manifest),
        classes_jar = _shell_quote(classes_jar),
        r_txt = _shell_quote(r_txt),
        resource_dirs_joined = _shell_quote(" ".join(resource_dirs)),
        resource_dirs = _android_shell_words(resource_dirs),
        asset_roots_joined = _shell_quote(" ".join(asset_roots)),
        asset_roots = _android_shell_words(asset_roots),
        aar = _shell_quote(aar),
        jar = _shell_quote(tools["jar"]),
    )
    run_action(
        argv = [sh, "-c", script],
        inputs = _unique([manifest, classes_jar, r_txt] + resource_files + asset_files),
        outputs = [aar],
        env = _android_env(tools),
        toolchain_identity = tools["identity"] + "\x00aar",
        identifier = "android_aar:" + ctx["label"]["id"],
    )
    return aar

def _android_dex(ctx, attrs, tools, runtime_jars):
    sh = _android_host_shell(ctx["label"]["id"])
    dex_dir = declare_output("dex")
    dex_hash = declare_output("dex.sha256")
    min_sdk = str(_android_attr(attrs, "min_sdk_version", 23))
    dexopts = _android_attr(attrs, "dexopts", [])
    base_args = [
        tools["d8"],
        "--min-api", min_sdk,
        "--lib", tools["android_jar"],
        "--output", dex_dir,
    ] + dexopts + runtime_jars
    script = """set -eu
rm -rf {dex_dir}
mkdir -p {dex_dir}
set -- {base_args}
"$@"
{hash_dex}
""".format(
        dex_dir = _shell_quote(dex_dir),
        base_args = _android_shell_words(base_args),
        hash_dex = _android_hash_tree_script(dex_dir, dex_hash, "-name '*.dex'"),
    )
    run_action(
        argv = [sh, "-c", script],
        inputs = _android_workspace_inputs(runtime_jars),
        outputs = [dex_dir, dex_hash],
        env = _android_env(tools),
        toolchain_identity = tools["identity"] + "\x00d8\x00min\x00" + min_sdk,
        identifier = "android_dex:" + ctx["label"]["id"],
    )
    return (dex_dir, dex_hash)

def _android_package_unsigned_apk(ctx, tools, resource_apk, dex_dir, dex_hash, native_libraries):
    sh = _android_host_shell(ctx["label"]["id"])
    unsigned_apk = declare_output("unsigned.apk")
    native_staging = ctx["build_dir"] + "/native_staging"
    native_copy_lines = []
    for library in native_libraries:
        abi = library.get("abi") or ""
        path = library.get("path") or ""
        if not abi or not path:
            continue
        destination_dir = native_staging + "/lib/" + abi
        native_copy_lines.append("mkdir -p " + _shell_quote(destination_dir))
        native_copy_lines.append("cp " + _shell_quote(path) + " " + _shell_quote(destination_dir + "/" + _android_basename(path)))
    script = """set -eu
rm -f {unsigned_apk}
cp {resource_apk} {unsigned_apk}
if [ -d {dex_dir} ]; then
  {jar} uf {unsigned_apk} -C {dex_dir} .
fi
rm -rf {native_staging}
{native_copy_lines}
if [ -d {native_staging}/lib ]; then
  {jar} uf {unsigned_apk} -C {native_staging} lib
fi
""".format(
        unsigned_apk = _shell_quote(unsigned_apk),
        resource_apk = _shell_quote(resource_apk),
        dex_dir = _shell_quote(dex_dir),
        jar = _shell_quote(tools["jar"]),
        native_staging = _shell_quote(native_staging),
        native_copy_lines = "\n".join(native_copy_lines),
    )
    run_action(
        argv = [sh, "-c", script],
        inputs = _unique([resource_apk, dex_hash] + _android_native_library_paths(native_libraries)),
        outputs = [unsigned_apk],
        env = _android_env(tools),
        toolchain_identity = tools["identity"] + "\x00unsigned_apk\x00native_libraries\x00" + str(native_libraries),
        identifier = "android_unsigned_apk:" + ctx["label"]["id"],
    )
    return unsigned_apk

def _android_zipalign(ctx, tools, unsigned_apk):
    aligned_apk = declare_output("aligned.apk")
    run_action(
        argv = [tools["zipalign"], "-f", "4", unsigned_apk, aligned_apk],
        inputs = [unsigned_apk],
        outputs = [aligned_apk],
        env = _android_env(tools),
        toolchain_identity = tools["identity"] + "\x00zipalign",
        identifier = "android_zipalign:" + ctx["label"]["id"],
    )
    return aligned_apk

def _android_sign_or_copy(ctx, attrs, tools, aligned_apk):
    sh = _android_host_shell(ctx["label"]["id"])
    apk = _android_apk_output(ctx)
    signing = _android_attr(attrs, "signing", "debug")
    if signing == "none":
        run_action(
            argv = [sh, "-c", "set -eu\ncp " + _shell_quote(aligned_apk) + " " + _shell_quote(apk) + "\n"],
            outputs = [apk],
            env = _android_env(tools),
            toolchain_identity = tools["identity"] + "\x00unsigned_final",
            identifier = "android_final_apk:" + ctx["label"]["id"],
        )
        return (apk, "")
    if signing != "debug":
        fail(ctx["label"]["id"] + ": android_binary signing supports `debug` and `none`; production signing is not implemented")
    debug_keystore = _android_attr(attrs, "debug_keystore", "")
    keystore = declare_output("debug.keystore")
    password = _android_attr(attrs, "debug_keystore_password", "android")
    alias = _android_attr(attrs, "debug_key_alias", "androiddebugkey")
    if password != "android":
        fail(ctx["label"]["id"] + ": `debug_keystore_password` must stay `android`; custom signing passwords are not supported")
    inputs = []
    if debug_keystore:
        source_keystore = _package_relative(ctx, debug_keystore)
        inputs.append(source_keystore)
    else:
        source_keystore = _android_default_debug_keystore(ctx["label"]["id"])
    keystore_digest = _android_file_sha256(ctx["label"]["id"], _android_abs_path(source_keystore))
    script = """set -eu
rm -f {apk}
rm -f {keystore}
cp {source_keystore} {keystore}
{apksigner} sign --ks {keystore} --ks-pass {password_arg} --key-pass {password_arg} --ks-key-alias {alias_arg} --out {apk} {aligned_apk}
""".format(
        apk = _shell_quote(apk),
        keystore = _shell_quote(keystore),
        source_keystore = _shell_quote(source_keystore),
        apksigner = _shell_quote(tools["apksigner"]),
        password_arg = _shell_quote("pass:" + password),
        alias_arg = _shell_quote(alias),
        aligned_apk = _shell_quote(aligned_apk),
    )
    run_action(
        argv = [sh, "-c", script],
        inputs = _unique(inputs + [aligned_apk]),
        outputs = [apk, keystore],
        env = _android_env(tools),
        toolchain_identity = tools["identity"] + "\x00debug_sign\x00keystore_sha256\x00" + keystore_digest,
        identifier = "android_sign:" + ctx["label"]["id"],
    )
    return (apk, keystore)

def _android_run_app(ctx, attrs, tools):
    label_id = ctx["label"]["id"]
    application_id = _android_attr(attrs, "application_id", "")
    launch_activity = _android_attr(attrs, "launch_activity", "")
    component = _android_launch_component(application_id, launch_activity)
    apk = _android_built_apk(ctx)
    adb = _android_adb_argv(tools, attrs)
    run_actions = [
        (adb + ["wait-for-device"], []),
        (adb + ["install", "-r", "-d", apk], [apk]),
    ]
    if component:
        run_actions.append((adb + ["shell", "am", "start", "-n", component], []))
    else:
        run_actions.append((adb + ["shell", _android_launcher_script(application_id)], []))
    index = 0
    for action in run_actions:
        index = index + 1
        run_action(
            argv = action[0],
            inputs = action[1],
            outputs = [],
            env = _android_env(tools),
            cacheable = False,
            toolchain_identity = tools["identity"] + "\x00run\x00" + str(index),
            identifier = "android_run:" + label_id + ":" + str(index),
        )
    return {
        "label_id": label_id,
        "target_kind": "android_binary",
        "application_id": application_id,
        "apk": apk,
        "adb": tools["adb"],
        "launch_activity": launch_activity,
    }

def _android_resource_impl(ctx):
    attrs = _android_resolve_attrs(ctx, ["manifest", "resource_dirs", "asset_dirs", "assets_dir", "android_sdk", "build_tools_version", "compile_sdk", "custom_package", "namespace", "package", "aapt2"])
    tools = _android_tools(ctx, attrs, False, False)
    manifest = _android_manifest(ctx, attrs)
    resource_files = _android_resource_files(attrs, True)
    asset_files = _android_asset_files(attrs)
    resource_dirs = _android_resource_dirs(ctx, attrs, resource_files)
    asset_roots = _android_asset_roots(ctx, attrs, asset_files)
    if len(resource_files) > 0 and len(resource_dirs) == 0:
        fail(ctx["label"]["id"] + ": resource_files matched files but none are under resource_dirs")
    if len(asset_files) > 0 and len(asset_roots) == 0:
        fail(ctx["label"]["id"] + ": assets matched files but none are under asset_dirs or assets_dir")
    dep_resource_apks = _android_resource_apks(ctx["deps"])
    dep_compiled_zips = _android_compiled_resource_zips(ctx["deps"])
    dep_asset_roots = _android_asset_roots_from_deps(ctx["deps"])
    dep_asset_files = _android_asset_files_from_deps(ctx["deps"])
    compiled_zips = _android_compile_resources(ctx, attrs, tools, resource_files, resource_dirs)
    resource_apk, r_src_dir, r_src_hash, r_txt = _android_link_resources(ctx, attrs, tools, manifest, compiled_zips, dep_compiled_zips, True, [], [])
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "android_resource",
        "namespace": _android_namespace(attrs),
        "manifest": manifest,
        "resource_apk": resource_apk,
        "r_src_dir": r_src_dir,
        "r_src_hash": r_src_hash,
        "r_txt": r_txt,
        "resource_dirs": resource_dirs,
        "asset_roots": asset_roots,
        "transitive_resource_apks": _unique([resource_apk] + dep_resource_apks),
        "compiled_resource_zips": compiled_zips,
        "transitive_compiled_resource_zips": _unique(compiled_zips + dep_compiled_zips),
        "transitive_asset_roots": _unique(asset_roots + dep_asset_roots),
        "transitive_asset_files": _unique(asset_files + dep_asset_files),
        "affected_inputs": _android_source_inputs(ctx, attrs, True) + [manifest],
    }

def _android_library_impl(ctx):
    attrs = _android_resolve_attrs(ctx, ["manifest", "resource_dirs", "asset_dirs", "assets_dir", "android_sdk", "build_tools_version", "compile_sdk", "custom_package", "namespace", "package", "javac", "jar", "java", "java_home", "kotlinc", "kotlin_home", "kotlin_stdlib", "aapt2"])
    _android_reject_unsupported_attrs(attrs, ctx["label"]["id"], ["enable_data_binding", "idl_srcs", "idl_import_root", "idl_parcelables", "idl_preprocessed", "plugins", "proguard_specs", "neverlink"])
    java_sources = _android_java_sources(ctx)
    kotlin_sources = _android_kotlin_sources(ctx)
    tools = _android_tools(ctx, attrs, True, False, len(kotlin_sources) > 0)
    manifest = _android_manifest(ctx, attrs)
    dep_compiled_zips = _android_compiled_resource_zips(ctx["deps"])
    resource_files = _android_resource_files(attrs, True)
    asset_files = _android_asset_files(attrs)
    resource_dirs = _android_resource_dirs(ctx, attrs, resource_files)
    asset_roots = _android_asset_roots(ctx, attrs, asset_files)
    if len(resource_files) > 0 and len(resource_dirs) == 0:
        fail(ctx["label"]["id"] + ": resource_files matched files but none are under resource_dirs")
    if len(asset_files) > 0 and len(asset_roots) == 0:
        fail(ctx["label"]["id"] + ": assets matched files but none are under asset_dirs or assets_dir")
    dep_jars = _android_compile_jars(ctx["deps"])
    dep_resource_apks = _android_resource_apks(ctx["deps"])
    dep_asset_roots = _android_asset_roots_from_deps(ctx["deps"])
    dep_asset_files = _android_asset_files_from_deps(ctx["deps"])
    dep_native_libraries = _android_native_libraries(ctx["deps"])
    compiled_zips = _android_compile_resources(ctx, attrs, tools, resource_files, resource_dirs)
    resource_apk, r_src_dir, r_src_hash, r_txt = _android_link_resources(ctx, attrs, tools, manifest, compiled_zips, dep_compiled_zips, True, [], [])
    classes_dir, classes_hash = _android_compile_java(ctx, attrs, tools, java_sources, r_src_dir, r_src_hash, dep_jars)
    classes_dir, classes_hash = _android_compile_kotlin(ctx, attrs, tools, kotlin_sources, classes_dir, classes_hash, dep_jars)
    classes_jar = _android_jar_classes(ctx, tools, classes_dir, classes_hash)
    aar = _android_package_aar(ctx, attrs, tools, manifest, classes_jar, r_txt, resource_dirs, asset_roots, resource_files, asset_files)
    local_compile_jars = [classes_jar]
    if len(kotlin_sources) > 0:
        local_compile_jars.append(tools["kotlin_stdlib"])
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "android_library",
        "namespace": _android_namespace(attrs),
        "manifest": manifest,
        "resource_apk": resource_apk,
        "classes_jar": classes_jar,
        "aar": aar,
        "transitive_compile_jars": _unique(local_compile_jars + dep_jars),
        "transitive_runtime_jars": _unique(local_compile_jars + _android_runtime_jars(ctx["deps"])),
        "transitive_resource_apks": _unique([resource_apk] + dep_resource_apks),
        "compiled_resource_zips": compiled_zips,
        "transitive_compiled_resource_zips": _unique(compiled_zips + dep_compiled_zips),
        "transitive_asset_roots": _unique(asset_roots + dep_asset_roots),
        "transitive_asset_files": _unique(asset_files + dep_asset_files),
        "transitive_android_native_libraries": dep_native_libraries,
        "affected_inputs": _android_source_inputs(ctx, attrs, True) + [manifest],
    }

def _android_binary_impl(ctx):
    attrs = _android_resolve_attrs(ctx, ["manifest", "resource_dirs", "asset_dirs", "assets_dir", "android_sdk", "build_tools_version", "compile_sdk", "application_id", "custom_package", "namespace", "javac", "jar", "java", "java_home", "kotlinc", "kotlin_home", "kotlin_stdlib", "aapt2", "d8", "apksigner", "zipalign", "debug_keystore", "adb", "adb_serial", "launch_activity"])
    _android_reject_unsupported_attrs(attrs, ctx["label"]["id"], ["enable_data_binding", "instruments", "manifest_values", "proguard_specs", "resource_configuration_filters", "densities", "nocompress_extensions", "startup_profiles", "native_target"])
    if ctx["capability"] == "run":
        tools = _android_adb_tools(ctx, attrs)
        return _android_run_app(ctx, attrs, tools)
    java_sources = _android_java_sources(ctx)
    kotlin_sources = _android_kotlin_sources(ctx)
    tools = _android_tools(ctx, attrs, True, True, len(kotlin_sources) > 0)
    manifest = _android_manifest(ctx, attrs)
    dep_compiled_zips = _android_compiled_resource_zips(ctx["deps"])
    resource_files = _android_resource_files(attrs, True)
    asset_files = _android_asset_files(attrs)
    resource_dirs = _android_resource_dirs(ctx, attrs, resource_files)
    asset_roots = _android_asset_roots(ctx, attrs, asset_files)
    if len(resource_files) > 0 and len(resource_dirs) == 0:
        fail(ctx["label"]["id"] + ": resource_files matched files but none are under resource_dirs")
    if len(asset_files) > 0 and len(asset_roots) == 0:
        fail(ctx["label"]["id"] + ": assets matched files but none are under asset_dirs or assets_dir")
    dep_jars = _android_compile_jars(ctx["deps"])
    runtime_jars = _android_runtime_jars(ctx["deps"])
    dep_asset_roots = _android_asset_roots_from_deps(ctx["deps"])
    dep_asset_files = _android_asset_files_from_deps(ctx["deps"])
    native_libraries = _android_native_libraries(ctx["deps"])
    compiled_zips = _android_compile_resources(ctx, attrs, tools, resource_files, resource_dirs)
    resource_apk, r_src_dir, r_src_hash, _ = _android_link_resources(ctx, attrs, tools, manifest, compiled_zips, dep_compiled_zips, False, _unique(asset_roots + dep_asset_roots), _unique(asset_files + dep_asset_files))
    classes_dir, classes_hash = _android_compile_java(ctx, attrs, tools, java_sources, r_src_dir, r_src_hash, dep_jars)
    classes_dir, classes_hash = _android_compile_kotlin(ctx, attrs, tools, kotlin_sources, classes_dir, classes_hash, dep_jars)
    classes_jar = _android_jar_classes(ctx, tools, classes_dir, classes_hash)
    if len(kotlin_sources) > 0:
        runtime_jars = _unique([tools["kotlin_stdlib"]] + runtime_jars)
    dex_dir, dex_hash = _android_dex(ctx, attrs, tools, _unique([classes_jar] + runtime_jars))
    unsigned_apk = _android_package_unsigned_apk(ctx, tools, resource_apk, dex_dir, dex_hash, native_libraries)
    aligned_apk = _android_zipalign(ctx, tools, unsigned_apk)
    apk, debug_keystore = _android_sign_or_copy(ctx, attrs, tools, aligned_apk)
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "android_binary",
        "application_id": attrs.get("application_id") or "",
        "manifest": manifest,
        "classes_jar": classes_jar,
        "resource_apk": resource_apk,
        "dex_dir": dex_dir,
        "unsigned_apk": unsigned_apk,
        "apk": apk,
        "debug_keystore": debug_keystore,
        "android_native_libraries": native_libraries,
        "affected_inputs": _android_source_inputs(ctx, attrs, True) + [manifest],
    }

_ANDROID_RESOURCE_ATTRS = [
    attr("manifest", "string", default = "AndroidManifest.xml", docs = "Package-relative AndroidManifest.xml path.", configurable = False),
    attr("resource_files", "list<string>", default = "[]", docs = "Android resource file glob patterns. Defaults to files under res when omitted."),
    attr("resource_dirs", "list<string>", default = "[\"res\"]", docs = "Package-relative resource roots passed to aapt2.", configurable = False),
    attr("assets", "list<string>", default = "[]", docs = "Android asset file glob patterns."),
    attr("asset_dirs", "list<string>", default = "[\"assets\"]", docs = "Package-relative asset roots propagated to Android package targets.", configurable = False),
    attr("assets_dir", "string", docs = "Single package-relative asset root alias.", configurable = False),
    attr("compile_sdk", "int", docs = "Android SDK API level used for android.jar. Defaults to the highest installed platform.", configurable = False),
    attr("min_sdk_version", "int", default = "23", docs = "Minimum Android API level.", configurable = False),
    attr("target_sdk_version", "int", docs = "Target Android API level. Defaults to compile_sdk.", configurable = False),
    attr("build_tools_version", "string", docs = "Android SDK build-tools version. Defaults to the highest installed version.", configurable = False),
    attr("android_sdk", "string", docs = "Android SDK root. Defaults to ANDROID_HOME or ANDROID_SDK_ROOT.", configurable = False),
    attr("aapt2", "string", docs = "Override aapt2 path.", configurable = False),
]

_ANDROID_JAVA_ATTRS = [
    attr("java_language_level", "string", default = "\"17\"", docs = "Java source and target level passed to javac.", configurable = False),
    attr("javac_opts", "list<string>", default = "[]", docs = "Additional javac flags appended after Once-managed flags.", configurable = False),
    attr("kotlinc_opts", "list<string>", default = "[]", docs = "Additional kotlinc flags appended after Once-managed flags.", configurable = False),
    attr("javac", "string", docs = "Override javac path.", configurable = False),
    attr("jar", "string", docs = "Override jar path.", configurable = False),
    attr("java", "string", docs = "Override java runtime path used by Android SDK tools.", configurable = False),
    attr("java_home", "string", docs = "Override JAVA_HOME passed to Android SDK tools.", configurable = False),
    attr("kotlinc", "string", docs = "Override kotlinc path.", configurable = False),
    attr("kotlin_home", "string", docs = "Override Kotlin home used to find kotlin-stdlib.jar.", configurable = False),
    attr("kotlin_stdlib", "string", docs = "Override kotlin-stdlib.jar path.", configurable = False),
]

_ANDROID_APK_ATTRS = [
    attr("dexopts", "list<string>", default = "[]", docs = "Additional d8 flags for APK targets.", configurable = False),
    attr("d8", "string", docs = "Override d8 path.", configurable = False),
    attr("apksigner", "string", docs = "Override apksigner path.", configurable = False),
    attr("zipalign", "string", docs = "Override zipalign path.", configurable = False),
]

android_resource = target_kind(
    docs = "Compiles Android resources into a static resource package and propagates assets to Android app targets.",
    attrs = _ANDROID_RESOURCE_ATTRS + [
        attr("namespace", "string", docs = "Java package for generated R classes.", configurable = False),
        attr("custom_package", "string", docs = "Alias for the generated R package.", configurable = False),
        attr("package", "string", docs = "Generated R package fallback.", configurable = False),
    ],
    deps = [
        dep("deps", ["android_resource"], "Android resources merged into this resource package."),
    ],
    providers = ["android_resource"],
    capabilities = [capability("build", ["default", "resources"])],
    examples = [
        example(
            "android-resource-minimal",
            name = "Minimal Android resources",
            use_when = "Use this for resources and assets that are shared by Android libraries or APKs.",
        ),
    ],
    impl = _android_resource_impl,
)

android_library = target_kind(
    docs = "Compiles Android Java and Kotlin sources with optional resources into a classes jar, static resource package, AAR, and transitive native-library package inputs consumed by Android app targets.",
    attrs = _ANDROID_RESOURCE_ATTRS + _ANDROID_JAVA_ATTRS + [
        attr("namespace", "string", docs = "Java package for generated R classes.", configurable = False),
        attr("custom_package", "string", docs = "Alias for the generated R package.", configurable = False),
        attr("package", "string", docs = "Generated R package fallback when namespace and custom_package are omitted.", configurable = False),
        attr("neverlink", "bool", default = "false", docs = "Reserved for runtime exclusion support.", configurable = False),
        attr("enable_data_binding", "bool", default = "false", docs = "Reserved for Android data binding support.", configurable = False),
        attr("idl_srcs", "list<string>", default = "[]", docs = "Reserved for AIDL support.", configurable = False),
        attr("idl_import_root", "string", docs = "Reserved for AIDL support.", configurable = False),
        attr("idl_parcelables", "list<string>", default = "[]", docs = "Reserved for AIDL support.", configurable = False),
        attr("idl_preprocessed", "list<string>", default = "[]", docs = "Reserved for AIDL support.", configurable = False),
        attr("plugins", "list<string>", default = "[]", docs = "Reserved for Java annotation processor support.", configurable = False),
        attr("proguard_specs", "list<string>", default = "[]", docs = "Reserved for consumer ProGuard specs.", configurable = False),
    ],
    deps = [
        dep("deps", ["android_library", "android_resource", "android_native_library", "native_linkable"], "Android libraries, resources, and native libraries consumed by this library."),
    ],
    providers = ["android_library", "android_archive", "java_library"],
    capabilities = [capability("build", ["default", "jar", "aar", "resources"])],
    examples = [
        example(
            "android-library-minimal",
            name = "Minimal Android library",
            use_when = "Use this for a first-party Android Kotlin or Java library with optional resources.",
        ),
    ],
    impl = _android_library_impl,
)

android_binary = target_kind(
    docs = "Builds an Android APK from Java and Kotlin sources, Android resources, native shared libraries, android_resource deps, and android_library deps.",
    attrs = _ANDROID_RESOURCE_ATTRS + _ANDROID_JAVA_ATTRS + _ANDROID_APK_ATTRS + [
        attr("application_id", "string", required = True, docs = "Android application id used for generated R classes and version metadata.", configurable = False),
        attr("namespace", "string", docs = "Java package for generated R classes. Defaults to application_id.", configurable = False),
        attr("custom_package", "string", docs = "Alias for the generated R package.", configurable = False),
        attr("version_code", "int", default = "1", docs = "APK versionCode passed to aapt2.", configurable = False),
        attr("version_name", "string", default = "\"1.0\"", docs = "APK versionName passed to aapt2.", configurable = False),
        attr("signing", "string", default = "\"debug\"", docs = "`debug` to sign with a debug key or `none` to produce an unsigned APK.", configurable = False),
        attr("debug_keystore", "string", docs = "Optional package-relative debug keystore. When omitted, Once uses ANDROID_DEBUG_KEYSTORE or ~/.android/debug.keystore.", configurable = False),
        attr("debug_keystore_password", "string", default = "\"android\"", docs = "Fixed public debug signing password.", configurable = False),
        attr("debug_key_alias", "string", default = "\"androiddebugkey\"", docs = "Key alias for debug signing only.", configurable = False),
        attr("adb", "string", docs = "Override adb path for the run capability.", configurable = False),
        attr("adb_serial", "string", docs = "Optional adb device serial for the run capability.", configurable = False),
        attr("launch_activity", "string", docs = "Optional Android activity component launched by once run. Defaults to the launcher intent for application_id.", configurable = False),
        attr("enable_data_binding", "bool", default = "false", docs = "Reserved for Android data binding support.", configurable = False),
        attr("instruments", "target", docs = "Reserved for instrumentation test support.", configurable = False),
        attr("manifest_values", "map<string,string>", default = "{}", docs = "Reserved for manifest placeholder expansion.", configurable = False),
        attr("proguard_specs", "list<string>", default = "[]", docs = "Reserved for shrinking and obfuscation.", configurable = False),
        attr("resource_configuration_filters", "list<string>", default = "[]", docs = "Reserved for resource filtering.", configurable = False),
        attr("densities", "list<string>", default = "[]", docs = "Reserved for density filtering.", configurable = False),
        attr("nocompress_extensions", "list<string>", default = "[]", docs = "Reserved for no-compress packaging options.", configurable = False),
        attr("startup_profiles", "list<string>", default = "[]", docs = "Reserved for ART startup profile packaging.", configurable = False),
        attr("native_target", "target", docs = "Reserved for native Android split support.", configurable = False),
    ],
    deps = [
        dep("deps", ["android_library", "android_resource", "android_native_library", "native_linkable"], "Android libraries, resources, and native shared libraries packaged into the APK."),
    ],
    providers = ["android_application", "android_apk"],
    capabilities = [
        capability("build", ["default", "apk", "dex", "resources"]),
        capability("run", ["default"], ["apk"]),
    ],
    examples = [
        example(
            "android-binary-minimal",
            name = "Minimal Android APK",
            use_when = "Use this for a small Android application target backed by Android SDK build-tools.",
        ),
        example(
            "native-mobile-shared-code-e2e",
            name = "Android app with shared native code",
            use_when = "Use this when an Android app should package native libraries built from Swift and Rust sources.",
        ),
    ],
    impl = _android_binary_impl,
)
