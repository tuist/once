def _kotlin_apple_sources(ctx):
    return _filter_by_extensions(glob(ctx["srcs"]), [".kt", ".kts"])

def _kotlin_apple_attr(attrs, key, default):
    value = attrs.get(key)
    if value == None:
        return default
    return value

def _kotlin_apple_default_target(platform, sdk_variant, arch):
    if platform == "ios":
        if sdk_variant == "device":
            if arch == "arm64":
                return "ios_arm64"
        elif arch == "arm64":
            return "ios_simulator_arm64"
        elif arch == "x86_64":
            return "ios_x64"
    if platform == "macos" or platform == "macosx":
        if arch == "arm64":
            return "macos_arm64"
        if arch == "x86_64":
            return "macos_x64"
    if platform == "tvos":
        if sdk_variant == "device" and arch == "arm64":
            return "tvos_arm64"
        if sdk_variant != "device" and arch == "arm64":
            return "tvos_simulator_arm64"
        if sdk_variant != "device" and arch == "x86_64":
            return "tvos_x64"
    if platform == "watchos":
        if sdk_variant == "device" and arch == "arm64":
            return "watchos_arm64"
        if sdk_variant != "device" and arch == "arm64":
            return "watchos_simulator_arm64"
        if sdk_variant != "device" and arch == "x86_64":
            return "watchos_x64"
    fail("unsupported Kotlin/Native Apple target for platform `" + platform + "`, sdk_variant `" + sdk_variant + "`, arch `" + arch + "`")

def _kotlin_apple_tool(ctx, attrs):
    configured = _kotlin_apple_attr(attrs, "kotlinc_native", "")
    if configured:
        tool = configured
    else:
        kotlin_home = _kotlin_apple_attr(attrs, "kotlin_home", "")
        if kotlin_home:
            tool = kotlin_home + "/bin/kotlinc-native"
        else:
            tool = host_which("kotlinc-native")
    version = host_command([tool, "-version"]).strip()
    identity = "once.kotlin.apple.native.v1\x00" + tool + "\x00" + version
    return (tool, identity)

def _kotlin_apple_trim_framework_suffix(path):
    suffix = ".framework"
    if _ends_with(path, suffix):
        return path[:len(path) - len(suffix)]
    return path

def _kotlin_apple_java_env(attrs):
    env = {}
    java_home = _kotlin_apple_attr(attrs, "java_home", "") or host_env("JAVA_HOME")
    if java_home:
        env["JAVA_HOME"] = java_home
        env["PATH"] = java_home + "/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    return env

def _kotlin_apple_framework_impl(ctx):
    attrs = ctx["attr"]
    platform = _kotlin_apple_attr(attrs, "platform", "")
    if not platform:
        fail(ctx["label"]["id"] + ": `platform` is required")
    sdk_variant = _kotlin_apple_attr(attrs, "sdk_variant", "simulator")
    arch = _kotlin_apple_attr(attrs, "arch", "") or host_arch()
    target = _kotlin_apple_attr(attrs, "target", "") or _kotlin_apple_default_target(platform, sdk_variant, arch)
    product_name = _kotlin_apple_attr(attrs, "product_name", "") or ctx["label"]["name"]
    module_name = _kotlin_apple_attr(attrs, "module_name", "") or product_name
    compiler_opts = _kotlin_apple_attr(attrs, "compiler_opts", [])
    konan_data_dir = _kotlin_apple_attr(attrs, "konan_data_dir", "")

    sources = _kotlin_apple_sources(ctx)
    if len(sources) == 0:
        fail("kotlin_apple_framework " + ctx["label"]["id"] + " has no Kotlin sources (.kt/.kts)")

    tool, identity = _kotlin_apple_tool(ctx, attrs)
    framework_dir = product_name + ".framework"
    framework_path = ctx["build_dir"] + "/" + framework_dir
    framework_binary = declare_output(framework_dir + "/" + product_name)
    framework_header = declare_output(framework_dir + "/Headers/" + product_name + ".h")
    framework_modulemap = declare_output(framework_dir + "/Modules/module.modulemap")
    framework_info_plist = declare_output(framework_dir + "/Info.plist")
    framework_files = [
        framework_binary,
        framework_header,
        framework_modulemap,
        framework_info_plist,
    ]
    output_base = _kotlin_apple_trim_framework_suffix(framework_path)
    compile_env = _kotlin_apple_java_env(attrs)
    if konan_data_dir:
        compile_env["KONAN_DATA_DIR"] = konan_data_dir
    argv = [
        tool,
        "-target", target,
        "-produce", "framework",
        "-module-name", module_name,
        "-o", output_base,
    ] + compiler_opts + sources

    prepare_path(framework_path, kind = "remove", identifier = "kotlin_apple_framework_clean:" + ctx["label"]["id"])

    run_action(
        argv = argv,
        inputs = sources,
        outputs = framework_files,
        env = compile_env,
        toolchain_identity = identity + "\x00target\x00" + target + "\x00module\x00" + module_name + "\x00konan_data_dir\x00" + konan_data_dir,
        identifier = "kotlin_apple_framework:" + ctx["label"]["id"],
    )

    framework_bundle = {
        "path": framework_path,
        "module_name": module_name,
        "files": framework_files,
        "label_id": ctx["label"]["id"],
    }

    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "kotlin_apple_framework",
        "platform": platform,
        "sdk_variant": sdk_variant,
        "arch": arch,
        "target": target,
        "framework_path": ctx["build_dir"] + "/" + framework_dir,
        "framework_module_name": module_name,
        "framework_files": framework_files,
        "transitive_link_framework_bundles": [framework_bundle],
        "transitive_framework_bundles": [framework_bundle],
        "transitive_frameworks": [framework_path],
        "affected_inputs": sources,
    }

