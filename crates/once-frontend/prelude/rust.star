def _ascii_env_key(value):
    upper = "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
    out = []
    for ch in value.elems():
        code = ord(ch)
        if code >= ord("a") and code <= ord("z"):
            out.append(upper[code - ord("a")])
        elif (code >= ord("A") and code <= ord("Z")) or (code >= ord("0") and code <= ord("9")):
            out.append(ch)
        else:
            out.append("_")
    return "".join(out)

def _rust_metadata_suffix(ctx):
    return _ascii_env_key(ctx["label"]["id"])

def _rust_build_dir(ctx):
    return ctx.get("build_dir") or (".once/out/" + ctx["label"]["id"])

def _rust_scratch_dir(ctx):
    return ctx.get("scratch_dir") or (".once/tmp/analysis/" + ctx["label"]["id"])

def _crate_name_from_label(ctx):
    return ctx["label"]["name"].replace("-", "_").replace(".", "_")

def _rust_attr(ctx, key, default):
    value = ctx["attr"].get(key)
    if value == None:
        return default
    return _rust_resolve_select(value, _rust_config_tokens(_rust_target_raw(ctx)), ctx["label"]["id"], key)

def _rust_target_raw(ctx):
    value = ctx["attr"].get("target")
    if value == None:
        return ""
    return value

def _is_select_shape(value):
    if type(value) != type({}):
        return False
    if len(value) != 1:
        return False
    inner = value.get("select")
    return type(inner) == type({})

def _rust_config_tokens(target):
    tokens = []
    if target:
        tokens.append(target)
        if "apple-darwin" in target:
            tokens.append("macos")
        elif "linux" in target:
            tokens.append("linux")
        elif "windows" in target:
            tokens.append("windows")
        if target.startswith("aarch64") or target.startswith("arm64"):
            tokens.extend(["arm64", "aarch64"])
        elif target.startswith("x86_64"):
            tokens.append("x86_64")
    else:
        os = host_os()
        arch = host_arch()
        tokens.append(os + "-" + arch)
        tokens.append(os)
        tokens.append(arch)
        if arch == "arm64":
            tokens.append(os + "-aarch64")
            tokens.append("aarch64")
    tokens.append("default")
    return _unique(tokens)

def _rust_select_branch(branches, tokens, label_id, attr_name):
    for token in tokens:
        if token in branches:
            return token
    fail(label_id + ": select() on `" + attr_name + "` has no branch matching the Rust configuration and no `default`")

def _rust_resolve_select(value, tokens, label_id, attr_name):
    if _is_select_shape(value):
        branches = value["select"]
        key = _rust_select_branch(branches, tokens, label_id, attr_name)
        return _rust_resolve_select(branches[key], tokens, label_id, attr_name)
    if type(value) == type([]):
        return [_rust_resolve_select(item, tokens, label_id, attr_name) for item in value]
    if type(value) == type({}):
        return {k: _rust_resolve_select(v, tokens, label_id, attr_name) for k, v in value.items()}
    return value

def _rust_crate_name(ctx):
    return _rust_attr(ctx, "crate_name", _crate_name_from_label(ctx))

def _rust_crate_root(ctx, default_root):
    return _package_relative(ctx, _rust_attr(ctx, "crate_root", default_root))

def _rust_sources(ctx, crate_root):
    srcs = _filter_by_extensions(glob(ctx["srcs"]), [".rs"])
    if crate_root not in srcs:
        srcs.append(crate_root)
    return _unique(srcs)

def _rust_source_inputs(ctx):
    return glob(ctx["srcs"])

def _rust_extra_inputs(ctx):
    return _rust_attr(ctx, "_extra_inputs", [])

def _rust_build_script_inputs(ctx):
    return glob(_rust_attr(ctx, "_build_script_inputs", []))

def _rust_target(ctx):
    return _rust_target_raw(ctx)

def _host_exe(name):
    if host_os() == "windows":
        return name + ".exe"
    return name

def _rustc_toolchain(target):
    rustc_probe = host_which("rustc")
    sysroot = host_command([rustc_probe, "--print", "sysroot"]).strip()
    rustc = sysroot + "/bin/" + _host_exe("rustc")
    version = host_command([rustc, "--version", "--verbose"]).strip()
    host_triple = _rust_host_triple_from_version(version)
    identity = "once.rust.rustc.v3\x00" + rustc + "\x00" + version + "\x00target\x00" + target
    return (rustc, identity, host_triple)

def _rust_host_triple_from_version(version):
    for line in version.split("\n"):
        if line.startswith("host: "):
            return line[len("host: "):].strip()
    return ""

def _rust_linker(ctx, crate_type, target, host_triple):
    if crate_type != "bin" and crate_type != "staticlib" and crate_type != "cdylib" and crate_type != "dylib" and crate_type != "proc-macro":
        return ([], "")
    linker = _rust_attr(ctx, "linker", "")
    if not linker and "android" in target:
        linker = _rust_android_linker(ctx, target)
    if not linker and host_os() != "windows" and (not target or target == host_triple):
        linker = host_which("cc")
    if not linker:
        return ([], "")
    return (["-C", "linker=" + linker], "\x00linker\x00" + linker + "\x00target\x00" + target)

def _rust_android_tool_prefix(target):
    if target.startswith("aarch64"):
        return "aarch64-linux-android"
    if target.startswith("armv7"):
        return "armv7a-linux-androideabi"
    if target.startswith("i686"):
        return "i686-linux-android"
    if target.startswith("x86_64"):
        return "x86_64-linux-android"
    return ""

def _rust_android_ndk_root(ctx):
    return _rust_attr(ctx, "android_ndk", "") or host_env("ANDROID_NDK_HOME")

def _rust_android_ndk_tool_dir(ctx):
    ndk = _rust_android_ndk_root(ctx)
    if not ndk:
        return ""
    candidates = []
    os = host_os()
    arch = host_arch()
    if os == "macos":
        candidates.extend(["darwin-" + arch, "darwin-arm64", "darwin-x86_64"])
    elif os == "linux":
        candidates.extend(["linux-" + arch, "linux-x86_64", "linux-arm64"])
    elif os == "windows":
        candidates.extend(["windows-" + arch, "windows-x86_64"])
    for tag in _unique(candidates):
        directory = ndk + "/toolchains/llvm/prebuilt/" + tag + "/bin"
        if host_file_exists(directory + "/" + _host_exe("clang")):
            return directory
    fail(ctx["label"]["id"] + ": Android NDK under `" + ndk + "` has no usable LLVM prebuilt for this host")

def _rust_android_linker(ctx, target):
    prefix = _rust_android_tool_prefix(target)
    if not prefix:
        fail(ctx["label"]["id"] + ": set `linker`; Once could not infer an Android NDK linker for target `" + target + "`")
    tool_dir = _rust_android_ndk_tool_dir(ctx)
    if not tool_dir:
        fail(ctx["label"]["id"] + ": set `android_ndk`, ANDROID_NDK_HOME, or `linker` so Once can find the Android NDK linker")
    api = str(_rust_attr(ctx, "android_api", 23))
    return tool_dir + "/" + prefix + api + "-clang"

def _rust_android_compile_env(ctx, target):
    if "android" not in target:
        return {}
    tool_dir = _rust_android_ndk_tool_dir(ctx)
    env = {}
    ndk = _rust_android_ndk_root(ctx)
    if ndk:
        env["ANDROID_NDK_HOME"] = ndk
    path = _rust_tool_path([tool_dir + "/" + _host_exe("clang")])
    if path:
        env["PATH"] = path
    return env

def _rust_feature_cfg_arg(feature):
    return "--cfg=feature=\"" + feature + "\""

def _rust_feature_flags(ctx):
    flags = []
    features = []
    features.extend(_rust_attr(ctx, "features", []))
    features.extend(_rust_attr(ctx, "crate_features", []))
    for feature in _unique(features):
        flags.append(_rust_feature_cfg_arg(feature))
    return flags

def _is_absolute_path(path):
    if not path:
        return False
    if path.startswith("/") or path.startswith("\\"):
        return True
    return len(path) >= 3 and path[1] == ":" and (path[2] == "/" or path[2] == "\\")

def _is_workspace_path_arg(arg):
    if not arg or arg.startswith("-") or _is_absolute_path(arg):
        return False
    return arg.startswith(".") or "/" in arg or "\\" in arg

def _workspace_arg(arg):
    if _is_workspace_path_arg(arg):
        return _workspace_absolute(arg)
    return arg

def _rust_response_path_arg(arg):
    workspace_arg = _workspace_arg(arg)
    if workspace_arg != arg or _is_absolute_path(arg) or _is_workspace_path_arg(arg):
        return workspace_arg.replace("\\", "/")
    return workspace_arg

def _rust_command_path_arg(arg):
    return _workspace_arg(arg).replace("\\", "/")

def _split_once(value, separator):
    for index in range(len(value)):
        if value[index:index + len(separator)] == separator:
            return [value[:index], value[index + len(separator):]]
    return []

def _workspace_extern_arg(arg):
    parts = _split_once(arg, "=")
    if len(parts) != 2:
        return arg
    return parts[0] + "=" + _rust_response_path_arg(parts[1])

def _workspace_search_path_arg(arg):
    parts = _split_once(arg, "=")
    if len(parts) != 2:
        return _rust_response_path_arg(arg)
    return parts[0] + "=" + _rust_response_path_arg(parts[1])

def _rust_response_file_args(ctx, args, name):
    # On Windows the command line passed to rustc can exceed the operating
    # system limit once a crate accumulates enough --extern and -L flags from
    # its dependency graph. Route the full argument list through a rustc
    # response file so only `@path` ends up on the command line. Other hosts
    # have generous limits, so the arguments stay inline there.
    #
    # The routing is intentionally unconditional on Windows rather than gated on
    # an estimated argv length. Estimating the command line length is fragile
    # (it has to account for quoting and process startup overhead, and getting
    # it wrong reintroduces the spawn failure this guards against), while the
    # cost of always routing is one small file write per compile. It does not
    # grow the action cache surface either: the arguments already participate in
    # the action digest, just by way of the response file contents instead of
    # the inline argv.
    if host_os() != "windows" or not args:
        return args
    path = _rust_scratch_dir(ctx) + "/" + _rust_declared_output(ctx, name)
    response_args = []
    mode = ""
    for arg in args:
        if mode == "path":
            response_args.append(_rust_response_path_arg(arg))
            mode = ""
            continue
        if mode == "extern":
            response_args.append(_workspace_extern_arg(arg))
            mode = ""
            continue
        if mode == "search":
            response_args.append(_workspace_search_path_arg(arg))
            mode = ""
            continue
        response_args.append(_rust_response_path_arg(arg))
        if arg == "-o":
            mode = "path"
        elif arg == "--extern":
            mode = "extern"
        elif arg == "-L":
            mode = "search"
    return [
        cmd_args(
            args = response_args,
            use_arg_file = {
                "path": path,
                "format": "rustc-response",
                "arg_format": "@{}",
            },
        ),
    ]

