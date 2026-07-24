_CMAKE_BUILD_DRIVER = """
cmake_minimum_required(VERSION 3.20)

foreach(_once_required ONCE_SOURCE_DIR ONCE_BUILD_DIR)
  if(NOT DEFINED ${_once_required} OR "${${_once_required}}" STREQUAL "")
    message(FATAL_ERROR "${_once_required} is required")
  endif()
endforeach()

set(_once_configure
  "${CMAKE_COMMAND}"
  "-S" "${ONCE_SOURCE_DIR}"
  "-B" "${ONCE_BUILD_DIR}"
)
if(DEFINED ONCE_GENERATOR AND NOT "${ONCE_GENERATOR}" STREQUAL "")
  list(APPEND _once_configure "-G" "${ONCE_GENERATOR}")
endif()
if(DEFINED ONCE_BUILD_TYPE AND NOT "${ONCE_BUILD_TYPE}" STREQUAL "")
  list(APPEND _once_configure "-DCMAKE_BUILD_TYPE=${ONCE_BUILD_TYPE}")
endif()
if(DEFINED ONCE_CONFIGURE_ARGS AND NOT "${ONCE_CONFIGURE_ARGS}" STREQUAL "")
  list(APPEND _once_configure ${ONCE_CONFIGURE_ARGS})
endif()

execute_process(
  COMMAND ${_once_configure}
  RESULT_VARIABLE _once_configure_result
  COMMAND_ECHO STDOUT
)
if(NOT _once_configure_result EQUAL 0)
  message(FATAL_ERROR "CMake configuration failed with exit code ${_once_configure_result}")
endif()

set(_once_build "${CMAKE_COMMAND}" "--build" "${ONCE_BUILD_DIR}")
if(DEFINED ONCE_BUILD_TYPE AND NOT "${ONCE_BUILD_TYPE}" STREQUAL "")
  list(APPEND _once_build "--config" "${ONCE_BUILD_TYPE}")
endif()
if(DEFINED ONCE_BUILD_TARGETS AND NOT "${ONCE_BUILD_TARGETS}" STREQUAL "")
  list(APPEND _once_build "--target" ${ONCE_BUILD_TARGETS})
endif()
if(DEFINED ONCE_BUILD_ARGS AND NOT "${ONCE_BUILD_ARGS}" STREQUAL "")
  list(APPEND _once_build ${ONCE_BUILD_ARGS})
endif()

execute_process(
  COMMAND ${_once_build}
  RESULT_VARIABLE _once_build_result
  COMMAND_ECHO STDOUT
)
if(NOT _once_build_result EQUAL 0)
  message(FATAL_ERROR "CMake build failed with exit code ${_once_build_result}")
endif()
"""

def _cmake_attr(ctx, key, default):
    return _configured_attr(ctx, key, default)

def _cmake_package_path(ctx, path):
    if not path or path == ".":
        return ctx["label"]["package"] or "."
    return _package_relative(ctx, path)

def _cmake_path_attrs(ctx, key):
    return [_package_relative(ctx, path) for path in _cmake_attr(ctx, key, [])]

def _cmake_globs(patterns):
    return glob(patterns) if patterns else []

def _cmake_collect(deps, key, own):
    values = []
    values.extend(own)
    for dep in deps:
        values.extend(dep.get(key) or [])
    return _unique(values)

def _cmake_encode_list(values):
    encoded = []
    for value in values:
        encoded.append(value.replace("\\", "\\\\").replace(";", "\\;"))
    return ";".join(encoded)

def _cmake_product_tokens():
    os = host_os()
    return {
        "{static_prefix}": "" if os == "windows" else "lib",
        "{static_suffix}": ".lib" if os == "windows" else ".a",
        "{shared_prefix}": "" if os == "windows" else "lib",
        "{shared_suffix}": ".dll" if os == "windows" else (".dylib" if os == "macos" else ".so"),
        "{exe_suffix}": ".exe" if os == "windows" else "",
    }

