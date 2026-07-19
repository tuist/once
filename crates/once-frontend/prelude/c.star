def _c_attr(ctx, key, default):
    value = ctx["attr"].get(key)
    if value == None:
        return default
    return value

def _c_parent_dirs(paths):
    out = []
    for path in paths:
        parent = _parent_dir(path)
        if parent and parent not in out:
            out.append(parent)
    return out

def _c_globs(patterns):
    if not patterns:
        return []
    return glob(patterns)

def _c_path_attr(ctx, key):
    return [_package_relative(ctx, path) for path in _c_attr(ctx, key, [])]

def _c_include_dirs(ctx, headers):
    includes = []
    includes.extend(_c_path_attr(ctx, "includes"))
    includes.extend(_c_parent_dirs(headers))
    return _unique(includes)

def _c_collect_transitive(deps, key, own):
    out = []
    out.extend(own)
    for dep in deps:
        out.extend(dep.get(key) or [])
    return _unique(out)

def _c_collect_linkopts(deps, own):
    out = []
    out.extend(own)
    for dep in deps:
        out.extend(dep.get("transitive_linkopts") or [])
    return out

def _c_dynamic_library_records(paths, abi):
    records = []
    if not abi:
        return records
    for path in paths:
        if path:
            records.append({"abi": abi, "path": path})
    return records

def _c_collect_android_native_libraries(deps, own):
    out = []
    out.extend(own)
    for dep in deps:
        out.extend(dep.get("transitive_android_native_libraries") or [])
        out.extend(dep.get("android_native_libraries") or [])
    return _unique_native_libraries(out)

def _c_output_extension(kind):
    os = host_os()
    if kind == "static":
        return ".lib" if os == "windows" else ".a"
    if kind == "shared":
        if os == "windows":
            return ".dll"
        if os == "macos":
            return ".dylib"
        return ".so"
    if kind == "object":
        return ".obj" if os == "windows" else ".o"
    return ""

def _c_library_name(ctx):
    name = _c_attr(ctx, "output_name", "") or ctx["label"]["name"]
    if host_os() == "windows":
        return name + _c_output_extension("static")
    return "lib" + name + _c_output_extension("static")

def _c_sources(ctx):
    return _filter_by_extensions(glob(ctx["srcs"]), [".c", ".cc", ".cpp", ".cxx"])

def _c_headers(ctx):
    headers = []
    headers.extend(_c_path_attr(ctx, "hdrs"))
    headers.extend(_c_globs(_c_attr(ctx, "header_globs", [])))
    return _unique(headers)

def _c_compile_args(src, obj, attrs, compile_context):
    args = ["-c", src, "-o", obj]
    for define in compile_context.get("defines") or []:
        args.append("-D" + define)
    for include in compile_context.get("include_dirs") or []:
        args.extend(["-I", include])
    for include in compile_context.get("quote_include_dirs") or []:
        args.extend(["-I", include])
    for include in compile_context.get("system_include_dirs") or []:
        args.extend(["-isystem", include])
    for include in compile_context.get("framework_include_dirs") or []:
        args.extend(["-F", include])
    args.extend(attrs.get("copts") or [])
    return args

def _c_compile_context(ctx, headers):
    deps = ctx["deps"]
    return {
        "headers": _c_collect_transitive(deps, "transitive_headers", headers),
        "include_dirs": _c_collect_transitive(deps, "transitive_include_dirs", _c_include_dirs(ctx, headers)),
        "quote_include_dirs": _c_collect_transitive(deps, "transitive_quote_include_dirs", _c_path_attr(ctx, "quote_includes")),
        "system_include_dirs": _c_collect_transitive(deps, "transitive_system_include_dirs", _c_path_attr(ctx, "system_includes")),
        "framework_include_dirs": _c_collect_transitive(deps, "transitive_framework_include_dirs", _c_path_attr(ctx, "framework_includes")),
        "defines": _c_collect_transitive(deps, "transitive_defines", _c_attr(ctx, "defines", [])),
    }

