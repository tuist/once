def _zig_attr(ctx, key, default):
    value = ctx["attr"].get(key)
    if value == None:
        return default
    return value

def _zig_label_suffix(ctx):
    return _zig_safe_name(ctx["label"]["id"])

def _zig_safe_name(value):
    out = []
    for ch in value.elems():
        code = ord(ch)
        if (code >= ord("a") and code <= ord("z")) or (code >= ord("A") and code <= ord("Z")) or (code >= ord("0") and code <= ord("9")):
            out.append(ch)
        elif ch == "_":
            out.append("_u")
        else:
            out.append("_x" + str(code) + "_")
    return "".join(out)

def _zig_main(ctx, default):
    configured = _zig_attr(ctx, "main", "")
    if configured:
        return _package_relative(ctx, configured)
    if default:
        return _package_relative(ctx, default)
    return ""

def _zig_import_name(ctx):
    return _zig_attr(ctx, "import_name", "") or ctx["label"]["name"]

def _zig_canonical_name(ctx):
    return "once_" + _zig_label_suffix(ctx)

def _zig_globs(patterns):
    if not patterns:
        return []
    return glob(patterns)

def _zig_path_attr(ctx, key):
    return [_package_relative(ctx, path) for path in _zig_attr(ctx, key, [])]

def _zig_source_inputs(ctx, main):
    inputs = []
    if main:
        inputs.append(main)
    inputs.extend(_zig_globs(ctx["srcs"]))
    inputs.extend(_zig_globs(_zig_attr(ctx, "extra_srcs", [])))
    return _unique(inputs)

def _zig_data_inputs(ctx):
    return _zig_globs(_zig_attr(ctx, "data", []))

def _zig_extra_docs(ctx):
    return _zig_globs(_zig_attr(ctx, "extra_docs", []))

def _zig_csrcs(ctx):
    return _zig_path_attr(ctx, "csrcs")

def _zig_collect_dep_sources(deps):
    inputs = []
    for dep in deps:
        for path in dep.get("transitive_sources") or []:
            inputs.append(path)
    return _unique(inputs)

def _zig_collect_dep_data(deps):
    inputs = []
    for dep in deps:
        for path in dep.get("transitive_data") or []:
            inputs.append(path)
    return _unique(inputs)

def _zig_zig_module_deps(deps):
    return [dep for dep in deps if dep.get("zig_module")]

def _zig_direct_c_deps(deps):
    return [dep for dep in deps if dep.get("c_provider")]

def _zig_collect_android_native_libraries(deps, own):
    out = []
    out.extend(own)
    for dep in deps:
        out.extend(dep.get("transitive_android_native_libraries") or [])
        out.extend(dep.get("android_native_libraries") or [])
    return _unique_native_libraries(out)

def _zig_dep_import_name(ctx, dep):
    names = _zig_attr(ctx, "import_names", {})
    label_id = dep.get("label_id") or ""
    if label_id and label_id in names:
        return names[label_id]
    if label_id:
        parts = label_id.split("/")
        short = parts[len(parts) - 1]
        if short in names:
            return names[short]
    import_name = dep.get("import_name") or ""
    if import_name and import_name in names:
        return names[import_name]
    canonical_name = dep.get("canonical_name") or ""
    if canonical_name and canonical_name in names:
        return names[canonical_name]
    return import_name

def _zig_dep_import_keys(dep):
    keys = []
    label_id = dep.get("label_id") or ""
    if label_id:
        keys.append(label_id)
        parts = label_id.split("/")
        keys.append(parts[len(parts) - 1])
    import_name = dep.get("import_name") or ""
    if import_name:
        keys.append(import_name)
    canonical_name = dep.get("canonical_name") or ""
    if canonical_name:
        keys.append(canonical_name)
    return _unique(keys)

def _zig_validate_import_names(ctx, deps):
    names = _zig_attr(ctx, "import_names", {})
    if len(names) == 0:
        return
    for key, value in names.items():
        if not value:
            fail(ctx["label"]["id"] + ": import_names[\"" + key + "\"] must not be empty")
        matches = 0
        for dep in _zig_zig_module_deps(deps):
            if key in _zig_dep_import_keys(dep):
                matches += 1
        if matches == 0:
            fail(ctx["label"]["id"] + ": import_names key `" + key + "` does not match any Zig module dependency")
        if matches > 1:
            fail(ctx["label"]["id"] + ": import_names key `" + key + "` is ambiguous across Zig module dependencies; use a full dependency label")

def _zig_module_context(ctx, main, deps, uses_c_module):
    mappings = []
    seen = {}
    seen_imports = {}
    _zig_validate_import_names(ctx, deps)
    for dep in _zig_zig_module_deps(deps):
        canonical_name = dep.get("canonical_name") or ""
        import_name = _zig_dep_import_name(ctx, dep)
        if not canonical_name or not import_name:
            continue
        if canonical_name in seen:
            fail(ctx["label"]["id"] + ": duplicate Zig module canonical name `" + canonical_name + "`; rename one target or adjust its label")
        seen[canonical_name] = True
        if import_name in seen_imports:
            fail(ctx["label"]["id"] + ": duplicate Zig import name `" + import_name + "`; use import_names to assign unique imports")
        seen_imports[import_name] = True
        mappings.append({
            "import_name": import_name,
            "canonical_name": canonical_name,
        })
    if uses_c_module and "c" not in seen:
        if "c" in seen_imports:
            fail(ctx["label"]["id"] + ": Zig import name `c` conflicts with the generated C module")
        mappings.append({
            "import_name": "c",
            "canonical_name": "c",
        })
    return {
        "import_name": _zig_import_name(ctx),
        "canonical_name": _zig_canonical_name(ctx),
        "main": main,
        "deps": mappings,
        "zigopts": _zig_attr(ctx, "zigopts", []),
    }

def _zig_context_identity(context):
    return context.get("canonical_name") or context.get("main") or ""

def _zig_append_unique_context(out, seen, context):
    identity = _zig_context_identity(context)
    if not identity or identity in seen:
        return
    seen[identity] = True
    out.append(context)

def _zig_transitive_module_contexts(deps):
    out = []
    seen = {}
    for dep in deps:
        if not dep.get("zig_module"):
            continue
        context = dep.get("module_context")
        if context != None:
            _zig_append_unique_context(out, seen, context)
        for transitive in dep.get("transitive_module_contexts") or []:
            _zig_append_unique_context(out, seen, transitive)
    return out

def _zig_module_args_for_context(context):
    args = []
    for mapping in context.get("deps") or []:
        args.extend(["--dep", mapping["import_name"] + "=" + mapping["canonical_name"]])
    args.extend(context.get("zigopts") or [])
    args.append("-M" + context["canonical_name"] + "=" + context["main"])
    return args

def _zig_module_args(root_context, dep_contexts, c_module_context):
    args = []
    args.extend(_zig_module_args_for_context(root_context))
    for context in dep_contexts:
        args.extend(_zig_module_args_for_context(context))
    if c_module_context != None:
        args.extend(_zig_module_args_for_context(c_module_context))
    return args

def _zig_target_os(ctx):
    target = _zig_attr(ctx, "target", "")
    if not target:
        return host_os()
    if "windows" in target:
        return "windows"
    if "macos" in target or "darwin" in target:
        return "macos"
    if "linux" in target:
        return "linux"
    return ""

def _zig_exe_extension(ctx):
    if _zig_target_os(ctx) == "windows":
        return ".exe"
    return ""

def _zig_object_extension(ctx):
    if _zig_target_os(ctx) == "windows":
        return ".obj"
    return ".o"

def _zig_static_extension(ctx):
    if _zig_target_os(ctx) == "windows":
        return ".lib"
    return ".a"

def _zig_shared_extension(ctx):
    os = _zig_target_os(ctx)
    if os == "windows":
        return ".dll"
    if os == "macos":
        return ".dylib"
    return ".so"

def _zig_lib_prefix(ctx):
    if _zig_target_os(ctx) == "windows":
        return ""
    return "lib"