def _cmake_product_path(ctx, product):
    path = product.replace("\\", "/")
    if not path or path.startswith("/") or (len(path) > 1 and path[1] == ":"):
        fail(ctx["label"]["id"] + ": CMake product paths must be non-empty and relative to the generated build directory")
    for component in path.split("/"):
        if component == "..":
            fail(ctx["label"]["id"] + ": CMake product path `" + product + "` must stay inside the generated build directory")
    for token, value in _cmake_product_tokens().items():
        path = path.replace(token, value)
    return path

def _cmake_static_library(path):
    return _ends_with(path, ".a") or _ends_with(path, ".lib")

def _cmake_dynamic_library(path):
    return _ends_with(path, ".so") or _ends_with(path, ".dylib") or _ends_with(path, ".dll")

def _cmake_project_inputs(ctx, headers):
    inputs = glob(ctx["srcs"])
    inputs.extend(headers)
    inputs.extend(_cmake_globs(_cmake_attr(ctx, "data", [])))
    for dep in ctx["deps"]:
        inputs.extend(dep.get("transitive_headers") or [])
        inputs.extend(dep.get("transitive_static_libraries") or [])
        inputs.extend(dep.get("transitive_dynamic_libraries") or [])
        inputs.extend(dep.get("transitive_data") or [])
    return _unique(inputs)

def _cmake_build_program(ctx, generator):
    requested = _cmake_attr(ctx, "build_program", "")
    if not requested and generator == "Ninja":
        requested = "ninja"
    if not requested:
        return ("", "")
    program = _resolve_host_executable(requested)
    if not program:
        fail(ctx["label"]["id"] + ": generated build program `" + requested + "` was not found")
    return (program, host_command([program, "--version"]).strip())