kotlin_apple_framework = target_kind(
    docs = "Compiles Kotlin/Native sources into an Apple framework bundle that Apple app and test targets can import and embed.",
    attrs = [
        attr("platform", "string", required = True, docs = "Apple platform such as `ios`, `macos`, `tvos`, or `watchos`.", configurable = False),
        attr("sdk_variant", "string", default = "\"simulator\"", docs = "`simulator` or `device`; ignored for macOS target inference.", configurable = False),
        attr("arch", "string", docs = "Target architecture used for Kotlin/Native target inference. Defaults to the host arch.", configurable = False),
        attr("target", "string", docs = "Kotlin/Native target, such as `ios_arm64`, `ios_simulator_arm64`, or `macos_arm64`. Defaults from platform, sdk_variant, and arch.", configurable = False),
        attr("product_name", "string", docs = "Framework product name. Defaults to the target name.", configurable = False),
        attr("module_name", "string", docs = "Swift/ObjC module name exported by the framework. Defaults to product_name.", configurable = False),
        attr("kotlinc_native", "string", docs = "Override kotlinc-native path.", configurable = False),
        attr("kotlin_home", "string", docs = "Kotlin/Native installation root used to find bin/kotlinc-native.", configurable = False),
        attr("java_home", "string", docs = "Java runtime home exposed to kotlinc-native. Defaults to JAVA_HOME when available.", configurable = False),
        attr("konan_data_dir", "string", docs = "Optional Kotlin/Native cache directory exposed as KONAN_DATA_DIR.", configurable = False),
        attr("compiler_opts", "list<string>", default = "[]", docs = "Additional kotlinc-native arguments appended after Once-managed flags.", configurable = False),
    ],
    deps = [],
    providers = ["kotlin_native_framework", "apple_framework", "apple_bundle", "native_linkable"],
    capabilities = [capability("build", ["default", "framework"])],
    examples = [
        example(
            "kotlin-apple-framework-minimal",
            name = "Minimal Kotlin Apple framework",
            use_when = "Use this when shared Kotlin code should be imported from Swift or Objective-C on Apple platforms.",
        ),
        example(
            "native-mobile-shared-code-e2e",
            name = "Shared mobile code app graph",
            use_when = "Use this when an Android app should package Swift and Rust native libraries alongside an Apple app that embeds Kotlin and links Rust.",
        ),
    ],
    impl = _kotlin_apple_framework_impl,
)

def _kotlin_jvm_attr(ctx, key, default):
    value = ctx["attr"].get(key)
    if value == None:
        return default
    return value