def _zig_output_name(ctx, kind):
    name = _zig_attr(ctx, "output_name", "") or ctx["label"]["name"]
    if kind == "binary" or kind == "test":
        return name + _zig_exe_extension(ctx)
    if kind == "object":
        return name + _zig_object_extension(ctx)
    if kind == "static":
        return _zig_lib_prefix(ctx) + name + _zig_static_extension(ctx)
    if kind == "shared":
        configured = _zig_attr(ctx, "shared_lib_name", "")
        if configured:
            return configured
        return _zig_lib_prefix(ctx) + name + _zig_shared_extension(ctx)
    return name

def _zig_docs_name(ctx):
    return ctx["label"]["name"] + ".docs"

def _zig_toolchain(ctx):
    configured = _zig_attr(ctx, "zig", "")
    zig = configured if configured else host_which("zig")
    version = host_command([zig, "version"]).strip()
    expected_version = _zig_attr(ctx, "zig_version", "")
    if expected_version and version != expected_version:
        fail(ctx["label"]["id"] + ": Zig compiler version is `" + version + "`, expected `" + expected_version + "`")
    bootstrapped = _zig_tristate_attr(ctx, "bootstrapped")
    return (zig, "once.zig.v4\x00" + zig + "\x00" + version + "\x00host\x00" + host_os() + "\x00" + host_arch() + "\x00target\x00" + _zig_attr(ctx, "target", "") + "\x00bootstrapped\x00" + str(bootstrapped))

def _zig_tristate_attr(ctx, key):
    value = _zig_attr(ctx, key, -1)
    if value == -1 or value == 0 or value == 1:
        return value
    fail(ctx["label"]["id"] + ": `" + key + "` must be -1, 0, or 1")

def _zig_effective_string_setting(ctx, key, host_key):
    value = _zig_attr(ctx, key, "")
    host_value = _zig_attr(ctx, host_key, "")
    if value and host_value:
        fail(ctx["label"]["id"] + ": set either `" + key + "` or `" + host_key + "`, not both")
    return value or host_value

def _zig_effective_list_setting(ctx, key, host_key):
    values = _zig_attr(ctx, key, [])
    host_values = _zig_attr(ctx, host_key, [])
    if len(values) > 0 and len(host_values) > 0:
        fail(ctx["label"]["id"] + ": set either `" + key + "` or `" + host_key + "`, not both")
    return values if len(values) > 0 else host_values

def _zig_mode_args(ctx):
    optimize = _zig_attr(ctx, "optimize", "")
    mode = _zig_effective_string_setting(ctx, "mode", "host_mode")
    if optimize and mode:
        fail(ctx["label"]["id"] + ": set either `optimize` or `mode`, not both")
    if optimize:
        return ["-O", optimize]
    if not mode:
        return []
    if mode == "auto":
        mode = "debug"
    if mode == "debug":
        return ["-O", "Debug"]
    if mode == "release_safe":
        return ["-O", "ReleaseSafe"]
    if mode == "release_small":
        return ["-O", "ReleaseSmall"]
    if mode == "release_fast":
        return ["-O", "ReleaseFast"]
    fail(ctx["label"]["id"] + ": `mode` must be `auto`, `debug`, `release_safe`, `release_small`, or `release_fast`")

def _zig_threaded_args(ctx):
    threaded = _zig_effective_string_setting(ctx, "threaded", "host_threaded")
    if not threaded:
        return []
    if threaded == "multi":
        return ["-fno-single-threaded"]
    if threaded == "single":
        return ["-fsingle-threaded"]
    fail(ctx["label"]["id"] + ": `threaded` must be `multi` or `single`")

def _zig_global_zigopts(ctx):
    return _zig_effective_list_setting(ctx, "zigopt", "host_zigopt")

def _zig_validate_link_configuration(ctx):
    _zig_tristate_attr(ctx, "use_cc_common_link")
    _zig_tristate_attr(ctx, "host_use_cc_common_link")
    _zig_tristate_attr(ctx, "use_standalone_translate_c")

def _zig_target_common_args(ctx):
    args = []
    _zig_validate_link_configuration(ctx)
    target = _zig_attr(ctx, "target", "")
    if target:
        args.extend(["-target", target])
    cpu = _zig_attr(ctx, "cpu", "")
    if cpu:
        args.append("-mcpu=" + cpu)
    args.extend(_zig_mode_args(ctx))
    args.extend(_zig_threaded_args(ctx))
    return args

def _zig_strip_and_zigopts_args(ctx):
    args = []
    if _zig_attr(ctx, "strip", False) or _zig_attr(ctx, "strip_debug_symbols", False):
        args.append("-fstrip")
    args.extend(_zig_global_zigopts(ctx))
    return args

def _zig_compiler_runtime_args(ctx):
    compiler_runtime = _zig_attr(ctx, "compiler_runtime", "default")
    if compiler_runtime == "include":
        return ["-fcompiler-rt"]
    if compiler_runtime == "exclude":
        return ["-fno-compiler-rt"]
    if compiler_runtime != "default":
        fail(ctx["label"]["id"] + ": compiler_runtime must be `default`, `include`, or `exclude`")
    return []

def _zig_common_args(ctx):
    args = _zig_target_common_args(ctx)
    args.extend(_zig_compiler_runtime_args(ctx))
    args.extend(_zig_strip_and_zigopts_args(ctx))
    return args

def _zig_translate_c_common_args(ctx):
    args = _zig_target_common_args(ctx)
    args.extend(_zig_strip_and_zigopts_args(ctx))
    return args

def _zig_build_env(ctx):
    return {
        "ZIG_GLOBAL_CACHE_DIR": ctx["build_dir"] + "/zig-cache/global",
        "ZIG_LOCAL_CACHE_DIR": ctx["build_dir"] + "/zig-cache/local",
    }

def _zig_host_env(keys):
    env = {}
    for key in keys:
        value = host_env(key)
        if value:
            env[key] = value
    return env

def _zig_run_env(ctx, test_dir = ""):
    env = {}
    if test_dir:
        env["HOME"] = test_dir + "/home"
    for key, value in _zig_attr(ctx, "env", {}).items():
        env[key] = value
    for key, value in _zig_attr(ctx, "test_env", {}).items():
        env[key] = value
    for key, value in _zig_host_env(_zig_attr(ctx, "env_inherit", [])).items():
        env[key] = value
    return env

def _zig_collect_list(deps, prefixed_key, c_key, own):
    out = []
    out.extend(own)
    for dep in deps:
        out.extend(dep.get(prefixed_key) or [])
        if dep.get("c_provider"):
            out.extend(dep.get(c_key) or [])
    return _unique(out)

def _zig_collect_linkopts(deps, own):
    out = []
    out.extend(own)
    for dep in deps:
        out.extend(dep.get("transitive_c_linkopts") or [])
        if dep.get("c_provider"):
            out.extend(dep.get("transitive_linkopts") or [])
    return out

def _zig_collect_c_provider(ctx, deps, own_static = [], own_dynamic = [], own_linkopts = []):
    return {
        "headers": _zig_collect_list(deps, "transitive_c_headers", "transitive_headers", []),
        "include_dirs": _zig_collect_list(deps, "transitive_c_include_dirs", "transitive_include_dirs", []),
        "quote_include_dirs": _zig_collect_list(deps, "transitive_c_quote_include_dirs", "transitive_quote_include_dirs", []),
        "system_include_dirs": _zig_collect_list(deps, "transitive_c_system_include_dirs", "transitive_system_include_dirs", []),
        "framework_include_dirs": _zig_collect_list(deps, "transitive_c_framework_include_dirs", "transitive_framework_include_dirs", []),
        "defines": _zig_collect_list(deps, "transitive_c_defines", "transitive_defines", []),
        "static_libraries": _zig_collect_list(deps, "transitive_c_static_libraries", "transitive_static_libraries", own_static),
        "dynamic_libraries": _zig_collect_list(deps, "transitive_c_dynamic_libraries", "transitive_dynamic_libraries", own_dynamic),
        "linkopts": _zig_collect_linkopts(deps, own_linkopts),
        "data": _zig_collect_list(deps, "transitive_c_data", "transitive_data", []),
    }

def _zig_c_provider_has_headers(dep):
    return len(dep.get("transitive_headers") or dep.get("headers") or []) > 0

def _zig_uses_c_module(deps, direct_c_deps):
    if len(direct_c_deps) > 0:
        for dep in direct_c_deps:
            if _zig_c_provider_has_headers(dep):
                return True
    for dep in deps:
        if dep.get("uses_c_module"):
            return True
    return False