def _rust_user_flags(ctx):
    return _rust_attr(ctx, "rustc_flags", [])

def _rust_encoded_rustflags(ctx):
    return "\x1f".join(_rust_user_flags(ctx))

def _host_which_optional(name):
    path = host_command([host_which("sh"), "-c", "command -v " + _shell_quote(name) + " 2>/dev/null || true"]).strip()
    return path

def _rust_host_env(keys):
    env = {}
    for key in keys:
        value = host_env(key)
        if value:
            env[key] = value
    return env

def _rust_windows_tool_env():
    if host_os() != "windows":
        return {}
    env = _rust_host_env([
        "COMSPEC",
        "INCLUDE",
        "LIB",
        "LIBPATH",
        "PATHEXT",
        "PROCESSOR_ARCHITECTURE",
        "PROCESSOR_ARCHITEW6432",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "SystemDrive",
        "SystemRoot",
        "TEMP",
        "TMP",
        "UCRTVersion",
        "UniversalCRTSdkDir",
        "VCINSTALLDIR",
        "VCToolsInstallDir",
        "VSINSTALLDIR",
        "VSCMD_ARG_HOST_ARCH",
        "VSCMD_ARG_TGT_ARCH",
        "VSCMD_VER",
        "VisualStudioVersion",
        "WindowsSdkBinPath",
        "WindowsSdkDir",
        "WindowsSDKLibVersion",
        "WindowsSdkVerBinPath",
        "WindowsSDKVersion",
    ])
    path = host_env("PATH") or host_env("Path")
    if path:
        env["PATH"] = path
    return env

def _rust_unix_tool_dirs():
    os = host_os()
    if os == "macos":
        return ["/usr/bin", "/bin", "/usr/sbin", "/sbin", "/Library/Apple/usr/bin"]
    if os == "linux":
        return ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
    return []

def _rust_tool_path(tools):
    dirs = []
    for tool in tools:
        directory = _parent_dir(tool)
        if directory:
            dirs.append(directory)
    dirs.extend(_rust_unix_tool_dirs())
    return ":".join(_unique(dirs))

def _rust_path_separator():
    return ";" if host_os() == "windows" else ":"

def _rust_merge_paths(primary, fallback):
    if not primary:
        return fallback
    if not fallback:
        return primary
    separator = _rust_path_separator()
    return separator.join(_unique(primary.split(separator) + fallback.split(separator)))

def _rust_merge_env_lower_precedence(env, fallback_env):
    for key, value in fallback_env.items():
        if key == "PATH":
            path = _rust_merge_paths(env.get("PATH") or "", value)
            if path:
                env["PATH"] = path
        elif key not in env:
            env[key] = value

def _rust_c_tool_env(target, host_triple):
    env = {}
    if host_os() == "windows" or target != host_triple:
        return env
    cc = host_which("cc")
    if cc:
        env["CC"] = cc
    ar = host_which("ar")
    if ar:
        env["AR"] = ar
    ranlib = _host_which_optional("ranlib")
    if ranlib:
        env["RANLIB"] = ranlib
    pkg_config = _host_which_optional("pkg-config")
    if pkg_config:
        env["PKG_CONFIG"] = pkg_config
    path = _rust_tool_path([cc, ar, _host_which_optional("as"), _host_which_optional("ld"), ranlib, pkg_config])
    if path:
        env["PATH"] = path
    return env

def _rust_cap_lints(ctx):
    value = _rust_attr(ctx, "cap_lints", "")
    if value:
        return ["--cap-lints", value]
    return []

def _rust_env(ctx):
    env = {}
    for key, value in _rust_attr(ctx, "env", {}).items():
        env[key] = value
    for key, value in _rust_attr(ctx, "rustc_env", {}).items():
        env[key] = value
    return env

def _workspace_absolute(path):
    if not path or _is_absolute_path(path):
        return path
    root = workspace_root()
    if not root:
        return path
    return root + "/" + path

def _rust_compile_env(ctx):
    env = _rust_env(ctx)
    for key, value in _rust_windows_tool_env().items():
        if key not in env:
            env[key] = value
    manifest_dir = env.get("CARGO_MANIFEST_DIR")
    if manifest_dir:
        env["CARGO_MANIFEST_DIR"] = _workspace_absolute(_rust_manifest_dir(ctx, manifest_dir))
    return env

def _rust_manifest_dir(ctx, manifest_dir):
    if not manifest_dir or manifest_dir.startswith("/"):
        return manifest_dir
    package = ctx["label"]["package"]
    if manifest_dir == ".":
        return package
    if manifest_dir.startswith("./"):
        suffix = manifest_dir[2:]
        if package and suffix:
            return package + "/" + suffix
        return package or suffix
    if package and (manifest_dir == package or manifest_dir.startswith(package + "/")):
        return manifest_dir
    return _package_relative(ctx, manifest_dir)

def _rust_compile_action_env(ctx, target, host_triple):
    env = _rust_compile_env(ctx)
    _rust_merge_env_lower_precedence(env, _rust_android_compile_env(ctx, target or host_triple))
    _rust_merge_env_lower_precedence(env, _rust_c_tool_env(target or host_triple, host_triple))
    return env

def _rustc_sysroot(rustc):
    return _parent_dir(_parent_dir(rustc))

def _rustc_runtime_dirs(rustc, host_triple):
    if host_os() != "windows":
        return []
    sysroot = _rustc_sysroot(rustc)
    if not sysroot or not host_triple:
        return []
    return [
        sysroot + "/bin",
        sysroot + "/lib/rustlib/" + host_triple + "/bin",
    ]

def _rust_add_windows_rustc_runtime_path(env, rustc, host_triple):
    if host_os() != "windows":
        return
    path = _rust_path_separator().join(_rustc_runtime_dirs(rustc, host_triple))
    if path:
        env["PATH"] = _rust_merge_paths(path, env.get("PATH") or "")

def _rust_target_args(target):
    if target:
        return ["--target", target]
    return []

def _rust_proc_macro_codegen_args(crate_type):
    if crate_type == "proc-macro":
        return ["-C", "prefer-dynamic"]
    return []

def _rust_cfg_env(rustc, target):
    argv = [rustc, "--print", "cfg"]
    argv.extend(_rust_target_args(target))
    values = {}
    flags = []
    for line in host_command(argv).split("\n"):
        if not line:
            continue
        if "=\"" in line and _ends_with(line, "\""):
            parts = line.split("=\"")
            key = parts[0]
            value = parts[1][:len(parts[1]) - 1]
            if key not in values:
                values[key] = []
            values[key].append(value)
        else:
            flags.append(line)
    env = {}
    for key, cfg_values in values.items():
        env["CARGO_CFG_" + _ascii_env_key(key)] = ",".join(_unique(cfg_values))
    for flag in flags:
        env["CARGO_CFG_" + _ascii_env_key(flag)] = ""
    return env

def _rust_linker_flags(ctx):
    flags = []
    for flag in _rust_attr(ctx, "linker_flags", []):
        flags.extend(["-C", "link-arg=" + flag])
    return flags

def _rust_aliases(ctx):
    return _rust_attr(ctx, "crate_aliases", {})

def _rust_dep_identity(dep):
    return dep.get("label_id") or dep.get("package_name") or dep.get("crate_name") or ""

def _rust_dep_matches(dep, name):
    label_id = dep.get("label_id") or ""
    if label_id == name:
        return True
    if label_id:
        parts = label_id.split("/")
        if parts[len(parts) - 1] == name:
            return True
    return dep.get("package_name") == name or dep.get("crate_name") == name

def _rust_append_unique_dep(out, seen, dep):
    identity = _rust_dep_identity(dep)
    if not identity:
        identity = "dep-" + str(len(out))
    if identity in seen:
        return
    seen[identity] = True
    out.append(dep)

def _rust_select_dependency_set(dependency_sets, packages, label_id):
    selected = []
    seen = {}
    for package in packages:
        matches = []
        for dependency_set in dependency_sets:
            for dep in dependency_set.get("deps") or []:
                if _rust_dep_matches(dep, package):
                    matches.append(dep)
        if len(matches) == 0:
            fail(label_id + ": cargo dependency `" + package + "` was not found in cargo_dependencies deps")
        if len(matches) > 1:
            fail(label_id + ": cargo dependency `" + package + "` matched multiple cargo_dependencies entries")
        _rust_append_unique_dep(selected, seen, matches[0])
    return selected

def _rust_cargo_package_name(ctx):
    package = _rust_attr(ctx, "cargo_package", "")
    if package:
        return package
    package = _rust_env(ctx).get("CARGO_PKG_NAME")
    if package:
        return package
    return ""

def _rust_resolved_deps(ctx):
    direct = []
    dependency_sets = []
    seen = {}
    for dep in ctx["deps"]:
        if dep.get("dependency_set"):
            dependency_sets.append(dep)
        else:
            _rust_append_unique_dep(direct, seen, dep)
    package = _rust_cargo_package_name(ctx)
    matched_package = False
    if package:
        for dependency_set in dependency_sets:
            workspace_deps = dependency_set.get("workspace_deps") or {}
            if package not in workspace_deps:
                continue
            matched_package = True
            for dep in workspace_deps.get(package) or []:
                _rust_append_unique_dep(direct, seen, dep)
    if not matched_package:
        for dependency_set in dependency_sets:
            if dependency_set.get("workspace_deps"):
                continue
            for dep in dependency_set.get("deps") or []:
                _rust_append_unique_dep(direct, seen, dep)
    return direct

def _rust_dep_crate_name(dep, aliases):
    label_id = dep.get("label_id")
    package_name = dep.get("package_name")
    crate_name = dep.get("crate_name")
    extern_name = dep.get("extern_name")
    if label_id and label_id in aliases:
        return aliases[label_id]
    if label_id:
        label_parts = label_id.split("/")
        short_label = label_parts[len(label_parts) - 1]
        if short_label in aliases:
            return aliases[short_label]
    if package_name and package_name in aliases:
        return aliases[package_name]
    if crate_name and crate_name in aliases:
        return aliases[crate_name]
    if extern_name:
        return extern_name
    return crate_name