def _kotlin_jvm_dependency_role(ctx, name):
    return (ctx.get("deps_by_role") or {}).get(name) or []

def _kotlin_jvm_sources(ctx):
    return _filter_by_extensions(glob(ctx["srcs"]), [".kt", ".kts"])

def _kotlin_jvm_compile_deps(ctx):
    return (
        ctx["deps"] +
        _kotlin_jvm_dependency_role(ctx, "associates") +
        _kotlin_jvm_dependency_role(ctx, "exported_deps") +
        _kotlin_jvm_dependency_role(ctx, "provided_deps")
    )

def _kotlin_jvm_runtime_deps(ctx):
    return (
        ctx["deps"] +
        _kotlin_jvm_dependency_role(ctx, "associates") +
        _kotlin_jvm_dependency_role(ctx, "exported_deps") +
        _kotlin_jvm_dependency_role(ctx, "runtime_deps")
    )

def _kotlin_jvm_compile_jars(deps):
    jars = []
    for dep in deps:
        jars.extend(dep.get("transitive_compile_jars") or [])
        jar = dep.get("compile_jar") or dep.get("classes_jar")
        if jar:
            jars.append(jar)
    return _unique(jars)

def _kotlin_jvm_runtime_jars(deps):
    jars = []
    for dep in deps:
        jars.extend(dep.get("transitive_runtime_jars") or [])
        jar = dep.get("runtime_jar") or dep.get("compile_jar") or dep.get("classes_jar")
        if jar:
            jars.append(jar)
    return _unique(jars)

def _kotlin_jvm_classpath_separator():
    if host_os() == "windows":
        return ";"
    return ":"

def _kotlin_jvm_is_absolute(path):
    return path.startswith("/") or (len(path) > 2 and path[1] == ":")

def _kotlin_jvm_workspace_inputs(paths):
    return [path for path in paths if path and not _kotlin_jvm_is_absolute(path)]

def _kotlin_jvm_home(ctx, kotlinc):
    configured = _kotlin_jvm_attr(ctx, "kotlin_home", "")
    if configured:
        return configured
    return _parent_dir(_parent_dir(kotlinc))

def _kotlin_jvm_tools(ctx, include_java):
    kotlinc = _kotlin_jvm_attr(ctx, "kotlinc", "") or host_which("kotlinc")
    kotlin_home = _kotlin_jvm_home(ctx, kotlinc)
    stdlib = _kotlin_jvm_attr(ctx, "kotlin_stdlib", "") or kotlin_home + "/lib/kotlin-stdlib.jar"
    if not host_file_exists(stdlib):
        fail(ctx["label"]["id"] + ": Kotlin standard library not found at `" + stdlib + "`; set `kotlin_stdlib` or `kotlin_home`")
    version = host_command([kotlinc, "-version"], merge_stderr = True).strip()
    tools = {
        "kotlinc": kotlinc,
        "kotlin_home": kotlin_home,
        "kotlin_stdlib": stdlib,
        "identity": "once.kotlin.jvm.v1\x00" + kotlinc + "\x00" + version + "\x00" + host_file_sha256(stdlib),
    }
    if include_java:
        java = _kotlin_jvm_attr(ctx, "java", "") or host_which("java")
        tools["java"] = java
        tools["identity"] = tools["identity"] + "\x00java\x00" + java + "\x00" + host_command([java, "-version"], merge_stderr = True).strip()
    if ctx.get("capability") == "test":
        javac = _kotlin_jvm_attr(ctx, "javac", "") or host_which("javac")
        tools["javac"] = javac
        tools["identity"] = tools["identity"] + "\x00javac\x00" + javac + "\x00" + host_command([javac, "-version"], merge_stderr = True).strip()
    return tools

def _kotlin_jvm_env(ctx):
    env = {}
    java_home = _kotlin_jvm_attr(ctx, "java_home", "") or host_env("JAVA_HOME")
    if java_home:
        env["JAVA_HOME"] = java_home
        env["PATH"] = java_home + "/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    return env

def _kotlin_jvm_data(ctx):
    direct = glob(_kotlin_jvm_attr(ctx, "data", []))
    return _collect_transitive(_kotlin_jvm_runtime_deps(ctx), "transitive_data", direct)