def _cmake_project_impl(ctx):
    products = [_cmake_product_path(ctx, product) for product in _cmake_attr(ctx, "products", [])]
    if not products:
        fail(ctx["label"]["id"] + ": products must list at least one file produced under the generated CMake build directory")

    cmake = _resolve_host_executable(_cmake_attr(ctx, "cmake", "cmake"))
    if not cmake:
        fail(ctx["label"]["id"] + ": CMake executable was not found")
    version = host_command([cmake, "--version"]).strip()
    source_dir = _cmake_package_path(ctx, _cmake_attr(ctx, "source_dir", "."))
    build_root = ctx["scratch_dir"] + "/cmake-build"
    driver = ctx["scratch_dir"] + "/OnceCMakeBuild.cmake"
    generator = _cmake_attr(ctx, "generator", "Ninja")
    build_type = _cmake_attr(ctx, "build_type", "Debug")
    build_program, build_program_version = _cmake_build_program(ctx, generator)
    configure_args = _cmake_attr(ctx, "configure_args", [])
    if build_program:
        configure_args = ["-DCMAKE_MAKE_PROGRAM:FILEPATH=" + build_program] + configure_args
    raw_products = [build_root + "/" + product for product in products]
    headers = _unique(_cmake_path_attrs(ctx, "hdrs") + _cmake_globs(_cmake_attr(ctx, "header_globs", [])))
    inputs = _cmake_project_inputs(ctx, headers)

    action_env = dict(_cmake_attr(ctx, "env", {}))
    if not action_env.get("PATH"):
        action_env["PATH"] = host_env("PATH")
    write_path(driver, _CMAKE_BUILD_DRIVER)
    run_action(
        argv = [
            cmake,
            "-DONCE_SOURCE_DIR:PATH=" + execution_path(source_dir),
            "-DONCE_BUILD_DIR:PATH=" + execution_path(build_root),
            "-DONCE_GENERATOR:STRING=" + generator,
            "-DONCE_BUILD_TYPE:STRING=" + build_type,
            "-DONCE_CONFIGURE_ARGS:STRING=" + _cmake_encode_list(configure_args),
            "-DONCE_BUILD_TARGETS:STRING=" + _cmake_encode_list(_cmake_attr(ctx, "build_targets", [])),
            "-DONCE_BUILD_ARGS:STRING=" + _cmake_encode_list(_cmake_attr(ctx, "build_args", [])),
            "-P", execution_path(driver),
        ],
        inputs = _unique(inputs + [driver]),
        outputs = raw_products,
        clean_paths = [build_root],
        env = action_env,
        toolchain_identity = "once.cmake.project.v1\x00" + version + "\x00" + generator + "\x00" + build_program + "\x00" + build_program_version + "\x00" + build_type,
        identifier = ctx["label"]["id"] + ":cmake-build",
    )

    staged_products = []
    for index in range(len(products)):
        staged = declare_output("products/" + products[index])
        copy_path(
            raw_products[index],
            staged,
            inputs = [raw_products[index]],
            toolchain_identity = "once.cmake.product.v1",
            identifier = ctx["label"]["id"] + ":cmake-product:" + products[index],
        )
        staged_products.append(staged)

    own_static = [path for path in staged_products if _cmake_static_library(path)]
    own_dynamic = [path for path in staged_products if _cmake_dynamic_library(path)]
    include_dirs = _cmake_path_attrs(ctx, "includes")
    defines = _cmake_attr(ctx, "defines", [])
    linkopts = _cmake_attr(ctx, "linkopts", [])
    data = _cmake_globs(_cmake_attr(ctx, "data", []))
    transitive_headers = _cmake_collect(ctx["deps"], "transitive_headers", headers)
    transitive_include_dirs = _cmake_collect(ctx["deps"], "transitive_include_dirs", include_dirs)
    transitive_defines = _cmake_collect(ctx["deps"], "transitive_defines", defines)
    transitive_static = _cmake_collect(ctx["deps"], "transitive_static_libraries", own_static)
    transitive_dynamic = _cmake_collect(ctx["deps"], "transitive_dynamic_libraries", own_dynamic)
    transitive_linkopts = _cmake_collect(ctx["deps"], "transitive_linkopts", linkopts)
    transitive_data = _cmake_collect(ctx["deps"], "transitive_data", data)
    archive = own_static[0] if own_static else ""
    return {
        "cmake_project": True,
        "c_provider": True,
        "label_id": ctx["label"]["id"],
        "target_kind": "cmake_project",
        "products": staged_products,
        "archive": archive,
        "static_library": archive,
        "static_libraries": own_static,
        "dynamic_libraries": own_dynamic,
        "headers": headers,
        "include_dirs": include_dirs,
        "defines": defines,
        "linkopts": linkopts,
        "transitive_headers": transitive_headers,
        "transitive_include_dirs": transitive_include_dirs,
        "transitive_quote_include_dirs": _cmake_collect(ctx["deps"], "transitive_quote_include_dirs", []),
        "transitive_system_include_dirs": _cmake_collect(ctx["deps"], "transitive_system_include_dirs", []),
        "transitive_framework_include_dirs": _cmake_collect(ctx["deps"], "transitive_framework_include_dirs", []),
        "transitive_defines": transitive_defines,
        "transitive_static_libraries": transitive_static,
        "transitive_dynamic_libraries": transitive_dynamic,
        "transitive_linkopts": transitive_linkopts,
        "transitive_data": transitive_data,
        "transitive_archives": transitive_static,
        "affected_inputs": _unique(inputs + transitive_data),
        "default_output": staged_products[0],
    }

def _cmake_resolver_file(ctx, path, description):
    key = path[2:] if path.startswith("./") else path
    content = (ctx.get("files") or {}).get(key)
    if content == None:
        fail(ctx["label"]["id"] + ": " + description + " `" + path + "` must be included in resolver_inputs, or srcs when resolver_inputs is omitted")
    return content

def _cmake_snapshot_inputs(ctx):
    attrs = ctx.get("attrs") or ctx.get("attr") or {}
    return _resolver_snapshot_inputs(ctx.get("files") or {}, [attrs.get("snapshot") or ""])

def _cmake_snapshot_selection(ctx):
    attrs = ctx.get("attrs") or ctx.get("attr") or {}
    return {
        "source_dir": attrs.get("source_dir") or ".",
        "generator": attrs.get("generator") or "Ninja",
        "build_type": attrs.get("build_type") or "Debug",
    }