def _rust_dep_args(deps, aliases):
    args = []
    dirs = []
    for dep in deps:
        crate_name = _rust_dep_crate_name(dep, aliases)
        rlib = dep.get("rlib")
        proc_macro = dep.get("proc_macro")
        artifact = rlib or (None if host_os() == "windows" else proc_macro)
        if crate_name and artifact:
            args.extend(["--extern", crate_name + "=" + artifact])
            directory = _parent_dir(artifact)
            if directory:
                dirs.append(directory)
        if proc_macro:
            directory = _parent_dir(proc_macro)
            if directory:
                dirs.append(directory)
        for transitive in dep.get("transitive_rlibs") or []:
            directory = _parent_dir(transitive)
            if directory:
                dirs.append(directory)
        for transitive in dep.get("transitive_proc_macros") or []:
            directory = _parent_dir(transitive)
            if directory:
                dirs.append(directory)
    for directory in _unique(dirs):
        args.extend(["-L", "dependency=" + directory])
    return args

def _rust_inline_proc_macro_extern_args(deps, aliases):
    if host_os() != "windows":
        return []
    args = []
    for dep in deps:
        crate_name = _rust_dep_crate_name(dep, aliases)
        proc_macro = dep.get("proc_macro")
        if crate_name and proc_macro:
            args.extend(["--extern", crate_name + "=" + _rust_command_path_arg(proc_macro)])
    return args

def _rust_search_path_args(deps):
    args = []
    dirs = []
    for dep in deps:
        artifact = dep.get("rlib") or dep.get("proc_macro")
        if artifact:
            directory = _parent_dir(artifact)
            if directory:
                dirs.append(directory)
        for transitive in dep.get("transitive_rlibs") or []:
            directory = _parent_dir(transitive)
            if directory:
                dirs.append(directory)
        for transitive in dep.get("transitive_proc_macros") or []:
            directory = _parent_dir(transitive)
            if directory:
                dirs.append(directory)
    for directory in _unique(dirs):
        args.extend(["-L", "dependency=" + directory])
    return args

def _rust_proc_macro_dirs(deps):
    dirs = []
    for dep in deps:
        proc_macro = dep.get("proc_macro")
        if proc_macro:
            directory = _parent_dir(proc_macro)
            if directory:
                dirs.append(_workspace_absolute(directory))
        for transitive in dep.get("transitive_proc_macros") or []:
            directory = _parent_dir(transitive)
            if directory:
                dirs.append(_workspace_absolute(directory))
    return _unique(dirs)

def _rust_add_windows_proc_macro_path(env, deps):
    if host_os() != "windows":
        return
    path = _rust_path_separator().join(_rust_proc_macro_dirs(deps))
    if path:
        env["PATH"] = _rust_merge_paths(path, env.get("PATH") or "")

def _rust_builtin_extern_args(crate_type):
    if crate_type == "proc-macro":
        return ["--extern", "proc_macro"]
    return []

def _rust_build_script_env(ctx, rustc, target, host_triple, out_dir, script_path):
    env = _rust_compile_env(ctx)
    _rust_merge_env_lower_precedence(env, _rust_c_tool_env(target or host_triple, host_triple))
    for key, value in _rust_cfg_env(rustc, target).items():
        if key not in env:
            env[key] = value
    for feature in _unique(_rust_attr(ctx, "features", []) + _rust_attr(ctx, "crate_features", [])):
        env["CARGO_FEATURE_" + _ascii_env_key(feature)] = "1"
    if not env.get("CARGO_MANIFEST_DIR"):
        env["CARGO_MANIFEST_DIR"] = _workspace_absolute(_parent_dir(script_path))
    if "DEBUG" not in env:
        env["DEBUG"] = "true"
    env["HOST"] = host_triple
    if "NUM_JOBS" not in env:
        env["NUM_JOBS"] = "1"
    if "OPT_LEVEL" not in env:
        env["OPT_LEVEL"] = "0"
    env["OUT_DIR"] = _workspace_absolute(out_dir)
    if "PROFILE" not in env:
        env["PROFILE"] = "debug"
    env["RUSTC"] = rustc
    env["TARGET"] = target or host_triple
    if "CARGO_ENCODED_RUSTFLAGS" not in env:
        env["CARGO_ENCODED_RUSTFLAGS"] = _rust_encoded_rustflags(ctx)
    return env

def _rust_manifest_links(ctx):
    return _rust_env(ctx).get("CARGO_MANIFEST_LINKS") or ""

def _rust_dep_metadata_key_shell(prefix):
    printf = host_which("printf")
    tr = host_which("tr")
    return """key=${{value%%=*}}
data=${{value#*=}}
if [ "$key" != "$value" ]; then
  key=$({printf} '%s' "$key" | {tr} '[:lower:].-' '[:upper:]__')
  export {prefix}${{key}}="$data"
fi
""".format(prefix = prefix, printf = _shell_quote(printf), tr = _shell_quote(tr))

def _rust_dep_metadata_exports(deps):
    inputs = []
    snippets = []
    for dep in deps:
        links = dep.get("links")
        stdout = dep.get("build_script_stdout")
        if not links or not stdout:
            continue
        inputs.append(stdout)
        prefix = "DEP_" + _ascii_env_key(links) + "_"
        snippets.append("""while IFS= read -r line; do
  case "$line" in
    cargo:rustc-*|cargo::rustc-*|cargo:rerun-if-*|cargo::rerun-if-*|cargo:warning=*|cargo::warning=*|cargo:error=*|cargo::error=*) ;;
    cargo::metadata=*)
      value=${{line#cargo::metadata=}}
{metadata}
      ;;
    cargo:metadata=*)
      value=${{line#cargo:metadata=}}
{metadata}
      ;;
    cargo::*)
      value=${{line#cargo::}}
{metadata}
      ;;
    cargo:*)
      value=${{line#cargo:}}
{metadata}
      ;;
  esac
done < {stdout}
""".format(metadata = _rust_dep_metadata_key_shell(prefix), stdout = _shell_quote(_workspace_absolute(stdout))))
    return ("\n".join(snippets), _unique(inputs))

def _rust_build_script_output_pipeline(runner, status, stdout):
    runner_exec = _shell_quote(_workspace_absolute(runner))
    printf = _shell_quote(host_which("printf"))
    tee = _shell_quote(host_which("tee"))
    status_abs = _shell_quote(status)
    stdout_abs = _shell_quote(_workspace_absolute(stdout))
    status_capture = "{ " + runner_exec + " 2>&1; " + printf + " '%s' \"$?\" > " + status_abs + "; }"
    return status_capture + " | " + tee + " " + stdout_abs

def _rust_build_script_run_shell(runner, stdout, run_env, metadata_exports):
    status = run_env["OUT_DIR"] + "/.once-build-script-status"
    status_abs = _shell_quote(status)
    lines = [
        "cd " + _shell_quote(run_env["CARGO_MANIFEST_DIR"]),
    ]
    if metadata_exports:
        lines.append(metadata_exports)
    lines.extend([
        _rust_build_script_output_pipeline(runner, status, stdout),
        "code=$(" + _shell_quote(host_which("cat")) + " " + status_abs + ")",
        "exit \"$code\"",
    ])
    return "\n".join(lines)

def _rust_build_script(ctx, rustc, identity, target, host_triple, edition, dep_args, dep_inputs, deps, metadata_deps, feature_flags):
    build_script = _rust_attr(ctx, "build_script", "")
    if not build_script:
        return (None, [], {}, None)
    script_path = _package_relative(ctx, build_script)
    runner = declare_output(_rust_declared_output(ctx, "build-script" + _rust_output_extension("bin", "")))
    out_dir = declare_output(_rust_declared_output(ctx, "build"))
    stdout = declare_output(_rust_declared_output(ctx, "build-script.stdout"))
    linker_args, linker_identity = _rust_linker(ctx, "bin", "", host_triple)
    compile_args = [
        "--crate-name", "build_script_build",
        "--crate-type", "bin",
        "--edition", edition,
        "-C", "metadata=" + _rust_metadata_suffix(ctx) + "_BUILD",
        "-o", runner,
    ]
    compile_args.extend(feature_flags)
    compile_args.extend(_rust_cap_lints(ctx))
    compile_args.extend(dep_args)
    compile_args.extend(linker_args)
    compile_args.append(script_path)
    compile_argv = (
        [rustc] +
        _rust_inline_proc_macro_extern_args(deps, _rust_aliases(ctx)) +
        _rust_response_file_args(ctx, compile_args, "build-script-rustc.rsp")
    )
    build_script_compile_env = _rust_compile_action_env(ctx, host_triple, host_triple)
    _rust_add_windows_rustc_runtime_path(build_script_compile_env, rustc, host_triple)
    _rust_add_windows_proc_macro_path(build_script_compile_env, deps)
    run_action(
        argv = compile_argv,
        inputs = _unique([script_path] + dep_inputs + _rust_extra_inputs(ctx)),
        outputs = [runner],
        env = build_script_compile_env,
        toolchain_identity = identity + linker_identity + "\x00build-script",
        identifier = ctx["label"]["id"] + ":build-script-rustc",
    )
    run_env = _rust_build_script_env(ctx, rustc, target, host_triple, out_dir, script_path)
    metadata_exports, metadata_inputs = _rust_dep_metadata_exports(metadata_deps)
    prepare_path(out_dir, kind = "directory", identifier = ctx["label"]["id"] + ":build-script-out-dir")
    prepare_path(out_dir + "/.once-build-script-status", kind = "remove", identifier = ctx["label"]["id"] + ":build-script-status-clean")
    run_script = _rust_build_script_run_shell(runner, stdout, run_env, metadata_exports)
    run_action(
        argv = [host_which("sh"), "-c", run_script],
        inputs = _unique([runner] + metadata_inputs + _rust_source_inputs(ctx) + _rust_build_script_inputs(ctx)),
        outputs = [out_dir, stdout],
        env = run_env,
        toolchain_identity = identity + "\x00build-script-run",
        identifier = ctx["label"]["id"] + ":build-script",
    )
    compile_env = _rust_compile_action_env(ctx, target, host_triple)
    compile_env["OUT_DIR"] = _workspace_absolute(out_dir)
    return (out_dir, [script_path, stdout], compile_env, stdout)