def _kotlin_jvm_output_name(ctx):
    return _kotlin_jvm_attr(ctx, "output", "") or ctx["label"]["name"] + ".jar"

def _kotlin_jvm_compile_argv(ctx, tools, sources, compile_jars, output):
    associates = _kotlin_jvm_compile_jars(_kotlin_jvm_dependency_role(ctx, "associates"))
    plugin_jars = [_package_relative(ctx, path) for path in _kotlin_jvm_attr(ctx, "compiler_plugin_jars", [])]
    module_name = _kotlin_jvm_attr(ctx, "module_name", "") or ctx["label"]["name"]
    args = [
        tools["kotlinc"],
        "-jvm-target", _kotlin_jvm_attr(ctx, "jvm_target", "17"),
        "-module-name", module_name,
    ]
    language_version = _kotlin_jvm_attr(ctx, "language_version", "")
    if language_version:
        args.extend(["-language-version", language_version])
    api_version = _kotlin_jvm_attr(ctx, "api_version", "")
    if api_version:
        args.extend(["-api-version", api_version])
    if len(compile_jars) > 0:
        args.extend(["-classpath", _kotlin_jvm_classpath_separator().join(compile_jars)])
    if len(associates) > 0:
        args.append("-Xfriend-paths=" + _kotlin_jvm_classpath_separator().join(associates))
    for plugin in plugin_jars:
        args.append("-Xplugin=" + plugin)
    for option in _kotlin_jvm_attr(ctx, "compiler_plugin_options", []):
        args.extend(["-P", option])
    args.extend(_kotlin_jvm_attr(ctx, "kotlinc_opts", []))
    args.extend(sources)
    args.extend(["-d", output])
    return args

def _kotlin_jvm_compile(ctx, target_kind):
    sources = _kotlin_jvm_sources(ctx)
    if len(sources) == 0:
        fail(target_kind + " " + ctx["label"]["id"] + " has no Kotlin sources (.kt/.kts)")
    tools = _kotlin_jvm_tools(ctx, False)
    compile_deps = _kotlin_jvm_compile_deps(ctx)
    runtime_deps = _kotlin_jvm_runtime_deps(ctx)
    compile_jars = _kotlin_jvm_compile_jars(compile_deps)
    runtime_jars = _kotlin_jvm_runtime_jars(runtime_deps)
    plugin_jars = [_package_relative(ctx, path) for path in _kotlin_jvm_attr(ctx, "compiler_plugin_jars", [])]
    output = declare_output(_kotlin_jvm_output_name(ctx))
    module_name = _kotlin_jvm_attr(ctx, "module_name", "") or ctx["label"]["name"]
    run_action(
        argv = _kotlin_jvm_compile_argv(ctx, tools, sources, compile_jars, output),
        inputs = _unique(sources + _kotlin_jvm_workspace_inputs(compile_jars + plugin_jars)),
        outputs = [output],
        env = _kotlin_jvm_env(ctx),
        toolchain_identity = tools["identity"] + "\x00module\x00" + module_name,
        identifier = "kotlin_jvm_compile:" + ctx["label"]["id"],
    )
    transitive_compile_jars = _unique([output] + compile_jars)
    transitive_runtime_jars = _unique([output, tools["kotlin_stdlib"]] + runtime_jars)
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": target_kind,
        "compile_jar": output,
        "runtime_jar": output,
        "transitive_compile_jars": transitive_compile_jars,
        "transitive_runtime_jars": transitive_runtime_jars,
        "transitive_data": _kotlin_jvm_data(ctx),
        "affected_inputs": _unique(sources + _kotlin_jvm_workspace_inputs(compile_jars + plugin_jars)),
    }

def _kotlin_jvm_library_impl(ctx):
    return _kotlin_jvm_compile(ctx, "kotlin_jvm_library")