def _c_link_context(ctx, archive):
    deps = ctx["deps"]
    own_static = [archive] if archive else []
    own_dynamic = _c_path_attr(ctx, "dynamic_libraries")
    own_linkopts = _c_attr(ctx, "linkopts", [])
    static_libraries = _c_collect_transitive(deps, "transitive_static_libraries", own_static)
    dynamic_libraries = _c_collect_transitive(deps, "transitive_dynamic_libraries", own_dynamic)
    linkopts = _c_collect_linkopts(deps, own_linkopts)
    return {
        "static_libraries": static_libraries,
        "dynamic_libraries": dynamic_libraries,
        "linkopts": linkopts,
    }

def _c_toolchain(ctx, sources):
    cc = _c_attr(ctx, "compiler", "") or host_which("cc")
    ar = _c_attr(ctx, "archiver", "") or host_which("ar")
    cc_version = host_command([cc, "--version"]).strip()
    identity = "once.c.v2\x00host\x00" + host_os() + "\x00" + host_arch() + "\x00cc\x00" + cc + "\x00" + cc_version
    cxx = ""
    if _c_sources_use_cxx(sources):
        cxx = _c_attr(ctx, "cxx_compiler", "") or host_which("c++")
        cxx_version = host_command([cxx, "--version"]).strip()
        identity = identity + "\x00cxx\x00" + cxx + "\x00" + cxx_version
    identity = identity + "\x00ar\x00" + ar + "\x00" + _c_attr(ctx, "archiver_identity", "")
    return (cc, cxx, ar, identity)

def _c_source_uses_cxx(src):
    return _ends_with(src, ".cc") or _ends_with(src, ".cpp") or _ends_with(src, ".cxx")

def _c_sources_use_cxx(sources):
    for src in sources:
        if _c_source_uses_cxx(src):
            return True
    return False

def _c_object_output(src):
    return declare_output("objects/" + src + _c_output_extension("object"))

def _c_library_impl(ctx):
    attrs = ctx["attr"]
    sources = _c_sources(ctx)
    headers = _c_headers(ctx)
    compile_context = _c_compile_context(ctx, headers)
    objects = []
    archive = ""
    if len(sources) > 0:
        cc, cxx, ar, identity = _c_toolchain(ctx, sources)
        for src in sources:
            obj = _c_object_output(src)
            compiler = cxx if _c_source_uses_cxx(src) else cc
            run_action(
                argv = [compiler] + _c_compile_args(src, obj, attrs, compile_context),
                inputs = _unique([src] + compile_context["headers"]),
                outputs = [obj],
                env = _c_attr(ctx, "env", {}),
                toolchain_identity = identity,
                identifier = ctx["label"]["id"] + ":c-compile:" + src,
            )
            objects.append(obj)

        archive = declare_output(_c_library_name(ctx))
        run_action(
            argv = [ar, "crs", archive] + objects,
            inputs = objects,
            outputs = [archive],
            env = _c_attr(ctx, "env", {}),
            toolchain_identity = identity,
            identifier = ctx["label"]["id"] + ":c-archive",
        )

    link_context = _c_link_context(ctx, archive)
    data = _c_globs(_c_attr(ctx, "data", []))
    transitive_data = _c_collect_transitive(ctx["deps"], "transitive_data", data)
    android_abi = _c_attr(ctx, "android_abi", "")
    android_native_libraries = _c_dynamic_library_records(_c_path_attr(ctx, "dynamic_libraries"), android_abi)
    transitive_android_native_libraries = _c_collect_android_native_libraries(ctx["deps"], android_native_libraries)
    return {
        "c_provider": True,
        "label_id": ctx["label"]["id"],
        "target_kind": "c_library",
        "archive": archive,
        "static_library": archive,
        "static_libraries": [archive] if archive else [],
        "dynamic_libraries": _c_path_attr(ctx, "dynamic_libraries"),
        "objects": objects,
        "headers": headers,
        "include_dirs": _c_include_dirs(ctx, headers),
        "quote_include_dirs": _c_path_attr(ctx, "quote_includes"),
        "system_include_dirs": _c_path_attr(ctx, "system_includes"),
        "framework_include_dirs": _c_path_attr(ctx, "framework_includes"),
        "defines": _c_attr(ctx, "defines", []),
        "copts": _c_attr(ctx, "copts", []),
        "linkopts": _c_attr(ctx, "linkopts", []),
        "transitive_headers": compile_context["headers"],
        "transitive_include_dirs": compile_context["include_dirs"],
        "transitive_quote_include_dirs": compile_context["quote_include_dirs"],
        "transitive_system_include_dirs": compile_context["system_include_dirs"],
        "transitive_framework_include_dirs": compile_context["framework_include_dirs"],
        "transitive_defines": compile_context["defines"],
        "transitive_static_libraries": link_context["static_libraries"],
        "transitive_dynamic_libraries": link_context["dynamic_libraries"],
        "transitive_linkopts": link_context["linkopts"],
        "transitive_data": transitive_data,
        "affected_inputs": _unique(sources + headers + transitive_data),
        "default_output": archive,
        "transitive_archives": link_context["static_libraries"],
        "transitive_android_native_libraries": transitive_android_native_libraries,
        "android_native_libraries": android_native_libraries,
    }