def _rustc_unix_build_script_args(argv, stdout):
    script = """set -eu
while IFS= read -r line; do
  case "$line" in
    cargo:rustc-cfg=*|cargo::rustc-cfg=*)
      value=${{line#cargo:rustc-cfg=}}
      value=${{value#cargo::rustc-cfg=}}
      set -- "$@" --cfg "$value"
      ;;
    cargo:rustc-check-cfg=*|cargo::rustc-check-cfg=*)
      value=${{line#cargo:rustc-check-cfg=}}
      value=${{value#cargo::rustc-check-cfg=}}
      set -- "$@" --check-cfg "$value"
      ;;
    cargo:rustc-env=*|cargo::rustc-env=*)
      value=${{line#cargo:rustc-env=}}
      value=${{value#cargo::rustc-env=}}
      export "$value"
      ;;
    cargo:rustc-link-arg=*|cargo::rustc-link-arg=*)
      value=${{line#cargo:rustc-link-arg=}}
      value=${{value#cargo::rustc-link-arg=}}
      set -- "$@" -C "link-arg=$value"
      ;;
    cargo:rustc-link-lib=*|cargo::rustc-link-lib=*)
      value=${{line#cargo:rustc-link-lib=}}
      value=${{value#cargo::rustc-link-lib=}}
      set -- "$@" -l "$value"
      ;;
    cargo:rustc-link-search=*|cargo::rustc-link-search=*)
      value=${{line#cargo:rustc-link-search=}}
      value=${{value#cargo::rustc-link-search=}}
      set -- "$@" -L "$value"
      ;;
  esac
done < {stdout}
exec "$@"
""".format(stdout = _shell_quote(stdout))
    return [host_which("sh"), "-c", script, "once-rustc"] + argv

def _rustc_windows_build_script_args(ctx, argv, stdout):
    wrapper = declare_output(_rust_declared_output(ctx, "rustc-build-script-wrapper.ps1"))
    script = """$ErrorActionPreference = 'Stop'
$rustcArgs = New-Object 'System.Collections.Generic.List[string]'
foreach ($arg in $args) {
  [void]$rustcArgs.Add($arg)
}
$dynamicRustcArgs = New-Object 'System.Collections.Generic.List[string]'

function Get-DirectiveValue($line, $name) {
  $oldPrefix = 'cargo:' + $name + '='
  if ($line.StartsWith($oldPrefix)) {
    return $line.Substring($oldPrefix.Length)
  }
  $newPrefix = 'cargo::' + $name + '='
  if ($line.StartsWith($newPrefix)) {
    return $line.Substring($newPrefix.Length)
  }
  return $null
}

foreach ($line in [System.IO.File]::ReadLines(""" + _powershell_quote(stdout) + """)) {
  $value = Get-DirectiveValue $line 'rustc-cfg'
  if ($null -ne $value) {
    [void]$dynamicRustcArgs.Add('--cfg')
    [void]$dynamicRustcArgs.Add($value)
    continue
  }
  $value = Get-DirectiveValue $line 'rustc-check-cfg'
  if ($null -ne $value) {
    [void]$dynamicRustcArgs.Add('--check-cfg')
    [void]$dynamicRustcArgs.Add($value)
    continue
  }
  $value = Get-DirectiveValue $line 'rustc-env'
  if ($null -ne $value) {
    $equals = $value.IndexOf('=')
    if ($equals -ge 0) {
      $name = $value.Substring(0, $equals)
      $envValue = $value.Substring($equals + 1)
      [System.Environment]::SetEnvironmentVariable($name, $envValue, 'Process')
    }
    continue
  }
  $value = Get-DirectiveValue $line 'rustc-link-arg'
  if ($null -ne $value) {
    [void]$dynamicRustcArgs.Add('-C')
    [void]$dynamicRustcArgs.Add("link-arg=$value")
    continue
  }
  $value = Get-DirectiveValue $line 'rustc-link-lib'
  if ($null -ne $value) {
    [void]$dynamicRustcArgs.Add('-l')
    [void]$dynamicRustcArgs.Add($value)
    continue
  }
  $value = Get-DirectiveValue $line 'rustc-link-search'
  if ($null -ne $value) {
    [void]$dynamicRustcArgs.Add('-L')
    [void]$dynamicRustcArgs.Add($value)
    continue
  }
}

if ($rustcArgs.Count -eq 0) {
  throw 'missing rustc argv'
}
$program = $rustcArgs[0]
$responseFile = $null
try {
  if ($dynamicRustcArgs.Count -gt 0) {
    $responseFile = [System.IO.Path]::Combine([System.IO.Path]::GetTempPath(), [System.IO.Path]::GetRandomFileName() + '.rsp')
    $encoding = New-Object System.Text.UTF8Encoding -ArgumentList $false
    [System.IO.File]::WriteAllLines($responseFile, $dynamicRustcArgs.ToArray(), $encoding)
    [void]$rustcArgs.Add("@$responseFile")
  }
  $rest = @()
  if ($rustcArgs.Count -gt 1) {
    $rest = $rustcArgs.GetRange(1, $rustcArgs.Count - 1).ToArray()
  }
  & $program @rest
  if ($global:LASTEXITCODE -ne $null) {
    exit $global:LASTEXITCODE
  }
  if (-not $?) {
    exit 1
  }
} finally {
  if ($null -ne $responseFile -and [System.IO.File]::Exists($responseFile)) {
    [System.IO.File]::Delete($responseFile)
  }
}
"""
    write_path(wrapper, script)
    return ([host_which("powershell"), "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", wrapper] + argv, [wrapper])

def _rustc_with_build_script_args(ctx, argv, stdout):
    if host_os() == "windows":
        return _rustc_windows_build_script_args(ctx, argv, stdout)
    return (_rustc_unix_build_script_args(argv, stdout), [])

def _rust_dep_inputs(deps):
    inputs = []
    for dep in deps:
        rlib = dep.get("rlib")
        if rlib:
            inputs.append(rlib)
        proc_macro = dep.get("proc_macro")
        if proc_macro:
            inputs.append(proc_macro)
        for transitive in dep.get("transitive_rlibs") or []:
            inputs.append(transitive)
        for transitive in dep.get("transitive_proc_macros") or []:
            inputs.append(transitive)
    return _unique(inputs)

def _collect_transitive(deps, key, own_values):
    out = []
    for value in own_values:
        out.append(value)
    for dep in deps:
        for value in dep.get(key) or []:
            out.append(value)
    return _unique(out)

def _rust_android_abi_for_target(target):
    if "android" not in target:
        return ""
    if target.startswith("aarch64"):
        return "arm64-v8a"
    if target.startswith("armv7"):
        return "armeabi-v7a"
    if target.startswith("i686"):
        return "x86"
    if target.startswith("x86_64"):
        return "x86_64"
    return ""

def _rust_android_abi(ctx, target, crate_type):
    configured = _rust_attr(ctx, "android_abi", "")
    if configured:
        return configured
    if crate_type != "cdylib" and crate_type != "dylib":
        return ""
    abi = _rust_android_abi_for_target(target)
    if "android" in target and not abi:
        fail(ctx["label"]["id"] + ": set `android_abi`; Once could not infer an Android ABI from target `" + target + "`")
    return abi

def _rust_native_library_key(library):
    return (library.get("abi") or "") + "\x00" + (library.get("path") or "")

def _rust_unique_native_libraries(libraries):
    seen = {}
    out = []
    for library in libraries:
        abi = library.get("abi") or ""
        path = library.get("path") or ""
        if not abi or not path:
            continue
        key = _rust_native_library_key(library)
        if key not in seen:
            seen[key] = True
            out.append({"abi": abi, "path": path})
    return out

def _rust_collect_android_native_libraries(deps, own):
    out = []
    out.extend(own)
    for dep in deps:
        out.extend(dep.get("transitive_android_native_libraries") or [])
        out.extend(dep.get("android_native_libraries") or [])
    return _rust_unique_native_libraries(out)

def _rust_output_extension(crate_type, target):
    os_key = target or host_os()
    if crate_type == "rlib":
        return ".rlib"
    if crate_type == "staticlib":
        return ".lib" if "windows" in os_key else ".a"
    if crate_type == "cdylib" or crate_type == "dylib" or crate_type == "proc-macro":
        if "windows" in os_key:
            return ".dll"
        if "apple" in os_key or "darwin" in os_key or os_key == "macos":
            return ".dylib"
        return ".so"
    if crate_type == "bin":
        return ".exe" if "windows" in os_key else ""
    return ""

def _rust_extra_filename(ctx, crate_type):
    if crate_type == "proc-macro" and host_os() == "windows":
        return "-" + _rust_metadata_suffix(ctx)
    return ""

def _rust_extra_filename_args(ctx, crate_type):
    extra_filename = _rust_extra_filename(ctx, crate_type)
    if extra_filename:
        return ["-C", "extra-filename=" + extra_filename]
    return []

def _rust_effective_target(ctx, crate_type):
    if crate_type == "proc-macro":
        return ""
    return _rust_target(ctx)

def _rust_output_name(ctx, crate_type):
    crate_name = _rust_crate_name(ctx)
    target = _rust_effective_target(ctx, crate_type)
    if crate_type == "bin":
        return crate_name + _rust_output_extension(crate_type, target)
    if crate_type == "rlib" or crate_type == "staticlib" or crate_type == "cdylib" or crate_type == "dylib" or crate_type == "proc-macro":
        prefix = "" if "windows" in (target or host_os()) and crate_type != "rlib" else "lib"
        return prefix + crate_name + _rust_extra_filename(ctx, crate_type) + _rust_output_extension(crate_type, target)
    return crate_name

def _rust_declared_output(ctx, name):
    prefix = ctx["attr"].get("_output_prefix") or ""
    return prefix + name

def _rust_output_args(crate_type, output):
    if crate_type == "proc-macro" and host_os() == "windows":
        return ["--out-dir", _parent_dir(output)]
    return ["-o", output]