def _kotlin_jvm_binary_impl(ctx):
    if ctx["capability"] != "run":
        return _kotlin_jvm_compile(ctx, "kotlin_jvm_binary")
    tools = _kotlin_jvm_tools(ctx, True)
    binary_jar = ctx["build_dir"] + "/" + _kotlin_jvm_output_name(ctx)
    runtime_jars = _unique([binary_jar, tools["kotlin_stdlib"]] + _kotlin_jvm_runtime_jars(_kotlin_jvm_runtime_deps(ctx)))
    data = _kotlin_jvm_data(ctx)
    run_dir = ctx["build_dir"] + "/run"
    marker = run_dir + "/run.json"
    log = run_dir + "/stdout.log"
    env = _kotlin_jvm_env(ctx)
    for name in _kotlin_jvm_attr(ctx, "env_inherit", []):
        value = host_env(name)
        if value:
            env[name] = value
    for name, value in _kotlin_jvm_attr(ctx, "run_env", {}).items():
        env[name] = value
    prepare_path(run_dir, kind = "directory", identifier = "kotlin_jvm_run_prepare:" + ctx["label"]["id"])
    run_action(
        argv = [tools["java"]] + _kotlin_jvm_attr(ctx, "jvm_flags", []) + [
            "-cp", _kotlin_jvm_classpath_separator().join(runtime_jars),
            _kotlin_jvm_attr(ctx, "main_class", ""),
        ] + _kotlin_jvm_attr(ctx, "args", []),
        inputs = _unique(_kotlin_jvm_workspace_inputs(runtime_jars) + data),
        outputs = [run_dir, log],
        stdout = log,
        stderr = log,
        env = env,
        cacheable = False,
        toolchain_identity = tools["identity"] + "\x00main\x00" + _kotlin_jvm_attr(ctx, "main_class", ""),
        identifier = "kotlin_jvm_run:" + ctx["label"]["id"],
    )
    write_path(marker, _run_result_json(ctx["label"]["id"]))
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "kotlin_jvm_binary",
        "compile_jar": binary_jar,
        "runtime_jar": binary_jar,
        "transitive_runtime_jars": runtime_jars,
        "transitive_data": data,
    }

def _kotlin_jvm_compile_test_runner(ctx, tools):
    source = declare_output("test_runner/OnceJvmTestRunner.java")
    classes = declare_output("test_runner/classes")
    classes_hash = declare_output("test_runner/classes.sha256")
    prepare_path(classes, kind = "remove", identifier = "kotlin_jvm_test_runner_clean:" + ctx["label"]["id"])
    prepare_path(classes, kind = "directory", identifier = "kotlin_jvm_test_runner_prepare:" + ctx["label"]["id"])
    write_path(source, _jvm_test_runner_source())
    run_action(
        argv = [tools["javac"], "-encoding", "UTF-8", "-d", classes, source],
        inputs = [source],
        outputs = [classes],
        env = _kotlin_jvm_env(ctx),
        toolchain_identity = tools["identity"] + "\x00kotlin_jvm_test_runner_javac.v1",
        identifier = "kotlin_jvm_test_runner_compile:" + ctx["label"]["id"],
    )
    write_tree_digest(classes, classes_hash, identifier = "kotlin_jvm_test_runner_digest:" + ctx["label"]["id"])
    return (classes, classes_hash)

def _kotlin_jvm_test_env(ctx, test_dir):
    env = _kotlin_jvm_env(ctx)
    env["HOME"] = test_dir + "/home"
    for name in _kotlin_jvm_attr(ctx, "env_inherit", []):
        value = host_env(name)
        if value:
            env[name] = value
    for name, value in _kotlin_jvm_attr(ctx, "env", {}).items():
        env[name] = value
    for name, value in _kotlin_jvm_attr(ctx, "test_env", {}).items():
        env[name] = value
    return env

def _kotlin_jvm_test_info(ctx, command_argv, command_env, results, log, native_results):
    return {
        "schema": "once.test_info.v1",
        "target": ctx["label"]["id"],
        "runner": {
            "type": "kotlin_jvm",
            "display_name": "Kotlin Java virtual machine test",
            "metadata": {},
        },
        "command": {
            "argv": command_argv,
            "env": command_env,
            "cwd": ".",
        },
        "outputs": {
            "results": results,
            "logs": [log],
            "native_results": [native_results],
            "coverage": [],
        },
        "listing": {
            "supported": True,
            "strategy": "scan_compiled_classes",
        },
        "filtering": {
            "case_filtering": "class_or_method",
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
            "timeout_ms": _kotlin_jvm_attr(ctx, "timeout_ms", None),
            "run_from_workspace_root": True,
        },
        "labels": _kotlin_jvm_attr(ctx, "labels", []),
        "metadata": {},
    }