def _zig_c_compile_args(cinfo):
    args = []
    for define in cinfo.get("defines") or []:
        args.append("-D" + define)
    for include in cinfo.get("include_dirs") or []:
        args.extend(["-I", include])
    for include in cinfo.get("quote_include_dirs") or []:
        args.extend(["-I", include])
    for include in cinfo.get("system_include_dirs") or []:
        args.extend(["-isystem", include])
    for include in cinfo.get("framework_include_dirs") or []:
        args.extend(["-F", include])
    return args

def _zig_translate_c_compile_args(cinfo):
    args = _zig_c_compile_args(cinfo)
    args.extend(["-I", "."])
    return args

def _zig_c_link_args(cinfo):
    args = []
    args.extend(cinfo.get("linkopts") or [])
    for path in cinfo.get("static_libraries") or []:
        if path:
            args.append(path)
    for path in cinfo.get("dynamic_libraries") or []:
        if path:
            args.append(path)
    return args

def _zig_c_inputs(cinfo):
    inputs = []
    inputs.extend(cinfo.get("headers") or [])
    inputs.extend(cinfo.get("static_libraries") or [])
    inputs.extend(cinfo.get("dynamic_libraries") or [])
    inputs.extend(cinfo.get("data") or [])
    return _unique(inputs)

def _zig_csrc_args(ctx):
    csrcs = _zig_csrcs(ctx)
    copts = _zig_attr(ctx, "copts", [])
    if len(csrcs) == 0 and len(copts) == 0:
        return []
    args = ["-cflags"]
    args.extend(copts)
    args.append("--")
    args.extend(csrcs)
    return args

def _zig_linker_script(ctx):
    linker_script = _zig_attr(ctx, "linker_script", "")
    if not linker_script:
        return ("", [])
    path = _package_relative(ctx, linker_script)
    return (path, ["-T", path])

def _zig_aux_outputs(ctx):
    outputs = []
    args = []
    if _zig_attr(ctx, "emit_asm", False):
        output = declare_output(ctx["label"]["name"] + ".s")
        outputs.append(output)
        args.append("-femit-asm=" + output)
    if _zig_attr(ctx, "emit_llvm_ir", False):
        output = declare_output(ctx["label"]["name"] + ".ll")
        outputs.append(output)
        args.append("-femit-llvm-ir=" + output)
    if _zig_attr(ctx, "emit_llvm_bc", False):
        output = declare_output(ctx["label"]["name"] + ".bc")
        outputs.append(output)
        args.append("-femit-llvm-bc=" + output)
    return (outputs, args)

def _zig_build_root(ctx, default_main):
    direct_zig_deps = _zig_zig_module_deps(ctx["deps"])
    if _zig_attr(ctx, "main", ""):
        main = _zig_main(ctx, default_main)
        direct_c_deps = _zig_direct_c_deps(ctx["deps"])
        uses_c_module = _zig_uses_c_module(ctx["deps"], direct_c_deps)
        return {
            "main": main,
            "root_context": _zig_module_context(ctx, main, ctx["deps"], uses_c_module),
            "dep_contexts": _zig_transitive_module_contexts(ctx["deps"]),
            "direct_sources": _zig_source_inputs(ctx, main),
            "uses_c_module": uses_c_module,
        }
    if len(direct_zig_deps) == 1 and default_main == "":
        dep = direct_zig_deps[0]
        if len(ctx["srcs"]) > 0:
            fail(ctx["label"]["id"] + ": `srcs` cannot be set when `main` is omitted; sources come from the single Zig module dependency")
        if len(_zig_attr(ctx, "extra_srcs", [])) > 0:
            fail(ctx["label"]["id"] + ": `extra_srcs` cannot be set when `main` is omitted; sources come from the single Zig module dependency")
        if len(_zig_attr(ctx, "csrcs", [])) > 0:
            fail(ctx["label"]["id"] + ": `csrcs` cannot be set when `main` is omitted; C sources come from the single Zig module dependency")
        if len(_zig_attr(ctx, "import_names", {})) > 0:
            fail(ctx["label"]["id"] + ": `import_names` has no effect when a single dependency is used as the root module; set `main` to define a module")
        return {
            "main": dep.get("main") or "",
            "root_context": dep.get("module_context"),
            "dep_contexts": dep.get("transitive_module_contexts") or [],
            "direct_sources": [],
            "uses_c_module": dep.get("uses_c_module") or False,
        }
    main = _zig_main(ctx, default_main)
    if not main:
        fail(ctx["label"]["id"] + ": set `main` or provide exactly one Zig module dependency")
    direct_c_deps = _zig_direct_c_deps(ctx["deps"])
    uses_c_module = _zig_uses_c_module(ctx["deps"], direct_c_deps)
    return {
        "main": main,
        "root_context": _zig_module_context(ctx, main, ctx["deps"], uses_c_module),
        "dep_contexts": _zig_transitive_module_contexts(ctx["deps"]),
        "direct_sources": _zig_source_inputs(ctx, main),
        "uses_c_module": uses_c_module,
    }

def _zig_validate_static_emit(ctx):
    if _zig_attr(ctx, "emit_bin", True):
        return
    if _zig_attr(ctx, "emit_asm", False):
        return
    if _zig_attr(ctx, "emit_llvm_ir", False):
        return
    if _zig_attr(ctx, "emit_llvm_bc", False):
        return
    fail(ctx["label"]["id"] + ": at least one emitted output must be enabled")

def _zig_translate_header_source(headers):
    lines = []
    for header in headers:
        lines.append("#include \"" + header + "\"")
    lines.append("")
    return "\n".join(lines)

def _zig_translate_c_script(zig, args, output):
    quoted = [_shell_quote(zig)]
    for arg in args:
        quoted.append(_shell_quote(arg))
    return " ".join(quoted) + " > " + _shell_quote(output)