def _rust_compile(ctx, crate_type, default_root, output_name):
    target = _rust_effective_target(ctx, crate_type)
    rustc, identity, host_triple = _rustc_toolchain(target)
    crate_name = _rust_crate_name(ctx)
    edition = _rust_attr(ctx, "edition", "2021")
    crate_root = _rust_crate_root(ctx, default_root)
    srcs = _rust_sources(ctx, crate_root)
    output = declare_output(_rust_declared_output(ctx, output_name))
    aliases = _rust_aliases(ctx)
    deps = _rust_resolved_deps(ctx)
    dep_args = _rust_dep_args(deps, aliases)
    dep_inputs = _rust_dep_inputs(deps)
    search_deps = ctx.get("search_deps") or []
    search_args = _rust_search_path_args(search_deps)
    search_inputs = _rust_dep_inputs(search_deps)
    build_deps = ctx.get("build_deps")
    if build_deps == None:
        build_deps = []
    build_dep_args = _rust_dep_args(build_deps, aliases)
    build_dep_args.extend(search_args)
    build_dep_inputs = _unique(_rust_dep_inputs(build_deps) + search_inputs)
    feature_flags = _rust_feature_flags(ctx)
    build_out_dir, build_inputs, build_env, build_stdout = _rust_build_script(ctx, rustc, identity, target, host_triple, edition, build_dep_args, build_dep_inputs, build_deps, deps + build_deps, feature_flags)
    compile_env = build_env if build_env else _rust_compile_action_env(ctx, target, host_triple)
    _rust_add_windows_rustc_runtime_path(compile_env, rustc, host_triple)
    _rust_add_windows_proc_macro_path(compile_env, deps)
    linker_args, linker_identity = _rust_linker(ctx, crate_type, target, host_triple)
    rustc_args = [
        "--crate-name", crate_name,
        "--crate-type", crate_type,
        "--edition", edition,
        "-C", "metadata=" + _rust_metadata_suffix(ctx),
    ]
    rustc_args.extend(_rust_extra_filename_args(ctx, crate_type))
    rustc_args.extend(_rust_output_args(crate_type, output))
    rustc_args.extend(_rust_target_args(target))
    rustc_args.extend(_rust_proc_macro_codegen_args(crate_type))
    rustc_args.extend(feature_flags)
    rustc_args.extend(_rust_user_flags(ctx))
    rustc_args.extend(_rust_cap_lints(ctx))
    rustc_args.extend(_rust_builtin_extern_args(crate_type))
    rustc_args.extend(dep_args)
    rustc_args.extend(search_args)
    rustc_args.extend(linker_args)
    rustc_args.extend(_rust_linker_flags(ctx))
    rustc_args.append(crate_root)
    argv = (
        [rustc] +
        _rust_inline_proc_macro_extern_args(deps, aliases) +
        _rust_response_file_args(ctx, rustc_args, "rustc.rsp")
    )
    if build_stdout:
        wrapped = _rustc_with_build_script_args(ctx, argv, build_stdout)
        argv = wrapped[0]
        build_inputs.extend(wrapped[1])
    run_action(
        argv = argv,
        inputs = _unique(srcs + dep_inputs + search_inputs + build_inputs + _rust_extra_inputs(ctx)),
        outputs = [output],
        env = compile_env,
        toolchain_identity = identity + linker_identity,
        identifier = ctx["label"]["id"] + ":rustc",
    )
    own_android_native_libraries = []
    android_abi = _rust_android_abi(ctx, target, crate_type)
    if android_abi and (crate_type == "cdylib" or crate_type == "dylib"):
        own_android_native_libraries.append({"abi": android_abi, "path": output})
    own_native_archives = [output] if crate_type == "staticlib" else []
    provider = {
        "label_id": ctx["label"]["id"],
        "target_kind": "rust_library",
        "crate_name": crate_name,
        "crate_type": crate_type,
        "target": target,
        "root": crate_root,
        "out_dir": build_out_dir,
        "links": _rust_manifest_links(ctx),
        "build_script_stdout": build_stdout,
        "rlib": output if crate_type == "rlib" else None,
        "binary": output if crate_type == "bin" else None,
        "staticlib": output if crate_type == "staticlib" else None,
        "dylib": output if crate_type == "cdylib" or crate_type == "dylib" else None,
        "proc_macro": output if crate_type == "proc-macro" else None,
        "transitive_rlibs": _collect_transitive(deps, "transitive_rlibs", [output] if crate_type == "rlib" else []),
        "transitive_proc_macros": _collect_transitive(deps, "transitive_proc_macros", [output] if crate_type == "proc-macro" else []),
        "transitive_sources": _collect_transitive(deps, "transitive_sources", srcs),
        "archive": output if crate_type == "staticlib" else "",
        "transitive_archives": _collect_transitive(deps, "transitive_archives", own_native_archives),
        "transitive_linkopts": _collect_transitive(deps, "transitive_linkopts", _rust_attr(ctx, "native_linkopts", [])),
        "android_abi": android_abi,
        "android_native_libraries": own_android_native_libraries,
        "transitive_android_native_libraries": _rust_collect_android_native_libraries(deps, own_android_native_libraries),
    }
    return provider

def _rust_library_impl(ctx):
    crate_type = _rust_attr(ctx, "crate_type", "rlib")
    return _rust_compile(ctx, crate_type, "src/lib.rs", _rust_output_name(ctx, crate_type))

def _rust_binary_impl(ctx):
    return _rust_compile(ctx, "bin", "src/main.rs", _rust_output_name(ctx, "bin"))

def _rust_crate_impl(ctx):
    provider = _rust_compile(ctx, "rlib", "src/lib.rs", _rust_output_name(ctx, "rlib"))
    provider["package_name"] = _rust_attr(ctx, "package_name", ctx["label"]["name"])
    provider["version"] = _rust_attr(ctx, "version", "")
    provider["source"] = _rust_attr(ctx, "source", "")
    provider["checksum"] = _rust_attr(ctx, "checksum", "")
    return provider

def _rust_proc_macro_impl(ctx):
    provider = _rust_compile(ctx, "proc-macro", "src/lib.rs", _rust_output_name(ctx, "proc-macro"))
    provider["package_name"] = _rust_attr(ctx, "package_name", ctx["label"]["name"])
    provider["version"] = _rust_attr(ctx, "version", "")
    provider["source"] = _rust_attr(ctx, "source", "")
    provider["checksum"] = _rust_attr(ctx, "checksum", "")
    return provider

def _cargo_dependencies_impl(ctx):
    resolved = _cargo_resolved_metadata(ctx)
    deps, providers_by_name = _cargo_compile_resolved_specs(ctx, resolved["specs"])
    workspace_deps = _cargo_workspace_deps(ctx, resolved["metadata"], providers_by_name)
    packages = _rust_attr(ctx, "packages", [])
    if packages:
        deps = _rust_select_dependency_set([{"deps": deps}], packages, ctx["label"]["id"])
        workspace_deps = _cargo_filter_workspace_deps(workspace_deps, deps)
    return {
        "dependency_set": True,
        "deps": deps,
        "packages": packages,
        "workspace_deps": workspace_deps,
    }

def _cargo_metadata_inputs(ctx):
    srcs = glob(ctx["srcs"])
    if len(srcs) > 0:
        return srcs
    manifest = _package_relative(ctx, _rust_attr(ctx, "manifest", "Cargo.toml"))
    lockfile = _package_relative(ctx, _rust_attr(ctx, "lockfile", "Cargo.lock"))
    return _unique([manifest, lockfile])

def _cargo_feature_args(ctx):
    args = []
    if _rust_attr(ctx, "no_default_features", False):
        args.append("--no-default-features")
    if _rust_attr(ctx, "all_features", False):
        args.append("--all-features")
    features = _rust_attr(ctx, "features", [])
    if features:
        args.extend(["--features", ",".join(features)])
    return args

def _cargo_resolved_metadata(ctx):
    manifest = _package_relative(ctx, _rust_attr(ctx, "manifest", "Cargo.toml"))
    vendor_dir = _trim_trailing_slash(_rust_attr(ctx, "vendor_dir", "third_party/rust/vendor"))
    target = _rust_target(ctx)
    _, _, host_triple = _rustc_toolchain(target)
    cargo = host_which("cargo")
    filter_platform = target or host_triple
    metadata = _cargo_metadata_for_platform(ctx, cargo, manifest, filter_platform)
    host_metadata = None
    if target and target != host_triple:
        host_metadata = _cargo_metadata_for_platform(ctx, cargo, manifest, host_triple)
    resolver_attrs = {"vendor_dir": vendor_dir}
    if target:
        resolver_attrs["target"] = target
    resolver_ctx = {
        "attrs": resolver_attrs,
    }
    return {
        "metadata": metadata,
        "specs": _cargo_metadata_targets(resolver_ctx, metadata, host_metadata),
    }

def _cargo_metadata_for_platform(ctx, cargo, manifest, platform):
    argv = [
        cargo,
        "metadata",
        "--locked",
        "--format-version", "1",
        "--manifest-path", _workspace_absolute(manifest),
    ]
    if platform:
        argv.extend(["--filter-platform", platform])
    argv.extend(_cargo_feature_args(ctx))
    metadata_content = host_command(argv)
    return json_decode(metadata_content)

def _cargo_ref_name(ref):
    if ref.startswith("./"):
        return ref[2:]
    parts = ref.split("/")
    return parts[len(parts) - 1]

def _cargo_copy_attrs(attrs):
    out = {}
    for key, value in (attrs or {}).items():
        out[key] = value
    return out

def _cargo_compile_resolved_specs(ctx, specs):
    remaining = []
    by_name = {}
    for spec in specs:
        remaining.append(spec)
        by_name[spec["name"]] = spec
    providers_by_name = {}
    providers = []
    metadata_inputs = _cargo_metadata_inputs(ctx)
    for _ in range(len(specs) + 1):
        if len(remaining) == 0:
            break
        next_remaining = []
        progressed = False
        for spec in remaining:
            dep_providers = []
            build_dep_providers = []
            ready = True
            for dep_ref in spec.get("deps") or []:
                dep_name = _cargo_ref_name(dep_ref)
                provider = providers_by_name.get(dep_name)
                if provider == None:
                    ready = False
                    break
                dep_providers.append(provider)
            if not ready:
                next_remaining.append(spec)
                continue
            for dep_ref in spec.get("build_deps") or []:
                dep_name = _cargo_ref_name(dep_ref)
                provider = providers_by_name.get(dep_name)
                if provider == None:
                    ready = False
                    break
                build_dep_providers.append(provider)
            if not ready:
                next_remaining.append(spec)
                continue
            provider = _cargo_compile_resolved_spec(ctx, spec, dep_providers, build_dep_providers, metadata_inputs, _cargo_search_deps(providers))
            providers_by_name[spec["name"]] = provider
            providers.append(provider)
            progressed = True
        if not progressed:
            names = [spec["name"] for spec in remaining]
            fail(ctx["label"]["id"] + ": Cargo dependency graph has a cycle or missing dependency among " + ", ".join(names))
        remaining = next_remaining
    if len(remaining) > 0:
        names = [spec["name"] for spec in remaining]
        fail(ctx["label"]["id"] + ": Cargo dependency graph has a cycle or missing dependency among " + ", ".join(names))
    return (providers, providers_by_name)