def _kotlin_jvm_test_impl(ctx):
    sources = _kotlin_jvm_sources(ctx)
    if len(sources) == 0:
        fail("kotlin_jvm_test " + ctx["label"]["id"] + " has no Kotlin test sources (.kt/.kts)")
    tools = _kotlin_jvm_tools(ctx, True)
    compile_jars = _kotlin_jvm_compile_jars(_kotlin_jvm_compile_deps(ctx))
    runtime_jars = _kotlin_jvm_runtime_jars(_kotlin_jvm_runtime_deps(ctx))
    plugin_jars = [_package_relative(ctx, path) for path in _kotlin_jvm_attr(ctx, "compiler_plugin_jars", [])]
    classes = declare_output("test/classes")
    classes_hash = declare_output("test/classes.sha256")
    prepare_path(classes, kind = "remove", identifier = "kotlin_jvm_test_classes_clean:" + ctx["label"]["id"])
    prepare_path(classes, kind = "directory", identifier = "kotlin_jvm_test_classes_prepare:" + ctx["label"]["id"])
    run_action(
        argv = _kotlin_jvm_compile_argv(ctx, tools, sources, compile_jars, classes),
        inputs = _unique(sources + _kotlin_jvm_workspace_inputs(compile_jars + plugin_jars)),
        outputs = [classes],
        env = _kotlin_jvm_env(ctx),
        toolchain_identity = tools["identity"] + "\x00kotlin_jvm_test_compile.v1",
        identifier = "kotlin_jvm_test_compile:" + ctx["label"]["id"],
    )
    write_tree_digest(classes, classes_hash, identifier = "kotlin_jvm_test_classes_digest:" + ctx["label"]["id"])
    runner_classes, runner_hash = _kotlin_jvm_compile_test_runner(ctx, tools)
    test_dir = ctx["build_dir"] + "/test-results"
    results = test_dir + "/test_results.json"
    log = test_dir + "/kotlin-jvm-test.log"
    native_results = test_dir + "/native_results.txt"
    runtime_classpath = _unique([runner_classes, classes, tools["kotlin_stdlib"]] + runtime_jars)
    command_argv = [tools["java"]] + _kotlin_jvm_attr(ctx, "jvm_flags", []) + [
        "-cp", _kotlin_jvm_classpath_separator().join(runtime_classpath),
        "OnceJvmTestRunner",
        classes,
        results,
        log,
        native_results,
        ctx["label"]["id"],
        "kotlin_jvm",
    ]
    test_class = _kotlin_jvm_attr(ctx, "test_class", "")
    if test_class:
        command_argv.append(test_class)
    command_argv.extend(_kotlin_jvm_attr(ctx, "args", []))
    command_env = _kotlin_jvm_test_env(ctx, test_dir)
    provider = {
        "label_id": ctx["label"]["id"],
        "target_kind": "kotlin_jvm_test",
        "classes_dir": classes,
        "classes_hash": classes_hash,
        "affected_inputs": _unique(sources + _kotlin_jvm_workspace_inputs(compile_jars + plugin_jars) + _kotlin_jvm_data(ctx)),
        "test_info": _kotlin_jvm_test_info(ctx, command_argv, command_env, results, log, native_results),
    }
    if ctx["capability"] != "test":
        return provider
    runtime_file_inputs = []
    for entry in runtime_classpath:
        if entry != runner_classes and entry != classes:
            runtime_file_inputs.append(entry)
    run_action(
        argv = command_argv,
        inputs = _unique([classes_hash, runner_hash] + _kotlin_jvm_workspace_inputs(runtime_file_inputs) + _kotlin_jvm_data(ctx)),
        outputs = [test_dir, results, log, native_results],
        env = command_env,
        toolchain_identity = tools["identity"] + "\x00kotlin_jvm_test_run.v1\x00classpath\x00" + _kotlin_jvm_classpath_separator().join(runtime_classpath),
        identifier = "kotlin_jvm_test:" + ctx["label"]["id"],
    )
    return provider