def _cmake_validate_snapshot(ctx, snapshot, path):
    if snapshot.get("schema") != "once.cmake.snapshot.v1":
        fail(ctx["label"]["id"] + ": CMake snapshot `" + path + "` must use schema `once.cmake.snapshot.v1`")
    binding = snapshot.get("once_snapshot")
    if type(binding) != type({}):
        fail(ctx["label"]["id"] + ": CMake snapshot `" + path + "` has no once_snapshot provenance")
    expected_inputs = _cmake_snapshot_inputs(ctx)
    bound_inputs = binding.get("inputs")
    if type(bound_inputs) != type({}) or sorted(bound_inputs.keys()) != sorted(expected_inputs.keys()):
        fail(ctx["label"]["id"] + ": CMake snapshot `" + path + "` input set does not match resolver_inputs")
    for input_path, content in expected_inputs.items():
        if bound_inputs.get(input_path) != content:
            fail(ctx["label"]["id"] + ": CMake snapshot `" + path + "` is stale relative to `" + input_path + "`")
    selection = binding.get("selection")
    if type(selection) != type({}):
        fail(ctx["label"]["id"] + ": CMake snapshot `" + path + "` has no configuration selection provenance")
    for key, value in _cmake_snapshot_selection(ctx).items():
        if selection.get(key) != value:
            fail(ctx["label"]["id"] + ": CMake snapshot `" + path + "` selection `" + key + "` does not match the target")
    if not binding.get("fingerprint"):
        fail(ctx["label"]["id"] + ": CMake snapshot `" + path + "` has no normalized fingerprint")

def _cmake_snapshot_targets(snapshot):
    targets = snapshot.get("targets")
    if type(targets) != type([]):
        fail("CMake snapshot targets must be a list")
    by_name = {}
    for target in targets:
        if type(target) != type({}):
            fail("CMake snapshot targets must contain only records")
        name = target.get("once_name")
        if not name:
            fail("CMake snapshot target has no once_name")
        if name in by_name:
            fail("CMake snapshot contains duplicate target `" + name + "`")
        by_name[name] = target
    for target in targets:
        for dep in target.get("deps") or []:
            if dep not in by_name:
                fail("CMake snapshot target `" + target["once_name"] + "` references missing dependency `" + dep + "`")
    return (targets, by_name)

def _cmake_selected_target_names(ctx, snapshot, targets, by_name):
    requested = (ctx.get("attrs") or {}).get("exports") or snapshot.get("exports") or []
    selected = {}
    for value in requested:
        match = value if value in by_name else ""
        if not match:
            for target in targets:
                if target.get("name") == value:
                    match = target["once_name"]
                    break
        if not match:
            fail(ctx["label"]["id"] + ": requested CMake export `" + value + "` is absent from the snapshot")
        selected[match] = True
    if not selected:
        for target in targets:
            selected[target["once_name"]] = True
    for _ in range(len(targets) + 1):
        for target in targets:
            if selected.get(target["once_name"]):
                for dep in target.get("deps") or []:
                    selected[dep] = True
    return selected

def _cmake_target_spec(target):
    return {
        "name": target["once_name"],
        "kind": "cmake_target",
        "deps": target.get("deps") or [],
        "srcs": target.get("sources") or [],
        "attrs": {
            "cmake_name": target.get("name") or target["once_name"],
            "cmake_type": target.get("type") or "",
            "artifacts": target.get("artifacts") or [],
            "include_dirs": target.get("include_dirs") or [],
            "compile_definitions": target.get("compile_definitions") or [],
            "snapshot_fingerprint": target.get("snapshot_fingerprint") or "",
            "_cmake_resolved": True,
        },
    }

def _cmake_workspace_resolver(ctx):
    attrs = ctx.get("attrs") or {}
    path = attrs.get("snapshot") or ""
    if not path:
        fail(ctx["label"]["id"] + ": snapshot is required")
    snapshot = json_decode(_cmake_resolver_file(ctx, path, "CMake snapshot"))
    _cmake_validate_snapshot(ctx, snapshot, path)
    targets, by_name = _cmake_snapshot_targets(snapshot)
    selected = _cmake_selected_target_names(ctx, snapshot, targets, by_name)
    specs = [_cmake_target_spec(target) for target in targets if selected.get(target["once_name"])]
    roots = []
    requested = attrs.get("exports") or snapshot.get("exports") or []
    if requested:
        for value in requested:
            if value in by_name:
                roots.append(value)
            else:
                for target in targets:
                    if target.get("name") == value:
                        roots.append(target["once_name"])
                        break
    else:
        roots = [spec["name"] for spec in specs]
    binding = snapshot["once_snapshot"]
    return {
        "targets": specs,
        "roots": _unique(roots),
        "attrs": {
            "_cmake_resolved": True,
            "_cmake_snapshot_fingerprint": binding["fingerprint"],
            "_cmake_exports": _unique(roots),
        },
    }