def _cargo_search_deps(providers):
    if host_os() != "windows":
        return []
    return providers

def _cargo_compile_resolved_spec(ctx, spec, dep_providers, build_dep_providers, metadata_inputs, search_deps):
    attrs = _cargo_copy_attrs(spec.get("attrs") or {})
    attrs["_output_prefix"] = spec["name"] + "/"
    attrs["_extra_inputs"] = metadata_inputs
    spec_ctx = {
        "label": {
            "package": ctx["label"]["package"],
            "name": spec["name"],
            "id": ctx["label"]["id"] + "/" + spec["name"],
        },
        "attr": attrs,
        "deps": dep_providers,
        "build_deps": build_dep_providers,
        "search_deps": search_deps,
        "srcs": spec.get("srcs") or [],
    }
    if spec.get("kind") == "rust_proc_macro":
        return _rust_proc_macro_impl(spec_ctx)
    return _rust_crate_impl(spec_ctx)

def _cargo_workspace_deps(ctx, metadata, providers_by_name):
    packages = metadata.get("packages") or []
    duplicate_counts = _cargo_duplicate_counts(packages)
    nodes = _cargo_resolve_nodes(metadata)
    host_dependency_ids = _cargo_host_dependency_ids(packages, nodes)
    id_to_target_name, _ = _cargo_dependency_name_maps(packages, duplicate_counts, host_dependency_ids)
    out = {}
    for package in packages:
        if package.get("source") != None:
            continue
        node = nodes.get(package.get("id")) or {}
        dep_refs, aliases = _cargo_metadata_deps(node, id_to_target_name, False)
        deps = []
        seen = {}
        for dep_ref in dep_refs:
            dep_name = _cargo_ref_name(dep_ref)
            provider = providers_by_name.get(dep_name)
            if provider != None:
                alias = aliases.get(dep_name)
                if alias:
                    provider = _cargo_provider_with_extern_name(provider, alias)
                _rust_append_unique_dep(deps, seen, provider)
        out[package["name"]] = deps
    return out

def _cargo_provider_with_extern_name(provider, extern_name):
    out = {}
    for key, value in provider.items():
        out[key] = value
    out["extern_name"] = extern_name
    return out

def _cargo_filter_workspace_deps(workspace_deps, deps):
    allowed = {}
    for dep in deps:
        allowed[_rust_dep_identity(dep)] = True
    out = {}
    for package, package_deps in workspace_deps.items():
        filtered = []
        seen = {}
        for dep in package_deps:
            if _rust_dep_identity(dep) in allowed:
                _rust_append_unique_dep(filtered, seen, dep)
        out[package] = filtered
    return out

def _trim_trailing_slash(value):
    if _ends_with(value, "/"):
        return value[:len(value) - 1]
    return value

def _cargo_lock_content(ctx):
    files = ctx["files"]
    if "Cargo.lock" in files:
        return files["Cargo.lock"]
    for _, content in files.items():
        return content
    fail("cargo resolver requires a lockfile input")

def _cargo_metadata_content(ctx):
    files = ctx["files"]
    for name in ["cargo-metadata.json", "metadata.json", "Cargo.metadata.json"]:
        if name in files:
            return files[name]
    for name, content in files.items():
        if _ends_with(name, ".json"):
            return content
    return None

def _cargo_dep_ref(raw):
    source = None
    left = raw
    if " (" in raw:
        parts = raw.split(" (")
        left = parts[0]
        source = parts[1]
        if _ends_with(source, ")"):
            source = source[:len(source) - 1]
    parts = left.split()
    if len(parts) < 1:
        fail("Cargo.lock dependency entry is empty")
    return {
        "name": parts[0],
        "version": parts[1] if len(parts) > 1 else None,
        "source": source,
    }

def _cargo_key(parts):
    return "\x00".join(parts)

def _cargo_source_suffix(source):
    out = []
    for ch in source.elems():
        code = ord(ch)
        if (code >= ord("a") and code <= ord("z")) or (code >= ord("A") and code <= ord("Z")) or (code >= ord("0") and code <= ord("9")) or ch == "_" or ch == "-" or ch == ".":
            out.append(ch)
        else:
            out.append("_")
    return "".join(out)

def _cargo_duplicate_counts(packages):
    counts = {}
    for package in packages:
        if package.get("source") == None:
            continue
        key = _cargo_key([package["name"], package["version"]])
        counts[key] = counts.get(key, 0) + 1
    return counts

def _cargo_target_name(package, duplicate_counts):
    base = package["name"] + "-" + package["version"]
    key = _cargo_key([package["name"], package["version"]])
    if duplicate_counts.get(key, 0) <= 1:
        return base
    return base + "-" + _cargo_source_suffix(package.get("source") or "")

def _cargo_index(packages, duplicate_counts):
    by_name_version_source = {}
    by_name_version = {}
    by_name = {}
    for package in packages:
        source = package.get("source")
        if source == None:
            continue
        target = _cargo_target_name(package, duplicate_counts)
        nvs_key = _cargo_key([package["name"], package["version"], source])
        nv_key = _cargo_key([package["name"], package["version"]])
        by_name_version_source[nvs_key] = target
        if nv_key not in by_name_version:
            by_name_version[nv_key] = []
        by_name_version[nv_key].append(target)
        if package["name"] not in by_name:
            by_name[package["name"]] = []
        by_name[package["name"]].append(target)
    return {
        "by_name_version_source": by_name_version_source,
        "by_name_version": by_name_version,
        "by_name": by_name,
    }

def _cargo_single_match(matches, dep_name):
    if len(matches) == 1:
        return "./" + matches[0]
    fail("Cargo.lock dependency `" + dep_name + "` is ambiguous across multiple packages; include version and source in the dependency entry")

def _cargo_resolve_dep(dep, index):
    name = dep["name"]
    version = dep["version"]
    source = dep["source"]
    if version != None and source != None:
        key = _cargo_key([name, version, source])
        target = index["by_name_version_source"].get(key)
        if target == None:
            fail("Cargo.lock dependency `" + name + "` version `" + version + "` from `" + source + "` has no package entry")
        return "./" + target
    if version != None:
        key = _cargo_key([name, version])
        matches = index["by_name_version"].get(key)
        if matches == None:
            fail("Cargo.lock dependency `" + name + "` version `" + version + "` has no package entry")
        return _cargo_single_match(matches, name)
    matches = index["by_name"].get(name)
    if matches == None:
        fail("Cargo.lock dependency `" + name + "` has no package entry")
    return _cargo_single_match(matches, name)

def _cargo_resolver_impl(ctx):
    metadata_content = _cargo_metadata_content(ctx)
    if metadata_content != None:
        return _cargo_metadata_resolver_impl(ctx, metadata_content)
    lock = toml_decode(_cargo_lock_content(ctx))
    packages = lock.get("package") or []
    duplicate_counts = _cargo_duplicate_counts(packages)
    index = _cargo_index(packages, duplicate_counts)
    vendor_dir = _trim_trailing_slash(ctx["attrs"].get("vendor_dir") or "vendor")
    targets = []
    for package in packages:
        if package.get("source") == None:
            continue
        name = _cargo_target_name(package, duplicate_counts)
        source_root = vendor_dir + "/" + name
        attrs = {
            "package_name": package["name"],
            "crate_name": package["name"].replace("-", "_"),
            "version": package["version"],
            "crate_root": source_root + "/src/lib.rs",
            "edition": "2021",
            "source": package.get("source") or "",
            "cap_lints": "allow",
        }
        checksum = package.get("checksum")
        if checksum != None:
            attrs["checksum"] = checksum
        deps = []
        for raw_dep in package.get("dependencies") or []:
            deps.append(_cargo_resolve_dep(_cargo_dep_ref(raw_dep), index))
        targets.append({
            "name": name,
            "kind": "rust_crate",
            "deps": deps,
            "srcs": _cargo_package_source_globs(source_root),
            "attrs": attrs,
        })
    return targets

def _cargo_normalized_path(path):
    return path.replace("\\", "/")

def _cargo_package_root(package):
    manifest = _cargo_normalized_path(package.get("manifest_path") or "")
    if manifest:
        return _parent_dir(manifest)
    return ""

def _cargo_source_rel(package, target):
    src = _cargo_normalized_path(target.get("src_path") or "")
    root = _cargo_normalized_path(_cargo_package_root(package))
    prefix = root + "/"
    if root and src.startswith(prefix):
        return src[len(prefix):]
    return "src/lib.rs"

def _cargo_source_glob(source_root, crate_root):
    parent = _parent_dir(crate_root)
    if parent.startswith(source_root + "/"):
        rel_parent = parent[len(source_root) + 1:]
        if rel_parent:
            return source_root + "/" + rel_parent + "/**/*.rs"
    return source_root + "/src/**/*.rs"

def _cargo_package_source_globs(source_root):
    return [
        source_root + "/Cargo.toml",
        source_root + "/build.rs",
        source_root + "/src/**/*.rs",
    ]

def _cargo_library_target(package):
    for target in package.get("targets") or []:
        if "proc-macro" in (target.get("kind") or []) or "proc-macro" in (target.get("crate_types") or []):
            return target
    for target in package.get("targets") or []:
        kinds = target.get("kind") or []
        crate_types = target.get("crate_types") or []
        if "lib" in kinds or "rlib" in crate_types or "lib" in crate_types:
            return target
    return None

def _cargo_build_script_target(package):
    for target in package.get("targets") or []:
        if "custom-build" in (target.get("kind") or []):
            return target
    return None

def _cargo_kind_for_target(target):
    if "proc-macro" in (target.get("kind") or []) or "proc-macro" in (target.get("crate_types") or []):
        return "rust_proc_macro"
    return "rust_crate"

def _cargo_package_is_proc_macro(package):
    target = _cargo_library_target(package)
    if target == None:
        return False
    return _cargo_kind_for_target(target) == "rust_proc_macro"

def _cargo_host_variant_name(package, duplicate_counts):
    return _cargo_target_name(package, duplicate_counts) + "-host"