_KOTLIN_JVM_COMMON_ATTRS = [
    attr("module_name", "string", docs = "Kotlin module name. Defaults to the target name.", configurable = False),
    attr("output", "string", docs = "Output Java archive file name. Defaults to the target name with a .jar suffix.", configurable = False),
    attr("jvm_target", "string", default = "\"17\"", docs = "Java virtual machine bytecode target passed to kotlinc."),
    attr("language_version", "string", docs = "Kotlin language version passed to kotlinc."),
    attr("api_version", "string", docs = "Kotlin application programming interface version passed to kotlinc."),
    attr("kotlinc_opts", "list<string>", default = "[]", docs = "Additional kotlinc arguments."),
    attr("compiler_plugin_jars", "list<string>", default = "[]", docs = "Package-relative Kotlin compiler plug-in Java archives passed through -Xplugin.", configurable = False),
    attr("compiler_plugin_options", "list<string>", default = "[]", docs = "Kotlin compiler plug-in options passed through -P.", configurable = False),
    attr("data", "list<string>", default = "[]", docs = "Package-relative runtime data patterns propagated to binaries."),
    attr("kotlinc", "string", docs = "Override the kotlinc path.", configurable = False),
    attr("kotlin_home", "string", docs = "Kotlin installation root used to find the standard library.", configurable = False),
    attr("kotlin_stdlib", "string", docs = "Override the Kotlin standard library Java archive path.", configurable = False),
    attr("java_home", "string", docs = "Java runtime home exposed to kotlinc. Defaults to JAVA_HOME when available.", configurable = False),
]

_KOTLIN_JVM_DEP_PROVIDERS = ["kotlin_jvm_library", "java_library"]
_KOTLIN_JVM_DEP_EDGES = [
    dep("deps", _KOTLIN_JVM_DEP_PROVIDERS, "Kotlin and Java libraries used for compilation and runtime."),
    dep("associates", ["kotlin_jvm_library"], "Kotlin friend modules whose internal declarations are visible during compilation."),
    dep("exported_deps", _KOTLIN_JVM_DEP_PROVIDERS, "Libraries compiled here and exported to downstream compile and runtime classpaths."),
    dep("provided_deps", _KOTLIN_JVM_DEP_PROVIDERS, "Libraries available for compilation but omitted from this target's runtime classpath."),
    dep("runtime_deps", _KOTLIN_JVM_DEP_PROVIDERS, "Libraries added only to this target's runtime classpath."),
]

_KOTLIN_TOOL = tool("kotlin", ["kotlinc"])
_JAVA_RUNTIME_TOOL = tool("java", ["java"])
# javac ships with the JDK, so it resolves from the same mise "java" tool as
# the runtime rather than a tool of its own. The distinct requirement keeps the
# javac executable off the classpath of targets that only need the runtime.
_JAVA_COMPILER_TOOL = tool("java", ["javac"])

kotlin_jvm_library = target_kind(
    docs = "Compiles Kotlin sources into a Java archive with typed compile, friend, exported, provided, and runtime dependency roles.",
    attrs = _KOTLIN_JVM_COMMON_ATTRS,
    deps = _KOTLIN_JVM_DEP_EDGES,
    providers = ["kotlin_jvm_library", "java_library"],
    capabilities = [capability("build", ["default", "jar"])],
    tools = [_KOTLIN_TOOL],
    source_references = [
        source_reference("Buck2", "kotlin_library", "https://buck2.build/docs/prelude/rules/kotlin/kotlin_library/", "Use when adopting a Buck2 Kotlin library and its classpath roles."),
        source_reference("Bazel rules_kotlin", "kt_jvm_library", "https://github.com/bazel-contrib/rules_kotlin", "Use when adopting a Bazel Kotlin library, associates, and standard Java dependency roles."),
    ],
    examples = [
        example("kotlin-jvm-library-minimal", name = "Minimal Kotlin Java virtual machine library", use_when = "Start here for a first-party Kotlin library that emits a Java archive."),
    ],
    impl = _kotlin_jvm_library_impl,
)