def _cmake_workspace_impl(ctx):
    if not _cmake_attr(ctx, "_cmake_resolved", False):
        fail(ctx["label"]["id"] + ": cmake_workspace must be expanded by its graph resolver before analysis")
    return {
        "cmake_workspace": True,
        "label_id": ctx["label"]["id"],
        "snapshot_fingerprint": _cmake_attr(ctx, "_cmake_snapshot_fingerprint", ""),
        "exports": _cmake_attr(ctx, "_cmake_exports", []),
        "targets": ctx["deps"],
    }

def _cmake_target_impl(ctx):
    if not _cmake_attr(ctx, "_cmake_resolved", False):
        fail(ctx["label"]["id"] + ": cmake_target is resolver-owned and must come from cmake_workspace")
    return {
        "cmake_target": True,
        "label_id": ctx["label"]["id"],
        "cmake_name": _cmake_attr(ctx, "cmake_name", ctx["label"]["name"]),
        "cmake_type": _cmake_attr(ctx, "cmake_type", ""),
        "artifacts": _cmake_attr(ctx, "artifacts", []),
        "sources": glob(ctx["srcs"]),
        "include_dirs": _cmake_attr(ctx, "include_dirs", []),
        "compile_definitions": _cmake_attr(ctx, "compile_definitions", []),
        "snapshot_fingerprint": _cmake_attr(ctx, "snapshot_fingerprint", ""),
        "dependencies": [dep.get("label_id") for dep in ctx["deps"]],
    }

_CMAKE_EXAMPLES = [
    example(
        "cmake-project-minimal",
        name = "CMake static library bridge",
        use_when = "Use this when CMake remains authoritative while Once builds declared products and imports a checked logical target snapshot.",
    ),
]

_CMAKE_SOURCE_REFERENCES = [
    source_reference(
        "CMake",
        "cmake",
        "https://cmake.org/cmake/help/latest/manual/cmake.1.html",
        "Use this for the authoritative configure and generated-build invocation contract.",
    ),
    source_reference(
        "CMake",
        "file-based application programming interface",
        "https://cmake.org/cmake/help/latest/manual/cmake-file-api.7.html",
        "Use this to import configured targets, dependencies, sources, artifacts, and toolchain metadata without parsing CMakeLists.txt.",
    ),
]

cmake_project = target_kind(
    docs = "Coarse bridge that keeps CMake authoritative, runs configure and build in one cached action, stages only declared products, and exposes native link providers.",
    attrs = [
        attr("cmake", "string", default = "\"cmake\"", docs = "CMake executable name, absolute path, or workspace-relative path.", configurable = False),
        attr("source_dir", "string", default = "\".\"", docs = "Package-relative CMake source directory.", configurable = False),
        attr("generator", "string", default = "\"Ninja\"", docs = "CMake generated-build backend.", configurable = False),
        attr("build_program", "string", docs = "Generated-build executable. Defaults to resolving `ninja` for the Ninja generator.", configurable = False),
        attr("build_type", "string", default = "\"Debug\"", docs = "CMake build configuration passed during configure and build."),
        attr("configure_args", "list<string>", default = "[]", docs = "Additional arguments passed to CMake configuration.", configurable = False),
        attr("build_targets", "list<string>", default = "[]", docs = "Optional native CMake targets passed to the generated build.", configurable = False),
        attr("build_args", "list<string>", default = "[]", docs = "Additional arguments passed to `cmake --build`.", configurable = False),
        attr("products", "list<string>", required = True, docs = "Files expected under the generated build directory. Product tokens support static, shared, and executable platform suffixes.", configurable = False),
        attr("hdrs", "list<string>", default = "[]", docs = "Public package-relative headers exposed to native dependents.", configurable = False),
        attr("header_globs", "list<string>", default = "[]", docs = "Public header glob patterns exposed to native dependents.", configurable = False),
        attr("includes", "list<string>", default = "[]", docs = "Public include directories propagated to native dependents.", configurable = False),
        attr("defines", "list<string>", default = "[]", docs = "Preprocessor definitions propagated to native dependents.", configurable = False),
        attr("linkopts", "list<string>", default = "[]", docs = "Linker options propagated to native dependents.", configurable = False),
        attr("data", "list<string>", default = "[]", docs = "Runtime data globs propagated to dependents.", configurable = False),
        attr("env", "map<string,string>", default = "{}", docs = "Environment variables passed to CMake configuration and build.", configurable = False),
    ],
    deps = [dep("deps", ["c_provider"], "Native dependency files included in the CMake action input tree.")],
    providers = ["cmake_project", "c_provider", "native_linkable", "apple_linkable"],
    capabilities = [capability("build", ["default", "products", "library"])],
    tools = [tool("cmake")],
    examples = _CMAKE_EXAMPLES,
    source_references = _CMAKE_SOURCE_REFERENCES,
    impl = _cmake_project_impl,
)