def _cargo_host_dependency_ids(packages, nodes, host_nodes = None):
    package_by_id, _ = _cargo_package_indexes(packages, _cargo_duplicate_counts(packages))
    if host_nodes == None:
        host_nodes = nodes
    needed = {}
    stack = []
    for package in packages:
        package_id = package.get("id")
        if package.get("source") == None or not package_id:
            continue
        if _cargo_package_is_proc_macro(package):
            stack.append(package_id)
        if _cargo_build_script_target(package) != None:
            node = nodes.get(package_id) or {}
            for dep in node.get("deps") or []:
                if _cargo_dep_is_build(dep):
                    dep_id = dep.get("pkg")
                    dep_package = package_by_id.get(dep_id)
                    if dep_package != None and dep_package.get("source") != None:
                        stack.append(dep_id)
    for _ in range(len(packages) + 1):
        if len(stack) == 0:
            break
        current = stack
        stack = []
        for package_id in current:
            package = package_by_id.get(package_id)
            if package == None or package.get("source") == None or package_id in needed:
                continue
            needed[package_id] = True
            node = host_nodes.get(package_id) or nodes.get(package_id) or {}
            include_build = _cargo_build_script_target(package) != None
            for dep in node.get("deps") or []:
                if not _cargo_dep_is_runtime(dep) and not (include_build and _cargo_dep_is_build(dep)):
                    continue
                dep_id = dep.get("pkg")
                dep_package = package_by_id.get(dep_id)
                if dep_package != None and dep_package.get("source") != None and dep_id not in needed:
                    stack.append(dep_id)
    return needed

def _cargo_dependency_name_maps(packages, duplicate_counts, host_dependency_ids):
    target_names = {}
    host_names = {}
    for package in packages:
        package_id = package.get("id")
        if package.get("source") == None or not package_id:
            continue
        target_name = _cargo_target_name(package, duplicate_counts)
        target_names[package_id] = target_name
        if _cargo_package_is_proc_macro(package):
            host_names[package_id] = target_name
        elif host_dependency_ids.get(package_id):
            host_names[package_id] = _cargo_host_variant_name(package, duplicate_counts)
    return (target_names, host_names)

def _cargo_package_indexes(packages, duplicate_counts):
    by_id = {}
    id_to_target_name = {}
    for package in packages:
        package_id = package.get("id")
        if not package_id:
            continue
        by_id[package_id] = package
        if package.get("source") != None:
            id_to_target_name[package_id] = _cargo_target_name(package, duplicate_counts)
    return (by_id, id_to_target_name)

def _cargo_resolve_nodes(metadata):
    nodes = {}
    resolve = metadata.get("resolve") or {}
    for node in resolve.get("nodes") or []:
        nodes[node.get("id")] = node
    return nodes

def _cargo_merge_packages(primary, secondary):
    out = []
    seen = {}
    for package in primary + secondary:
        package_id = package.get("id")
        key = package_id or package.get("name") + "@" + package.get("version")
        if key in seen:
            continue
        seen[key] = True
        out.append(package)
    return out

def _cargo_package_id_set(packages):
    ids = {}
    for package in packages:
        package_id = package.get("id")
        if package_id:
            ids[package_id] = True
    return ids

def _cargo_dep_is_runtime(dep):
    saw_kind = False
    for kind in dep.get("dep_kinds") or []:
        saw_kind = True
        if kind.get("kind") == None:
            return True
    return not saw_kind

def _cargo_dep_is_build(dep):
    for kind in dep.get("dep_kinds") or []:
        if kind.get("kind") == "build":
            return True
    return False

def _cargo_metadata_dep_refs(node, id_to_target_name, include_runtime, include_build):
    deps = []
    aliases = {}
    for dep in node.get("deps") or []:
        runtime = include_runtime and _cargo_dep_is_runtime(dep)
        build = include_build and _cargo_dep_is_build(dep)
        if not runtime and not build:
            continue
        dep_id = dep.get("pkg")
        target_name = id_to_target_name.get(dep_id)
        if target_name == None:
            continue
        deps.append("./" + target_name)
        local_name = dep.get("name")
        if local_name:
            aliases[target_name] = local_name
    return (_unique(deps), aliases)

def _cargo_metadata_deps(node, id_to_target_name, include_build):
    return _cargo_metadata_dep_refs(node, id_to_target_name, True, include_build)

def _cargo_metadata_build_deps(node, id_to_target_name):
    return _cargo_metadata_dep_refs(node, id_to_target_name, False, True)

def _cargo_version_components(version):
    build_parts = version.split("+")
    without_build = build_parts[0]
    pre_parts = without_build.split("-")
    core = pre_parts[0]
    pre = "-".join(pre_parts[1:]) if len(pre_parts) > 1 else ""
    core_parts = core.split(".")
    return {
        "major": core_parts[0] if len(core_parts) > 0 else "",
        "minor": core_parts[1] if len(core_parts) > 1 else "",
        "patch": core_parts[2] if len(core_parts) > 2 else "",
        "pre": pre,
    }

def _cargo_package_authors(package):
    return ":".join(package.get("authors") or [])

def _cargo_package_field(package, key):
    value = package.get(key)
    if value == None:
        return ""
    return value

def _cargo_rustc_env(package, target, source_root):
    crate_name = target.get("name") or package["name"].replace("-", "_")
    version = package["version"]
    components = _cargo_version_components(version)
    env = {
        "CARGO_CRATE_NAME": crate_name,
        "CARGO_MANIFEST_DIR": source_root,
        "CARGO_PKG_AUTHORS": _cargo_package_authors(package),
        "CARGO_PKG_DESCRIPTION": _cargo_package_field(package, "description"),
        "CARGO_PKG_HOMEPAGE": _cargo_package_field(package, "homepage"),
        "CARGO_PKG_LICENSE": _cargo_package_field(package, "license"),
        "CARGO_PKG_LICENSE_FILE": _cargo_package_field(package, "license_file"),
        "CARGO_PKG_NAME": package["name"],
        "CARGO_PKG_README": _cargo_package_field(package, "readme"),
        "CARGO_PKG_REPOSITORY": _cargo_package_field(package, "repository"),
        "CARGO_PKG_RUST_VERSION": _cargo_package_field(package, "rust_version"),
        "CARGO_PKG_VERSION": version,
        "CARGO_PKG_VERSION_MAJOR": components["major"],
        "CARGO_PKG_VERSION_MINOR": components["minor"],
        "CARGO_PKG_VERSION_PATCH": components["patch"],
        "CARGO_PKG_VERSION_PRE": components["pre"],
    }
    links = package.get("links")
    if links != None:
        env["CARGO_MANIFEST_LINKS"] = links
    return env

def _cargo_metadata_attrs(package, target, node, source_root, crate_root, aliases):
    attrs = {
        "package_name": package["name"],
        "crate_name": target.get("name") or package["name"].replace("-", "_"),
        "version": package["version"],
        "crate_root": crate_root,
        "edition": target.get("edition") or package.get("edition") or "2021",
        "features": node.get("features") or [],
        "source": package.get("source") or "",
        "rustc_env": _cargo_rustc_env(package, target, source_root),
        "cap_lints": "allow",
    }
    if aliases:
        attrs["crate_aliases"] = aliases
    build_script = _cargo_build_script_target(package)
    if build_script != None:
        attrs["build_script"] = source_root + "/" + _cargo_source_rel(package, build_script)
    return attrs

def _cargo_metadata_resolver_impl(ctx, metadata_content):
    metadata = json_decode(metadata_content)
    return _cargo_metadata_targets(ctx, metadata)

def _cargo_merge_aliases(left, right):
    out = {}
    for key, value in left.items():
        out[key] = value
    for key, value in right.items():
        out[key] = value
    return out

def _cargo_metadata_target_spec(package, target, node, source_root, crate_root, name, kind, deps, build_deps, aliases, rust_target):
    attrs = _cargo_metadata_attrs(package, target, node, source_root, crate_root, aliases)
    if rust_target:
        attrs["target"] = rust_target
    checksum = package.get("checksum")
    if checksum != None:
        attrs["checksum"] = checksum
    if attrs.get("build_script"):
        attrs["_build_script_inputs"] = [source_root + "/**"]
    spec = {
        "name": name,
        "kind": kind,
        "deps": deps,
        "srcs": _cargo_package_source_globs(source_root),
        "attrs": attrs,
    }
    if build_deps:
        spec["build_deps"] = build_deps
    return spec

def _cargo_metadata_targets(ctx, metadata, host_metadata = None):
    target_packages = metadata.get("packages") or []
    host_packages = host_metadata.get("packages") if host_metadata != None else []
    packages = _cargo_merge_packages(target_packages, host_packages)
    target_package_ids = _cargo_package_id_set(target_packages)
    duplicate_counts = _cargo_duplicate_counts(packages)
    nodes = _cargo_resolve_nodes(metadata)
    host_nodes = _cargo_resolve_nodes(host_metadata) if host_metadata != None else nodes
    host_dependency_ids = _cargo_host_dependency_ids(packages, nodes, host_nodes)
    id_to_target_name, id_to_host_name = _cargo_dependency_name_maps(packages, duplicate_counts, host_dependency_ids)
    vendor_dir = _trim_trailing_slash(ctx["attrs"].get("vendor_dir") or "vendor")
    rust_target = ctx["attrs"].get("target") or ""
    targets = []
    for package in packages:
        if package.get("source") == None:
            continue
        target = _cargo_library_target(package)
        if target == None:
            continue
        package_id = package.get("id")
        name = id_to_target_name.get(package_id) or _cargo_target_name(package, duplicate_counts)
        kind = _cargo_kind_for_target(target)
        source_root = _cargo_vendor_source_root(vendor_dir, package, _cargo_target_name(package, duplicate_counts))
        rel_root = _cargo_source_rel(package, target)
        crate_root = source_root + "/" + rel_root
        node = nodes.get(package_id) or {}
        host_node = host_nodes.get(package_id) or node

        if kind == "rust_proc_macro":
            deps, aliases = _cargo_metadata_deps(host_node, id_to_host_name, False)
            build_deps, build_aliases = _cargo_metadata_build_deps(host_node, id_to_host_name)
            targets.append(_cargo_metadata_target_spec(package, target, host_node, source_root, crate_root, name, kind, deps, build_deps, _cargo_merge_aliases(aliases, build_aliases), ""))
            continue

        if target_package_ids.get(package_id):
            deps, aliases = _cargo_metadata_deps(node, id_to_target_name, False)
            build_deps, build_aliases = _cargo_metadata_build_deps(node, id_to_host_name)
            targets.append(_cargo_metadata_target_spec(package, target, node, source_root, crate_root, name, kind, deps, build_deps, _cargo_merge_aliases(aliases, build_aliases), rust_target))

        host_name = id_to_host_name.get(package_id)
        if host_name:
            host_deps, host_aliases = _cargo_metadata_deps(host_node, id_to_host_name, False)
            host_build_deps, host_build_aliases = _cargo_metadata_build_deps(host_node, id_to_host_name)
            targets.append(_cargo_metadata_target_spec(package, target, host_node, source_root, crate_root, host_name, kind, host_deps, host_build_deps, _cargo_merge_aliases(host_aliases, host_build_aliases), ""))
    return targets