def _zig_powershell_argv(script):
    return [host_which("powershell"), "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script]

def _zig_translate_c_windows_script(zig, args, output):
    quoted = [_powershell_quote(zig)]
    for arg in args:
        quoted.append(_powershell_quote(arg))
    return """$ErrorActionPreference = 'Stop'
& {command} > {output}
if ($global:LASTEXITCODE -ne $null) {{
  exit $global:LASTEXITCODE
}}
if (-not $?) {{
  exit 1
}}
""".format(command = " ".join(quoted), output = _powershell_quote(output))

def _zig_translate_c_action(ctx, cinfo, name_prefix):
    headers = cinfo.get("headers") or []
    if len(headers) == 0:
        return None
    header = declare_output(name_prefix + "_c.h")
    output = declare_output(name_prefix + "_c.zig")
    write_path(header, _zig_translate_header_source(headers))
    standalone = _zig_tristate_attr(ctx, "use_standalone_translate_c") == 1 or _zig_attr(ctx, "translate_c", "")
    if standalone:
        translate_c = _zig_attr(ctx, "translate_c", "") or host_which("translate-c")
        translate_c_identity = _zig_attr(ctx, "translate_c_identity", "")
        args = [header]
        args.extend(_zig_translate_c_common_args(ctx))
        args.extend(_zig_translate_c_compile_args(cinfo))
        args.extend(["-o", output, "--emulate=clang"])
        run_action(
            argv = [translate_c] + args,
            inputs = _unique([header] + _zig_c_inputs(cinfo)),
            outputs = [output],
            env = _zig_build_env(ctx),
            toolchain_identity = "once.zig.translate_c.v1\x00" + translate_c + "\x00" + translate_c_identity,
            identifier = ctx["label"]["id"] + ":zig-translate-c:" + name_prefix,
        )
    else:
        zig, identity = _zig_toolchain(ctx)
        args = ["translate-c"]
        args.extend(_zig_translate_c_common_args(ctx))
        args.extend(_zig_translate_c_compile_args(cinfo))
        args.append("-lc")
        args.append(header)
        if host_os() == "windows":
            argv = _zig_powershell_argv(_zig_translate_c_windows_script(zig, args, output))
        else:
            argv = [host_which("sh"), "-c", _zig_translate_c_script(zig, args, output)]
        run_action(
            argv = argv,
            inputs = _unique([header] + _zig_c_inputs(cinfo)),
            outputs = [output],
            env = _zig_build_env(ctx),
            toolchain_identity = identity + "\x00translate-c",
            identifier = ctx["label"]["id"] + ":zig-translate-c:" + name_prefix,
        )
    return {
        "import_name": "c",
        "canonical_name": "c",
        "main": output,
        "deps": [],
        "zigopts": [],
    }

def _zig_compile_args(ctx, command, output, root, c_module_context, cinfo, emit_bin, aux_args):
    args = []
    if command == "test":
        args.append("--test-no-exec")
    args.extend(_zig_common_args(ctx))
    args.extend(_zig_c_compile_args(cinfo))
    args.extend(_zig_module_args(root["root_context"], root["dep_contexts"], c_module_context))
    test_runner = _zig_attr(ctx, "test_runner", "")
    if command == "test" and test_runner:
        args.extend(["--test-runner", _package_relative(ctx, test_runner)])
    args.extend(_zig_csrc_args(ctx))
    linker_script, linker_args = _zig_linker_script(ctx)
    args.extend(linker_args)
    if emit_bin and output:
        args.append("-femit-bin=" + output)
    elif not emit_bin:
        args.append("-fno-emit-bin")
    args.extend(aux_args)
    args.extend(_zig_attr(ctx, "linkopts", []))
    args.extend(_zig_c_link_args(cinfo))
    return args

def _zig_docs_args(ctx, command, root, c_module_context, cinfo, docs_output):
    args = []
    if command == "test":
        args.append("--test-no-exec")
    args.extend(_zig_common_args(ctx))
    args.extend(_zig_c_compile_args(cinfo))
    args.extend(_zig_module_args(root["root_context"], root["dep_contexts"], c_module_context))
    args.extend(_zig_csrc_args(ctx))
    linker_script, linker_args = _zig_linker_script(ctx)
    args.extend(linker_args)
    args.extend(_zig_attr(ctx, "linkopts", []))
    args.extend(_zig_c_link_args(cinfo))
    args.extend(["-femit-docs=" + docs_output, "-fno-emit-bin", "-fno-emit-implib"])
    return args

def _zig_compile_inputs(ctx, root, c_module_context, cinfo):
    inputs = []
    inputs.extend(root["direct_sources"])
    inputs.extend(_zig_collect_dep_sources(ctx["deps"]))
    inputs.extend(_zig_csrcs(ctx))
    inputs.extend(_zig_c_inputs(cinfo))
    test_runner = _zig_attr(ctx, "test_runner", "")
    if test_runner:
        inputs.append(_package_relative(ctx, test_runner))
    linker_script, linker_args = _zig_linker_script(ctx)
    if linker_script:
        inputs.append(linker_script)
    if c_module_context != None:
        inputs.append(c_module_context["main"])
    return _unique(inputs)

def _zig_docs_action(ctx, command, root, c_module_context, cinfo, inputs, zig, identity):
    docs_output = declare_output(_zig_docs_name(ctx))
    docs_command = command
    command_prefix = []
    if command == "build-lib-dynamic":
        docs_command = "build-lib"
        command_prefix = ["-dynamic"]
    argv = [zig] + [docs_command] + command_prefix + _zig_docs_args(ctx, docs_command, root, c_module_context, cinfo, docs_output)
    run_action(
        argv = argv,
        inputs = _unique(inputs + _zig_extra_docs(ctx)),
        outputs = [docs_output],
        env = _zig_build_env(ctx),
        toolchain_identity = identity + "\x00zig-docs",
        identifier = ctx["label"]["id"] + ":zig-docs",
    )
    return docs_output

def _zig_compile(ctx, kind, default_main, command, provider_kind):
    if kind == "static":
        _zig_validate_static_emit(ctx)
    root = _zig_build_root(ctx, default_main)
    cinfo = _zig_collect_c_provider(ctx, ctx["deps"])
    c_module_context = _zig_translate_c_action(ctx, cinfo, "c") if root["uses_c_module"] else None
    primary_output = ""
    emit_bin = True
    if kind == "static":
        emit_bin = _zig_attr(ctx, "emit_bin", True)
    if emit_bin:
        primary_output = declare_output(_zig_output_name(ctx, kind))
    aux_outputs, aux_args = _zig_aux_outputs(ctx)
    zig, identity = _zig_toolchain(ctx)
    inputs = _zig_compile_inputs(ctx, root, c_module_context, cinfo)
    compile_command = command
    command_prefix = []
    if command == "build-lib-dynamic":
        compile_command = "build-lib"
        command_prefix = ["-dynamic"]
    argv = [zig] + [compile_command] + command_prefix + _zig_compile_args(ctx, compile_command, primary_output, root, c_module_context, cinfo, emit_bin, aux_args)
    if kind == "shared" and primary_output:
        argv.append("-fsoname=" + _basename(primary_output))
    run_action(
        argv = argv,
        inputs = inputs,
        outputs = ([primary_output] if primary_output else []) + aux_outputs,
        env = _zig_build_env(ctx),
        toolchain_identity = identity,
        identifier = ctx["label"]["id"] + ":zig-" + command,
    )
    provider = _zig_provider(ctx, provider_kind, root, primary_output, cinfo, kind)
    if command != "test":
        provider["zig_docs"] = _zig_docs_action(ctx, command, root, c_module_context, cinfo, inputs, zig, identity)
    if kind == "binary":
        provider["binary"] = primary_output
    if kind == "test":
        provider["test_binary"] = primary_output
    if kind == "static":
        provider["static_library"] = primary_output
        provider["archive"] = primary_output
    if kind == "shared":
        provider["shared_library"] = primary_output
        provider["dylib"] = primary_output
    return provider

def _zig_provider(ctx, provider_kind, root, output, cinfo, output_kind):
    deps = ctx["deps"]
    data = _zig_data_inputs(ctx)
    direct_sources = root["direct_sources"]
    transitive_sources = _unique(direct_sources + _zig_collect_dep_sources(deps))
    transitive_data = _unique(data + _zig_collect_dep_data(deps) + (cinfo.get("data") or []))
    is_module = provider_kind == "zig_library" or provider_kind == "zig_c_library"
    own_static = [output] if output and output_kind == "static" else []
    own_dynamic = [output] if output and output_kind == "shared" else []
    c_provider = _zig_collect_c_provider(ctx, deps, own_static, own_dynamic, _zig_attr(ctx, "linkopts", []))
    android_abi = _zig_attr(ctx, "android_abi", "")
    android_native_libraries = []
    if android_abi:
        for path in own_dynamic:
            android_native_libraries.append({"abi": android_abi, "path": path})
    transitive_android_native_libraries = _zig_collect_android_native_libraries(deps, android_native_libraries)
    provider = {
        "label_id": ctx["label"]["id"],
        "target_kind": provider_kind,
        "import_name": _zig_import_name(ctx),
        "canonical_name": _zig_canonical_name(ctx),
        "main": root["main"],
        "module_context": root["root_context"],
        "transitive_module_contexts": root["dep_contexts"],
        "uses_c_module": root["uses_c_module"],
        "transitive_sources": transitive_sources,
        "transitive_data": transitive_data,
        "affected_inputs": _unique(transitive_sources + transitive_data + _zig_c_inputs(cinfo)),
        "default_output": output,
        "c_provider": output_kind == "static" or output_kind == "shared",
        "static_library": own_static[0] if len(own_static) > 0 else "",
        "static_libraries": own_static,
        "dynamic_libraries": own_dynamic,
        "objects": [],
        "headers": [],
        "include_dirs": [],
        "quote_include_dirs": [],
        "system_include_dirs": [],
        "framework_include_dirs": [],
        "defines": [],
        "copts": [],
        "linkopts": _zig_attr(ctx, "linkopts", []),
        "archive": own_static[0] if len(own_static) > 0 else "",
        "transitive_headers": c_provider["headers"],
        "transitive_include_dirs": c_provider["include_dirs"],
        "transitive_quote_include_dirs": c_provider["quote_include_dirs"],
        "transitive_system_include_dirs": c_provider["system_include_dirs"],
        "transitive_framework_include_dirs": c_provider["framework_include_dirs"],
        "transitive_defines": c_provider["defines"],
        "transitive_static_libraries": c_provider["static_libraries"],
        "transitive_dynamic_libraries": c_provider["dynamic_libraries"],
        "transitive_linkopts": c_provider["linkopts"],
        "transitive_archives": c_provider["static_libraries"],
        "android_abi": android_abi,
        "android_native_libraries": android_native_libraries,
        "transitive_android_native_libraries": transitive_android_native_libraries,
        "transitive_c_headers": c_provider["headers"],
        "transitive_c_include_dirs": c_provider["include_dirs"],
        "transitive_c_quote_include_dirs": c_provider["quote_include_dirs"],
        "transitive_c_system_include_dirs": c_provider["system_include_dirs"],
        "transitive_c_framework_include_dirs": c_provider["framework_include_dirs"],
        "transitive_c_defines": c_provider["defines"],
        "transitive_c_static_libraries": c_provider["static_libraries"],
        "transitive_c_dynamic_libraries": c_provider["dynamic_libraries"],
        "transitive_c_linkopts": c_provider["linkopts"],
        "transitive_c_data": c_provider["data"],
    }
    if is_module:
        provider["zig_module"] = True
    return provider

def _zig_library_impl(ctx):
    main = _zig_main(ctx, "")
    if not main:
        fail(ctx["label"]["id"] + ": zig_library requires `main`")
    direct_c_deps = _zig_direct_c_deps(ctx["deps"])
    uses_c_module = _zig_uses_c_module(ctx["deps"], direct_c_deps)
    root = {
        "main": main,
        "root_context": _zig_module_context(ctx, main, ctx["deps"], uses_c_module),
        "dep_contexts": _zig_transitive_module_contexts(ctx["deps"]),
        "direct_sources": _zig_source_inputs(ctx, main),
        "uses_c_module": uses_c_module,
    }
    cinfo = _zig_collect_c_provider(ctx, ctx["deps"])
    return _zig_provider(ctx, "zig_library", root, "", cinfo, "module")

def _zig_binary_run_script(ctx, binary, marker, log):
    args = [_shell_quote(arg) for arg in _zig_attr(ctx, "args", [])]
    return """set +e
{binary} {args} > {log} 2>&1
code=$?
printf '{{"schema":"once.run_result.v1","target":{target},"exit_code":%s}}\\n' "$code" > {marker}
exit "$code"
""".format(
        binary = _shell_quote("./" + binary),
        args = " ".join(args),
        log = _shell_quote(log),
        target = _zig_json_string(ctx["label"]["id"]),
        marker = _shell_quote(marker),
    )

def _zig_binary_run_windows_script(ctx, binary, marker, log):
    args = [_powershell_quote(arg) for arg in _zig_attr(ctx, "args", [])]
    return """$ErrorActionPreference = 'Continue'
$encoding = New-Object System.Text.UTF8Encoding -ArgumentList $false
& {binary} {args} > {log} 2>&1
$code = $global:LASTEXITCODE
if ($null -eq $code) {{
  if ($?) {{
    $code = 0
  }} else {{
    $code = 1
  }}
}}
$result = [ordered]@{{
  schema = 'once.run_result.v1'
  target = {target}
  exit_code = $code
}}
$json = $result | ConvertTo-Json -Compress
[System.IO.File]::WriteAllText({marker}, $json + [System.Environment]::NewLine, $encoding)
exit $code
""".format(
        binary = _powershell_quote("./" + binary),
        args = " ".join(args),
        log = _powershell_quote(log),
        target = _powershell_quote(ctx["label"]["id"]),
        marker = _powershell_quote(marker),
    )

def _zig_binary_impl(ctx):
    if ctx["capability"] == "run":
        binary = ctx["build_dir"] + "/" + _zig_output_name(ctx, "binary")
        run_dir = ctx["build_dir"] + "/run"
        marker = run_dir + "/run.json"
        log = run_dir + "/stdout.log"
        if host_os() == "windows":
            argv = _zig_powershell_argv(_zig_binary_run_windows_script(ctx, binary, marker, log))
        else:
            argv = [host_which("sh"), "-c", _zig_binary_run_script(ctx, binary, marker, log)]
        prepare_path(run_dir, kind = "directory", identifier = ctx["label"]["id"] + ":zig-run-prepare")
        run_action(
            argv = argv,
            inputs = _unique([binary] + _zig_data_inputs(ctx) + _zig_collect_dep_data(ctx["deps"])),
            outputs = [run_dir, marker, log],
            env = _zig_run_env(ctx),
            toolchain_identity = "once.zig.run.v1\x00" + binary,
            identifier = ctx["label"]["id"] + ":zig-run",
            cacheable = False,
        )
        root = _zig_build_root(ctx, "")
        return _zig_provider(ctx, "zig_binary", root, binary, _zig_collect_c_provider(ctx, ctx["deps"]), "binary")
    return _zig_compile(ctx, "binary", "", "build-exe", "zig_binary")

def _zig_static_library_impl(ctx):
    return _zig_compile(ctx, "static", "", "build-lib", "zig_static_library")

def _zig_shared_library_impl(ctx):
    return _zig_compile(ctx, "shared", "", "build-lib-dynamic", "zig_shared_library")

def _zig_configure_impl(ctx):
    output = _zig_attr(ctx, "output", "static")
    if output == "static":
        return _zig_compile(ctx, "static", "", "build-lib", "zig_static_library")
    if output == "shared":
        return _zig_compile(ctx, "shared", "", "build-lib-dynamic", "zig_shared_library")
    fail(ctx["label"]["id"] + ": `output` must be `static` or `shared`")

def _zig_configure_binary_impl(ctx):
    return _zig_binary_impl(ctx)

def _zig_configure_test_impl(ctx):
    return _zig_test_impl(ctx)

def _zig_json_string(value):
    out = ["\""]
    for ch in value.elems():
        if ch == "\"":
            out.append("\\\"")
        elif ch == "\\":
            out.append("\\\\")
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\r":
            out.append("\\r")
        elif ch == "\t":
            out.append("\\t")
        else:
            out.append(ch)
    out.append("\"")
    return "".join(out)

def _zig_test_info(ctx, test_binary, results, log, native_results, test_dir):
    labels = _zig_attr(ctx, "labels", [])
    timeout_ms = _zig_attr(ctx, "timeout_ms", None)
    return {
        "schema": "once.test_info.v1",
        "target": ctx["label"]["id"],
        "runner": {
            "type": "zig_test",
            "display_name": "Zig test",
            "metadata": {},
        },
        "command": {
            "argv": [test_binary] + _zig_attr(ctx, "args", []),
            "env": _zig_run_env(ctx, test_dir),
            "cwd": ".",
        },
        "outputs": {
            "results": results,
            "logs": [log],
            "native_results": [native_results],
            "coverage": [],
        },
        "listing": {
            "supported": False,
            "strategy": "suite",
        },
        "filtering": {
            "case_filtering": "runner_args",
        },
        "sharding": {
            "supported": False,
        },
        "retries": {
            "supported": False,
            "default_attempts": 1,
        },
        "execution": {
            "cacheable": True,
            "timeout_ms": timeout_ms,
            "run_from_workspace_root": True,
        },
        "labels": labels,
        "metadata": {},
    }

def _zig_test_script(ctx, test_binary, results, log, native_results):
    args = [_shell_quote(arg) for arg in _zig_attr(ctx, "args", [])]
    target = _zig_json_string(ctx["label"]["id"])
    case_id = _zig_json_string(ctx["label"]["id"] + "::suite")
    case_name = _zig_json_string(ctx["label"]["name"])
    suite = _zig_json_string(ctx["label"]["id"])
    log_json = _zig_json_string(log)
    native_json = _zig_json_string(native_results)
    return """set +e
{binary} {args} > {log} 2>&1
code=$?
printf '$ %s\\nlog: %s\\nexit: %s\\n' {binary} {log} "$code" > {native_results}
if [ "$code" -eq 0 ]; then
  status=passed
  passed=1
  failed=0
else
  status=failed
  passed=0
  failed=1
fi
printf '{{"schema":"once.test_results.v1","target":{target},"runner":{{"type":"zig_test","metadata":{{}}}},"status":"%s","summary":{{"total":1,"passed":%s,"failed":%s,"skipped":0,"flaky":0}},"cases":[{{"id":{case_id},"name":{case_name},"suite":{suite},"status":"%s","attempts":[{{"status":"%s"}}],"runner_metadata":{{}}}}],"artifacts":{{"logs":[{log_json}],"native_results":[{native_json}]}}}}\\n' "$status" "$passed" "$failed" "$status" "$status" > {results}
exit "$code"
""".format(
        binary = _shell_quote("./" + test_binary),
        args = " ".join(args),
        log = _shell_quote(log),
        native_results = _shell_quote(native_results),
        target = target,
        case_id = case_id,
        case_name = case_name,
        suite = suite,
        log_json = log_json,
        native_json = native_json,
        results = _shell_quote(results),
    )

def _zig_test_windows_script(ctx, test_binary, results, log, native_results):
    args = [_powershell_quote(arg) for arg in _zig_attr(ctx, "args", [])]
    return """$ErrorActionPreference = 'Continue'
$encoding = New-Object System.Text.UTF8Encoding -ArgumentList $false
& {binary} {args} > {log} 2>&1
$code = $global:LASTEXITCODE
if ($null -eq $code) {{
  if ($?) {{
    $code = 0
  }} else {{
    $code = 1
  }}
}}
if ($code -eq 0) {{
  $status = 'passed'
  $passed = 1
  $failed = 0
}} else {{
  $status = 'failed'
  $passed = 0
  $failed = 1
}}
$native = '$ ' + {binary_display} + [System.Environment]::NewLine + 'log: ' + {log_display} + [System.Environment]::NewLine + 'exit: ' + [string]$code + [System.Environment]::NewLine
[System.IO.File]::WriteAllText({native_results}, $native, $encoding)
$result = [ordered]@{{
  schema = 'once.test_results.v1'
  target = {target}
  runner = [ordered]@{{
    type = 'zig_test'
    metadata = [ordered]@{{}}
  }}
  status = $status
  summary = [ordered]@{{
    total = 1
    passed = $passed
    failed = $failed
    skipped = 0
    flaky = 0
  }}
  cases = @([ordered]@{{
    id = {case_id}
    name = {case_name}
    suite = {suite}
    status = $status
    attempts = @([ordered]@{{
      status = $status
    }})
    runner_metadata = [ordered]@{{}}
  }})
  artifacts = [ordered]@{{
    logs = @({log_json})
    native_results = @({native_json})
  }}
}}
$json = $result | ConvertTo-Json -Depth 10 -Compress
[System.IO.File]::WriteAllText({results}, $json + [System.Environment]::NewLine, $encoding)
exit $code
""".format(
        binary = _powershell_quote("./" + test_binary),
        args = " ".join(args),
        log = _powershell_quote(log),
        binary_display = _powershell_quote("./" + test_binary),
        log_display = _powershell_quote(log),
        native_results = _powershell_quote(native_results),
        target = _powershell_quote(ctx["label"]["id"]),
        case_id = _powershell_quote(ctx["label"]["id"] + "::suite"),
        case_name = _powershell_quote(ctx["label"]["name"]),
        suite = _powershell_quote(ctx["label"]["id"]),
        log_json = _powershell_quote(log),
        native_json = _powershell_quote(native_results),
        results = _powershell_quote(results),
    )

def _zig_test_impl(ctx):
    test_dir = ctx["build_dir"] + "/test"
    results = test_dir + "/test_results.json"
    log = test_dir + "/zig-test.log"
    native_results = test_dir + "/native_results.txt"
    if ctx["capability"] == "metadata":
        test_binary = ctx["build_dir"] + "/" + _zig_output_name(ctx, "test")
        provider = {
            "label_id": ctx["label"]["id"],
            "target_kind": "zig_test",
            "default_output": test_binary,
            "test_binary": test_binary,
        }
        provider["test_info"] = _zig_test_info(ctx, provider["test_binary"], results, log, native_results, test_dir)
        return provider

    target = _zig_attr(ctx, "target", "")
    if ctx["capability"] == "test" and target:
        fail(ctx["label"]["id"] + ": zig_test execution supports the host target only; remove `target` or run the cross-compiled test binary with a platform runner")

    provider = _zig_compile(ctx, "test", "", "test", "zig_test")
    provider["test_info"] = _zig_test_info(ctx, provider["test_binary"], results, log, native_results, test_dir)
    if ctx["capability"] != "test":
        return provider

    if host_os() == "windows":
        argv = _zig_powershell_argv(_zig_test_windows_script(ctx, provider["test_binary"], results, log, native_results))
    else:
        argv = [host_which("sh"), "-c", _zig_test_script(ctx, provider["test_binary"], results, log, native_results)]
    prepare_path(test_dir, kind = "directory", identifier = ctx["label"]["id"] + ":zig-test-prepare")
    run_action(
        argv = argv,
        inputs = _unique([provider["test_binary"]] + _zig_data_inputs(ctx) + _zig_collect_dep_data(ctx["deps"])),
        outputs = [test_dir, results, log, native_results],
        env = _zig_run_env(ctx, test_dir),
        toolchain_identity = "once.zig_test.run.v1\x00" + provider["test_binary"],
        identifier = ctx["label"]["id"] + ":zig-test-run",
    )
    return provider

def _zig_c_library_impl(ctx):
    cinfo = _zig_collect_c_provider(ctx, ctx["deps"])
    c_module_context = _zig_translate_c_action(ctx, cinfo, ctx["label"]["name"])
    if c_module_context == None:
        fail(ctx["label"]["id"] + ": zig_c_library requires at least one C header from deps")
    root = {
        "main": c_module_context["main"],
        "root_context": {
            "import_name": _zig_import_name(ctx),
            "canonical_name": _zig_canonical_name(ctx),
            "main": c_module_context["main"],
            "deps": [],
            "zigopts": _zig_attr(ctx, "zigopts", []),
        },
        "dep_contexts": [],
        "direct_sources": [c_module_context["main"]],
        "uses_c_module": False,
    }
    return _zig_provider(ctx, "zig_c_library", root, c_module_context["main"], cinfo, "module")

_ZIG_MODULE_ATTRS = [
    attr("main", "string", required = True, docs = "Package-relative root Zig module.", configurable = False),
    attr("import_name", "string", docs = "Name used by downstream modules when importing this target. Defaults to the target name.", configurable = False),
    attr("import_names", "map<string, string>", default = "{}", docs = "Map a dependency label, dependency short name, or dependency import name to the local import name used by this target.", configurable = False),
    attr("extra_srcs", "list<string>", default = "[]", docs = "Additional package-relative source or data globs included in dependent compile input digests.", configurable = False),
    attr("data", "list<string>", default = "[]", docs = "Runtime data globs included in provider metadata and dependent run inputs.", configurable = False),
    attr("zigopts", "list<string>", default = "[]", docs = "Additional flags added to this Zig module specification.", configurable = False),
]

_ZIG_BUILD_ATTRS = [
    attr("zig", "string", docs = "Path to the Zig compiler. Defaults to resolving `zig` on PATH.", configurable = False),
    attr("zig_version", "string", docs = "Expected `zig version` output. Analysis fails if the selected compiler reports a different version.", configurable = False),
    attr("bootstrapped", "int", default = "-1", docs = "Compatibility selector for bootstrapped Zig toolchains. Use `zig` to select the compiler path; accepted values are -1, 0, and 1.", configurable = False),
    attr("main", "string", docs = "Package-relative root Zig module. If omitted, deps must contain exactly one Zig module dependency and sources come from that module.", configurable = False),
    attr("import_name", "string", docs = "Name used by downstream modules when importing this target. Defaults to the target name.", configurable = False),
    attr("import_names", "map<string, string>", default = "{}", docs = "Map a dependency label, dependency short name, or dependency import name to the local import name used by this target.", configurable = False),
    attr("extra_srcs", "list<string>", default = "[]", docs = "Additional package-relative source or data globs included in the compile action input digest.", configurable = False),
    attr("extra_docs", "list<string>", default = "[]", docs = "Additional package-relative files included in the documentation action input digest.", configurable = False),
    attr("data", "list<string>", default = "[]", docs = "Runtime data globs included in run or test inputs.", configurable = False),
    attr("target", "string", docs = "Zig target triple passed to `zig -target`. Defaults to the host target.", configurable = False),
    attr("cpu", "string", docs = "Optional Zig processor value passed as `-mcpu=...`.", configurable = False),
    attr("optimize", "string", docs = "Optional Zig optimization mode passed through `-O`, such as `Debug`, `ReleaseSafe`, `ReleaseFast`, or `ReleaseSmall`.", configurable = False),
    attr("mode", "string", docs = "Zig build mode. Accepted values are `auto`, `debug`, `release_safe`, `release_small`, and `release_fast`; maps to `-O`.", configurable = False),
    attr("host_mode", "string", docs = "Once-compatible alias for `mode` when porting configured host targets. Set either `mode` or `host_mode`.", configurable = False),
    attr("threaded", "string", docs = "`multi` passes `-fno-single-threaded`; `single` passes `-fsingle-threaded`.", configurable = False),
    attr("host_threaded", "string", docs = "Once-compatible alias for `threaded` when porting configured host targets. Set either `threaded` or `host_threaded`.", configurable = False),
    attr("strip", "bool", default = "false", docs = "Alias for strip_debug_symbols.", configurable = False),
    attr("strip_debug_symbols", "bool", default = "false", docs = "Pass `-fstrip` to remove debug symbols.", configurable = False),
    attr("compiler_runtime", "string", default = "default", docs = "`default`, `include`, or `exclude`; controls `-fcompiler-rt` and `-fno-compiler-rt`.", configurable = False),
    attr("zigopt", "list<string>", default = "[]", docs = "Global Zig flags prepended before module-specific `zigopts`.", configurable = False),
    attr("host_zigopt", "list<string>", default = "[]", docs = "Once-compatible alias for `zigopt` when porting configured host targets. Set either `zigopt` or `host_zigopt`.", configurable = False),
    attr("zigopts", "list<string>", default = "[]", docs = "Additional flags added to this Zig module specification.", configurable = False),
    attr("linkopts", "list<string>", default = "[]", docs = "Additional linker flags appended after Once-managed flags.", configurable = False),
    attr("use_cc_common_link", "int", default = "-1", docs = "Compatibility setting for rules that choose a C provider linker path. Once always consumes C providers through its native provider model. Accepted values are -1, 0, and 1.", configurable = False),
    attr("host_use_cc_common_link", "int", default = "-1", docs = "Host-configuration compatibility setting for C provider linking. Accepted values are -1, 0, and 1.", configurable = False),
    attr("use_standalone_translate_c", "int", default = "-1", docs = "Set to 1 to use a standalone `translate-c` executable instead of `zig translate-c`; set `translate_c` to choose the executable path.", configurable = False),
    attr("translate_c", "string", docs = "Standalone `translate-c` executable used when `use_standalone_translate_c` is 1, or whenever this path is set.", configurable = False),
    attr("translate_c_identity", "string", docs = "Optional stable identity for the standalone `translate-c` executable, such as a version or content digest, folded into translate action cache keys.", configurable = False),
    attr("csrcs", "list<string>", default = "[]", docs = "C source files passed to Zig through `-cflags ... --`.", configurable = False),
    attr("copts", "list<string>", default = "[]", docs = "C compiler flags passed after `-cflags` for csrcs.", configurable = False),
    attr("linker_script", "string", docs = "Package-relative linker script passed as `-T`.", configurable = False),
    attr("emit_asm", "bool", default = "false", docs = "Emit assembly output in addition to the primary output.", configurable = False),
    attr("emit_llvm_ir", "bool", default = "false", docs = "Emit compiler intermediate representation output.", configurable = False),
    attr("emit_llvm_bc", "bool", default = "false", docs = "Emit compiler bitcode output.", configurable = False),
    attr("output_name", "string", docs = "Output file name without platform extension. Defaults to the target name.", configurable = False),
]

_ZIG_DEP_EDGE = [dep("deps", ["zig_module", "c_provider"], "Zig modules and C provider dependencies consumed by this target.")]

zig_library = target_kind(
    docs = "Zig module provider compiled at the use site by downstream Zig build targets.",
    attrs = _ZIG_MODULE_ATTRS,
    deps = _ZIG_DEP_EDGE,
    providers = ["zig_module"],
    capabilities = [capability("build", [])],
    examples = [
        example(
            "zig-library-minimal",
            name = "Minimal Zig library",
            use_when = "Use this when a Zig module should be importable by binaries, static libraries, shared libraries, or tests.",
        ),
    ],
    impl = _zig_library_impl,
)

zig_c_library = target_kind(
    docs = "Zig module generated by translating C provider headers with `zig translate-c`.",
    attrs = [
        attr("zig", "string", docs = "Path to the Zig compiler. Defaults to resolving `zig` on PATH.", configurable = False),
        attr("zig_version", "string", docs = "Expected `zig version` output when using `zig translate-c`.", configurable = False),
        attr("import_name", "string", docs = "Name used by downstream modules when importing this target. Defaults to the target name.", configurable = False),
        attr("zigopts", "list<string>", default = "[]", docs = "Additional flags added to this Zig module specification.", configurable = False),
        attr("data", "list<string>", default = "[]", docs = "Runtime data globs included in provider metadata and dependent run inputs.", configurable = False),
        attr("target", "string", docs = "Zig target triple passed to `zig translate-c`.", configurable = False),
        attr("cpu", "string", docs = "Optional Zig processor value passed as `-mcpu=...`.", configurable = False),
        attr("mode", "string", docs = "Zig build mode passed to translation. Accepted values are `auto`, `debug`, `release_safe`, `release_small`, and `release_fast`.", configurable = False),
        attr("host_mode", "string", docs = "Once-compatible alias for `mode` when porting configured host targets. Set either `mode` or `host_mode`.", configurable = False),
        attr("threaded", "string", docs = "`multi` passes `-fno-single-threaded`; `single` passes `-fsingle-threaded`.", configurable = False),
        attr("host_threaded", "string", docs = "Once-compatible alias for `threaded` when porting configured host targets. Set either `threaded` or `host_threaded`.", configurable = False),
        attr("strip", "bool", default = "false", docs = "Alias for strip_debug_symbols.", configurable = False),
        attr("strip_debug_symbols", "bool", default = "false", docs = "Pass `-fstrip` to translation.", configurable = False),
        attr("zigopt", "list<string>", default = "[]", docs = "Global Zig flags prepended before module-specific `zigopts`.", configurable = False),
        attr("host_zigopt", "list<string>", default = "[]", docs = "Once-compatible alias for `zigopt` when porting configured host targets. Set either `zigopt` or `host_zigopt`.", configurable = False),
        attr("use_cc_common_link", "int", default = "-1", docs = "Compatibility setting for rules that choose a C provider linker path. Once always consumes C providers through its native provider model. Accepted values are -1, 0, and 1.", configurable = False),
        attr("host_use_cc_common_link", "int", default = "-1", docs = "Host-configuration compatibility setting for C provider linking. Accepted values are -1, 0, and 1.", configurable = False),
        attr("use_standalone_translate_c", "int", default = "-1", docs = "Set to 1 to use a standalone `translate-c` executable instead of `zig translate-c`; set `translate_c` to choose the executable path.", configurable = False),
        attr("translate_c", "string", docs = "Standalone `translate-c` executable used when `use_standalone_translate_c` is 1, or whenever this path is set.", configurable = False),
        attr("translate_c_identity", "string", docs = "Optional stable identity for the standalone `translate-c` executable, such as a version or content digest, folded into translate action cache keys.", configurable = False),
        attr("bootstrapped", "int", default = "-1", docs = "Compatibility selector for bootstrapped Zig toolchains. Use `zig` to select the compiler path; accepted values are -1, 0, and 1.", configurable = False),
    ],
    deps = [dep("deps", ["c_provider"], "C provider dependencies whose headers are translated and whose link inputs are propagated.")],
    providers = ["zig_module"],
    capabilities = [capability("build", ["source"])],
    examples = [
        example(
            "zig-c-library-with-c-provider",
            name = "Zig C library from C provider",
            use_when = "Use this when Zig code should import translated C declarations from a C provider dependency.",
        ),
    ],
    impl = _zig_c_library_impl,
)

zig_binary = target_kind(
    docs = "Zig executable compiled with `zig build-exe` from a root module, Zig module deps, and C provider deps.",
    attrs = _ZIG_BUILD_ATTRS + [
        attr("env", "map<string, string>", default = "{}", docs = "Environment variables passed to the binary during `once run`.", configurable = False),
        attr("args", "list<string>", default = "[]", docs = "Arguments passed to the binary during `once run`.", configurable = False),
    ],
    deps = _ZIG_DEP_EDGE,
    providers = ["zig_binary"],
    capabilities = [
        capability("build", ["binary", "asm", "llvm_ir", "llvm_bc", "zig_docs"]),
        capability("run", ["default"], ["binary"]),
    ],
    examples = [
        example(
            "zig-binary-with-library",
            name = "Zig binary with a module dependency",
            use_when = "Use this when a Zig executable imports first-party Zig modules.",
        ),
    ],
    impl = _zig_binary_impl,
)

zig_static_library = target_kind(
    docs = "Zig static library compiled with `zig build-lib`.",
    attrs = _ZIG_BUILD_ATTRS + [
        attr("emit_bin", "bool", default = "true", docs = "Emit the static library. Disable only when another emitted output is enabled.", configurable = False),
    ],
    deps = _ZIG_DEP_EDGE,
    providers = ["zig_static_library", "c_provider", "native_linkable", "apple_linkable"],
    capabilities = [capability("build", ["static_library", "asm", "llvm_ir", "llvm_bc", "zig_docs"])],
    examples = [
        example(
            "zig-static-library-minimal",
            name = "Minimal Zig static library",
            use_when = "Use this when Zig code should produce a static library for native linkers.",
        ),
    ],
    impl = _zig_static_library_impl,
)

zig_shared_library = target_kind(
    docs = "Zig shared library compiled with `zig build-lib -dynamic`.",
    attrs = _ZIG_BUILD_ATTRS + [
        attr("shared_lib_name", "string", docs = "Exact shared library output file name. Defaults to the platform library name for the target.", configurable = False),
        attr("android_abi", "string", docs = "Android Application Binary Interface directory for Android packaging, such as `arm64-v8a`.", configurable = False),
    ],
    deps = _ZIG_DEP_EDGE,
    providers = ["zig_shared_library", "c_provider", "native_linkable", "android_native_library"],
    capabilities = [capability("build", ["shared_library", "asm", "llvm_ir", "llvm_bc", "zig_docs"])],
    examples = [
        example(
            "zig-shared-library-minimal",
            name = "Minimal Zig shared library",
            use_when = "Use this when Zig code should produce a shared library for native linkers or Android packaging.",
        ),
    ],
    impl = _zig_shared_library_impl,
)

zig_configure = target_kind(
    docs = "Once-native configured Zig library target. Configuration is declared on this target instead of using a transition wrapper.",
    attrs = _ZIG_BUILD_ATTRS + [
        attr("output", "string", default = "static", docs = "`static` compiles with `zig build-lib`; `shared` compiles with `zig build-lib -dynamic`.", configurable = False),
        attr("emit_bin", "bool", default = "true", docs = "Emit the static library when `output` is `static`. Disable only when another emitted output is enabled.", configurable = False),
        attr("shared_lib_name", "string", docs = "Exact shared library output file name when `output` is `shared`. Defaults to the platform library name for the target.", configurable = False),
        attr("android_abi", "string", docs = "Android Application Binary Interface directory for shared library packaging, such as `arm64-v8a`.", configurable = False),
    ],
    deps = _ZIG_DEP_EDGE,
    providers = ["zig_static_library", "zig_shared_library", "c_provider", "native_linkable", "apple_linkable", "android_native_library"],
    capabilities = [capability("build", ["static_library", "shared_library", "asm", "llvm_ir", "llvm_bc", "zig_docs"])],
    examples = [
        example(
            "zig-configure-library-minimal",
            name = "Configured Zig library",
            use_when = "Use this when a library target needs Once-native Zig configuration attributes such as mode, threading, or Zig compiler version.",
        ),
    ],
    impl = _zig_configure_impl,
)

zig_configure_binary = target_kind(
    docs = "Once-native configured Zig executable target. Configuration is declared on this target instead of using a transition wrapper.",
    attrs = _ZIG_BUILD_ATTRS + [
        attr("env", "map<string, string>", default = "{}", docs = "Environment variables passed to the binary during `once run`.", configurable = False),
        attr("args", "list<string>", default = "[]", docs = "Arguments passed to the binary during `once run`.", configurable = False),
    ],
    deps = _ZIG_DEP_EDGE,
    providers = ["zig_binary"],
    capabilities = [
        capability("build", ["binary", "asm", "llvm_ir", "llvm_bc", "zig_docs"]),
        capability("run", ["default"], ["binary"]),
    ],
    examples = [
        example(
            "zig-configure-binary-minimal",
            name = "Configured Zig binary",
            use_when = "Use this when an executable target needs Once-native Zig configuration attributes such as mode, threading, or Zig compiler version.",
        ),
    ],
    impl = _zig_configure_binary_impl,
)

zig_test = target_kind(
    docs = "Zig test target compiled with `zig test --test-no-exec` and run through Once's generic test capability.",
    attrs = _ZIG_BUILD_ATTRS + [
        attr("env", "map<string, string>", default = "{}", docs = "Environment variables passed to the Zig test binary.", configurable = False),
        attr("env_inherit", "list<string>", default = "[]", docs = "Environment variable names inherited from the host for test execution.", configurable = False),
        attr("args", "list<string>", default = "[]", docs = "Arguments passed to the compiled Zig test binary.", configurable = False),
        attr("test_env", "map<string, string>", default = "{}", docs = "Once-compatible alias for additional test execution environment variables.", configurable = False),
        attr("test_runner", "string", docs = "Package-relative Zig file passed as `--test-runner`.", configurable = False),
        attr("labels", "list<string>", default = "[]", docs = "Labels exposed through once_test_info for test discovery.", configurable = True),
        attr("timeout_ms", "int", docs = "Optional test timeout in milliseconds.", configurable = False),
    ],
    deps = _ZIG_DEP_EDGE,
    providers = ["zig_test", "once_test_info"],
    capabilities = [
        capability("build", ["binary", "asm", "llvm_ir", "llvm_bc"]),
        capability("test", ["default", "test_results", "logs"], ["binary"]),
    ],
    examples = [
        example(
            "zig-test-minimal",
            name = "Minimal Zig test",
            use_when = "Use this when a Zig module should run tests through Once's generic test capability.",
        ),
    ],
    impl = _zig_test_impl,
)

zig_configure_test = target_kind(
    docs = "Once-native configured Zig test target. Configuration is declared on this target instead of using a transition wrapper.",
    attrs = _ZIG_BUILD_ATTRS + [
        attr("env", "map<string, string>", default = "{}", docs = "Environment variables passed to the Zig test binary.", configurable = False),
        attr("env_inherit", "list<string>", default = "[]", docs = "Environment variable names inherited from the host for test execution.", configurable = False),
        attr("args", "list<string>", default = "[]", docs = "Arguments passed to the compiled Zig test binary.", configurable = False),
        attr("test_env", "map<string, string>", default = "{}", docs = "Once-compatible alias for additional test execution environment variables.", configurable = False),
        attr("test_runner", "string", docs = "Package-relative Zig file passed as `--test-runner`.", configurable = False),
        attr("labels", "list<string>", default = "[]", docs = "Labels exposed through once_test_info for test discovery.", configurable = True),
        attr("timeout_ms", "int", docs = "Optional test timeout in milliseconds.", configurable = False),
    ],
    deps = _ZIG_DEP_EDGE,
    providers = ["zig_test", "once_test_info"],
    capabilities = [
        capability("build", ["binary", "asm", "llvm_ir", "llvm_bc"]),
        capability("test", ["default", "test_results", "logs"], ["binary"]),
    ],
    examples = [
        example(
            "zig-configure-test-minimal",
            name = "Configured Zig test",
            use_when = "Use this when a Zig test needs Once-native Zig configuration attributes such as mode, threading, or Zig compiler version.",
        ),
    ],
    impl = _zig_configure_test_impl,
)