cmake_workspace = target_kind(
    docs = "Imports a checked CMake file-interface snapshot as queryable synthetic targets while leaving execution with cmake_project.",
    attrs = [
        attr("snapshot", "string", required = True, docs = "Package-relative normalized CMake snapshot with exact resolver-input provenance.", configurable = False),
        attr("resolver_inputs", "list<string>", default = "[]", docs = "Package-relative text globs supplied to the resolver. Include the snapshot and every bound configuration input.", configurable = False),
        attr("source_dir", "string", default = "\".\"", docs = "Package-relative source directory recorded in snapshot selection provenance.", configurable = False),
        attr("generator", "string", default = "\"Ninja\"", docs = "CMake generated-build backend recorded in snapshot selection provenance.", configurable = False),
        attr("build_type", "string", default = "\"Debug\"", docs = "CMake build configuration recorded in snapshot selection provenance."),
        attr("exports", "list<string>", default = "[]", docs = "CMake names or generated Once names to expose. Defaults to snapshot exports.", configurable = False),
        attr("_cmake_resolved", "bool", default = "false", docs = "Resolver-owned marker indicating that the snapshot was expanded.", configurable = False),
        attr("_cmake_snapshot_fingerprint", "string", docs = "Resolver-owned normalized snapshot fingerprint.", configurable = False),
        attr("_cmake_exports", "list<string>", default = "[]", docs = "Resolver-owned generated export names.", configurable = False),
    ],
    deps = [dep("deps", ["cmake_target"], "Resolver-generated logical CMake targets.")],
    providers = ["cmake_workspace"],
    examples = _CMAKE_EXAMPLES,
    source_references = _CMAKE_SOURCE_REFERENCES,
    resolver = _cmake_workspace_resolver,
    impl = _cmake_workspace_impl,
)

cmake_target = target_kind(
    docs = "Resolver-generated logical CMake target used for graph queries, dependency explanations, and affected-input reasoning.",
    attrs = [
        attr("cmake_name", "string", required = True, docs = "Original configured CMake target name.", configurable = False),
        attr("cmake_type", "string", required = True, docs = "Configured CMake target type.", configurable = False),
        attr("artifacts", "list<string>", default = "[]", docs = "Normalized artifacts reported by CMake.", configurable = False),
        attr("include_dirs", "list<string>", default = "[]", docs = "Normalized include directories reported by CMake.", configurable = False),
        attr("compile_definitions", "list<string>", default = "[]", docs = "Configured compile definitions reported by CMake.", configurable = False),
        attr("snapshot_fingerprint", "string", required = True, docs = "Normalized snapshot fingerprint shared by imported targets.", configurable = False),
        attr("_cmake_resolved", "bool", default = "false", docs = "Resolver-owned marker preventing direct manifest authoring.", configurable = False),
    ],
    deps = [dep("deps", ["cmake_target"], "Configured logical CMake target dependencies.")],
    providers = ["cmake_target"],
    examples = _CMAKE_EXAMPLES,
    source_references = _CMAKE_SOURCE_REFERENCES,
    impl = _cmake_target_impl,
)