def _cargo_vendor_source_root(vendor_dir, package, target_name):
    versioned = vendor_dir + "/" + target_name
    if _cargo_vendor_candidate_matches(versioned, package["version"]):
        return versioned
    bare = vendor_dir + "/" + package["name"]
    if _cargo_vendor_candidate_matches(bare, package["version"]):
        return bare
    return versioned

def _cargo_vendor_candidate_matches(candidate, version):
    manifest = _workspace_absolute(candidate + "/Cargo.toml")
    version_line = "version = \"" + version + "\""
    return host_file_exists(manifest) and host_file_contains(manifest, version_line)

_RUST_COMMON_ATTRS = [
    attr("crate_name", "string", docs = "Rust crate name passed to rustc. Defaults to the target name with `-` and `.` rewritten as `_`.", configurable = False),
    attr("crate_root", "string", docs = "Package-relative path to lib.rs or main.rs. Defaults to src/lib.rs for libraries and src/main.rs for binaries.", configurable = False),
    attr("edition", "string", default = "2021", docs = "Rust edition passed to rustc.", configurable = False),
    attr("features", "list<string>", default = "[]", docs = "Cargo feature names lowered to rustc `--cfg=feature=...` flags."),
    attr("crate_features", "list<string>", default = "[]", docs = "Bazel-compatible alias for `features`."),
    attr("target", "string", docs = "Rust target triple passed to `rustc --target`. Defaults to the host target.", configurable = False),
    attr("env", "map<string, string>", default = "{}", docs = "Environment variables for rustc, matching Buck2's `env` attribute.", configurable = False),
    attr("rustc_env", "map<string, string>", default = "{}", docs = "Bazel-compatible rustc environment variables.", configurable = False),
    attr("rustc_flags", "list<string>", default = "[]", docs = "Additional rustc flags appended after Once-managed flags.", configurable = False),
    attr("cap_lints", "string", docs = "Optional rustc lint cap passed as `--cap-lints`; generated Cargo dependencies use `allow` to match Cargo dependency builds.", configurable = False),
    attr("linker", "string", docs = "Optional linker path passed as `-C linker=...`. Defaults to `cc` for host Unix binary-like targets and to the Android NDK clang wrapper for Android targets when ANDROID_NDK_HOME or android_ndk is set.", configurable = False),
    attr("linker_flags", "list<string>", default = "[]", docs = "Additional linker flags lowered to `-C link-arg=...`.", configurable = False),
    attr("native_linkopts", "list<string>", default = "[]", docs = "Linker flags propagated to native consumers such as Apple app or framework targets.", configurable = False),
    attr("android_abi", "string", docs = "Android ABI directory for cdylib or dylib outputs, such as `arm64-v8a`; inferred from common Android target triples when omitted.", configurable = False),
    attr("android_api", "int", default = "23", docs = "Android API level used to select the NDK clang wrapper for Android targets.", configurable = False),
    attr("android_ndk", "string", docs = "Android NDK root used to find clang wrapper linkers. Defaults to ANDROID_NDK_HOME.", configurable = False),
    attr("crate_aliases", "map<string, string>", default = "{}", docs = "Map dependency label, package name, or crate name to the local extern crate name.", configurable = False),
    attr("cargo_package", "string", docs = "Cargo package name used to select direct external deps from a cargo_dependencies dependency set. Defaults to CARGO_PKG_NAME when present.", configurable = False),
    attr("build_script", "string", docs = "Package-relative Cargo build script path. Once compiles and runs it before rustc, consumes common cargo:rustc-* stdout directives, and passes direct dependency links metadata as DEP_* env vars.", configurable = False),
]

cargo_dependencies = target_kind(
    docs = "Cacheable Cargo dependency set consumed by Rust targets. The target kind reads Cargo.toml and Cargo.lock through `cargo metadata`, compiles resolved external crates as Once actions, and exposes them as one graph dependency.",
    attrs = [
        attr("manifest", "string", default = "Cargo.toml", docs = "Workspace-relative Cargo manifest path passed to `cargo metadata --manifest-path`.", configurable = False),
        attr("lockfile", "string", default = "Cargo.lock", docs = "Workspace-relative Cargo lockfile path included in the dependency action key.", configurable = False),
        attr("vendor_dir", "string", default = "third_party/rust/vendor", docs = "Workspace-relative directory containing vendored crate sources.", configurable = False),
        attr("packages", "list<string>", default = "[]", docs = "Optional package names to expose from this dependency set. Defaults to all resolved external packages.", configurable = True),
        attr("features", "list<string>", default = "[]", docs = "Cargo features passed to `cargo metadata --features`.", configurable = True),
        attr("all_features", "bool", default = "false", docs = "Pass `--all-features` to Cargo metadata.", configurable = True),
        attr("no_default_features", "bool", default = "false", docs = "Pass `--no-default-features` to Cargo metadata.", configurable = True),
        attr("target", "string", docs = "Rust target triple passed to Cargo as `--filter-platform`. Defaults to the host target.", configurable = False),
    ],
    providers = ["rust_dependency_set"],
    capabilities = [capability("build", [])],
    examples = [
        example(
            "rust-binary-with-crate",
            name = "Rust binary with Cargo dependencies",
            use_when = "Use this when a binary consumes external crates resolved from Cargo.toml and Cargo.lock.",
        ),
    ],
    impl = _cargo_dependencies_impl,
)

rust_library = target_kind(
    docs = "Rust library compiled with rustc. Rlibs feed Rust deps, staticlibs feed Apple linkers, and Android cdylibs are packaged as native shared libraries.",
    attrs = _RUST_COMMON_ATTRS + [
        attr("crate_type", "string", default = "rlib", docs = "Rust crate type for the library output. Defaults to `rlib`; final artifacts may use `staticlib`, `cdylib`, or `dylib`.", configurable = False),
    ],
    deps = [dep("deps", ["rust_crate", "rust_proc_macro", "rust_dependency_set"], "Rust crate dependencies consumed through --extern.")],
    providers = ["rust_crate", "native_linkable", "apple_linkable", "android_native_library"],
    capabilities = [capability("build", ["library"])],
    examples = [
        example(
            "rust-library-minimal",
            name = "Minimal Rust library",
            use_when = "Start here for a first-party Rust rlib target with no external dependencies.",
        ),
        example(
            "rust-native-mobile-library",
            name = "Rust native mobile library",
            use_when = "Use this when Rust shared code should build as a static library for Apple and a shared library for Android.",
        ),
        example(
            "native-mobile-shared-code-e2e",
            name = "Shared mobile code app graph",
            use_when = "Use this when one Rust codebase should be packaged into Android and linked into Apple apps alongside Swift and Kotlin shared code.",
        ),
    ],
    impl = _rust_library_impl,
)

rust_binary = target_kind(
    docs = "Rust executable compiled with rustc from a main crate and Rust crate deps.",
    attrs = _RUST_COMMON_ATTRS,
    deps = [dep("deps", ["rust_crate", "rust_proc_macro", "rust_dependency_set"], "Rust crate dependencies consumed through --extern.")],
    providers = ["rust_binary"],
    capabilities = [capability("build", ["binary"])],
    examples = [
        example(
            "rust-binary-with-crate",
            name = "Rust binary with Cargo dependencies",
            use_when = "Use this when a binary consumes external crates resolved from Cargo.toml and Cargo.lock.",
        ),
    ],
    impl = _rust_binary_impl,
)

rust_crate = target_kind(
    docs = "Resolved third-party Rust crate lowered from Cargo package metadata into a normal Once Rust library target.",
    attrs = _RUST_COMMON_ATTRS + [
        attr("package_name", "string", required = True, docs = "Original Cargo package name."),
        attr("version", "string", required = True, docs = "Resolved Cargo package version."),
        attr("source", "string", docs = "Cargo source identifier, such as registry+https://github.com/rust-lang/crates.io-index.", configurable = False),
        attr("checksum", "string", docs = "Cargo.lock checksum for registry packages.", configurable = False),
    ],
    deps = [dep("deps", ["rust_crate", "rust_proc_macro", "rust_dependency_set"], "Resolved Cargo package dependencies.")],
    providers = ["rust_crate"],
    capabilities = [capability("build", ["rlib"])],
    examples = [
        example(
            "rust-crate-minimal",
            name = "Rust crate minimal",
            use_when = "Use this when inspecting the lowered target shape for one resolved Cargo package.",
        ),
    ],
    impl = _rust_crate_impl,
)

rust_proc_macro = target_kind(
    docs = "Rust procedural macro compiled for the execution host and consumed through --extern by Rust targets.",
    attrs = _RUST_COMMON_ATTRS + [
        attr("package_name", "string", docs = "Original Cargo package name when the target was lowered from Cargo metadata."),
        attr("version", "string", docs = "Resolved Cargo package version when the target was lowered from Cargo metadata."),
        attr("source", "string", docs = "Cargo source identifier, such as registry+https://github.com/rust-lang/crates.io-index.", configurable = False),
        attr("checksum", "string", docs = "Cargo.lock checksum for registry packages.", configurable = False),
    ],
    deps = [dep("deps", ["rust_crate", "rust_proc_macro", "rust_dependency_set"], "Rust crate dependencies consumed by the procedural macro.")],
    providers = ["rust_proc_macro"],
    capabilities = [capability("build", ["proc_macro"])],
    examples = [
        example(
            "rust-proc-macro-minimal",
            name = "Minimal Rust procedural macro",
            use_when = "Use when a Rust crate exports a procedural macro consumed by other Rust targets.",
        ),
    ],
    impl = _rust_proc_macro_impl,
)