kotlin_jvm_binary = target_kind(
    docs = "Compiles a Kotlin Java virtual machine entry point into a Java archive and runs its main class through the host Java runtime.",
    attrs = _KOTLIN_JVM_COMMON_ATTRS + [
        attr("main_class", "string", required = True, docs = "Fully qualified Kotlin main class, such as dev.once.hello.MainKt.", configurable = False),
        attr("args", "list<string>", default = "[]", docs = "Arguments passed to the main class during once run.", configurable = False),
        attr("jvm_flags", "list<string>", default = "[]", docs = "Flags passed to the Java virtual machine before the classpath.", configurable = False),
        attr("run_env", "map<string,string>", default = "{}", docs = "Environment variables passed to the Java runtime.", configurable = False),
        attr("env_inherit", "list<string>", default = "[]", docs = "Host environment variable names inherited before run_env overrides.", configurable = False),
        attr("java", "string", docs = "Override the Java runtime path.", configurable = False),
    ],
    deps = _KOTLIN_JVM_DEP_EDGES,
    providers = ["kotlin_jvm_binary"],
    capabilities = [
        capability("build", ["default", "jar"]),
        capability("run", ["default"], ["jar"]),
    ],
    tools = [_KOTLIN_TOOL, _JAVA_RUNTIME_TOOL],
    source_references = [
        source_reference("Bazel rules_kotlin", "kt_jvm_binary", "https://github.com/bazel-contrib/rules_kotlin", "Use when adopting a Bazel Kotlin binary and its runtime classpath."),
    ],
    examples = [
        example("kotlin-jvm-binary-minimal", name = "Minimal Kotlin Java virtual machine binary", use_when = "Use this for a runnable Kotlin main class with a local library dependency."),
    ],
    impl = _kotlin_jvm_binary_impl,
)

kotlin_jvm_test = target_kind(
    docs = "Compiles Kotlin tests for the Java virtual machine and emits normalized Once test results.",
    attrs = _KOTLIN_JVM_COMMON_ATTRS + [
        attr("test_class", "string", docs = "Optional fully qualified test class or Class#method filter.", configurable = False),
        attr("args", "list<string>", default = "[]", docs = "Additional fully qualified class or Class#method filters.", configurable = False),
        attr("jvm_flags", "list<string>", default = "[]", docs = "Flags passed to the Java virtual machine before the test classpath.", configurable = False),
        attr("env", "map<string,string>", default = "{}", docs = "Environment variables passed to the test runner before test_env overrides.", configurable = False),
        attr("env_inherit", "list<string>", default = "[]", docs = "Host environment variable names inherited before explicit test environment values.", configurable = False),
        attr("test_env", "map<string,string>", default = "{}", docs = "Environment variables passed to the test runner.", configurable = False),
        attr("java", "string", docs = "Override the Java runtime path.", configurable = False),
        attr("javac", "string", docs = "Override the Java compiler path used for the reflection runner.", configurable = False),
        attr("labels", "list<string>", default = "[]", docs = "Labels exposed through once_test_info for test discovery."),
        attr("timeout_ms", "int", docs = "Optional test timeout in milliseconds.", configurable = False),
    ],
    deps = _KOTLIN_JVM_DEP_EDGES,
    providers = ["kotlin_jvm_test", "once_test_info"],
    capabilities = [capability("test", ["default", "test_results", "logs"])],
    tools = [_KOTLIN_TOOL, _JAVA_RUNTIME_TOOL, _JAVA_COMPILER_TOOL],
    source_references = [
        source_reference("Buck2", "kotlin_test", "https://buck2.build/docs/prelude/rules/kotlin/kotlin_test/", "Use when adopting a Buck2 host-side Kotlin test and its test runner settings."),
        source_reference("Bazel rules_kotlin", "kt_jvm_test", "https://github.com/bazel-contrib/rules_kotlin", "Use when adopting a Bazel Kotlin Java virtual machine test and its runtime dependencies."),
    ],
    examples = [
        example("kotlin-jvm-test-minimal", name = "Minimal Kotlin Java virtual machine test", use_when = "Use this for host-side Kotlin tests that should emit normalized Once test results."),
    ],
    impl = _kotlin_jvm_test_impl,
)