_C_LIBRARY_ATTRS = [
    attr("compiler", "string", docs = "Path to the C compiler. Defaults to resolving `cc` on PATH.", configurable = False),
    attr("cxx_compiler", "string", docs = "Path to the C++ compiler. Defaults to resolving `c++` on PATH.", configurable = False),
    attr("archiver", "string", docs = "Path to the static library archiver. Defaults to resolving `ar` on PATH.", configurable = False),
    attr("archiver_identity", "string", docs = "Optional stable identity for the archiver executable, such as a version or content digest, folded into compile and archive cache keys.", configurable = False),
    attr("hdrs", "list<string>", default = "[]", docs = "Public header files exposed to dependents.", configurable = False),
    attr("header_globs", "list<string>", default = "[]", docs = "Public header glob patterns exposed to dependents.", configurable = False),
    attr("includes", "list<string>", default = "[]", docs = "Public include directories propagated to dependents.", configurable = False),
    attr("quote_includes", "list<string>", default = "[]", docs = "Public quote include directories propagated as regular include directories.", configurable = False),
    attr("system_includes", "list<string>", default = "[]", docs = "Public system include directories propagated to dependents.", configurable = False),
    attr("framework_includes", "list<string>", default = "[]", docs = "Public framework search directories propagated to dependents.", configurable = False),
    attr("defines", "list<string>", default = "[]", docs = "Definitions propagated to dependent compile actions.", configurable = False),
    attr("copts", "list<string>", default = "[]", docs = "Compiler flags used by this target's C and C++ compile actions.", configurable = False),
    attr("linkopts", "list<string>", default = "[]", docs = "Linker flags propagated to dependent link actions.", configurable = False),
    attr("dynamic_libraries", "list<string>", default = "[]", docs = "Prebuilt dynamic libraries propagated to dependent link actions.", configurable = False),
    attr("android_abi", "string", docs = "Android Application Binary Interface directory for dynamic_libraries, such as `arm64-v8a`.", configurable = False),
    attr("data", "list<string>", default = "[]", docs = "Runtime data globs propagated to dependents.", configurable = False),
    attr("env", "map<string, string>", default = "{}", docs = "Environment variables passed to compile and archive actions.", configurable = False),
    attr("output_name", "string", docs = "Static library output name without prefix or extension. Defaults to the target name.", configurable = False),
]

c_library = target_kind(
    docs = "C and C++ static library provider consumed by native target kinds such as Rust and Zig.",
    attrs = _C_LIBRARY_ATTRS,
    deps = [dep("deps", ["c_provider"], "C provider dependencies whose headers and link inputs are propagated.")],
    providers = ["c_provider", "native_linkable", "apple_linkable", "android_native_library"],
    capabilities = [capability("build", ["library"])],
    examples = [
        example(
            "c-library-minimal",
            name = "Minimal C library",
            use_when = "Use this when Zig or another native target needs first-party C headers and a static library.",
        ),
    ],
    impl = _c_library_impl,
)
