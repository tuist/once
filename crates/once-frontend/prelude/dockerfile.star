_DOCKERFILE_TOOL = tool("docker", executables = ["docker"])

def _dockerfile_attr(ctx, name, default):
    return _configured_attr(ctx, name, default)

def _dockerfile_local_path(value, attribute):
    normalized = value.replace("\\", "/")
    if (
        not normalized or
        normalized.startswith("/") or
        (len(normalized) > 1 and normalized[1] == ":") or
        "://" in normalized
    ):
        _dockerfile_fail("dockerfile_invalid_path", attribute, attribute + " must be a non-empty package-relative path", "Set " + attribute + " to a path relative to the target's package, without a leading slash or scheme")
    parts = []
    for part in normalized.split("/"):
        if part == "..":
            _dockerfile_fail("dockerfile_path_outside_package", attribute, attribute + " must stay inside the package", "Declare the target in the directory that should be the root, and set " + attribute + " relative to it")
        if part and part != ".":
            parts.append(part)
    return "/".join(parts) if parts else "."

def _dockerfile_workspace_path(ctx, value):
    if value == ".":
        return ctx["label"]["package"] or "."
    if value.startswith("./"):
        value = value[2:]
    package = ctx["label"]["package"]
    return package + "/" + value if package else value

def _dockerfile_existing_ignore(ctx, dockerfile, context):
    specific = dockerfile + ".dockerignore"
    specific_workspace_path = _dockerfile_workspace_path(ctx, specific)
    if host_file_exists(workspace_root() + "/" + specific_workspace_path):
        return specific
    context_ignore = ".dockerignore" if context == "." else context + "/.dockerignore"
    context_workspace_path = _dockerfile_workspace_path(ctx, context_ignore)
    if host_file_exists(workspace_root() + "/" + context_workspace_path):
        return context_ignore
    return None

def _dockerfile_literal_excludes(source):
    lines = []
    has_negation = False
    for raw in source.split("\n"):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("!"):
            has_negation = True
            continue
        lines.append(line)
    if has_negation:
        return {"paths": [], "names": []}
    paths = []
    names = []
    for line in lines:
        normalized = line[1:] if line.startswith("/") else line
        normalized = normalized[:-1] if normalized.endswith("/") else normalized
        basename = normalized[3:] if normalized.startswith("**/") else None
        if (
            not normalized or
            normalized == "." or
            normalized == ".." or
            normalized.startswith("/") or
            normalized.endswith("/") or
            normalized.startswith("../") or
            normalized.endswith("/..") or
            "/../" in normalized or
            "//" in normalized or
            "\\" in normalized or
            "*" in normalized or
            "?" in normalized or
            "[" in normalized or
            "]" in normalized
        ):
            if (
                basename and
                "/" not in basename and
                "\\" not in basename and
                "*" not in basename and
                "?" not in basename and
                "[" not in basename and
                "]" not in basename and
                basename not in [".", ".."]
            ):
                names.append(basename)
            continue
        paths.append(normalized)
    return {
        "paths": _unique(paths),
        "names": _unique(names),
    }

def _dockerfile_inputs(ctx, dockerfile, context):
    ignore = _dockerfile_existing_ignore(ctx, dockerfile, context)
    symlinks = {}
    if ctx["srcs"] and not _dockerfile_attr(ctx, "resolver_inputs", []):
        inputs = _file_globs(ctx["srcs"])
        discovery = "explicit"
    else:
        excludes = {"paths": [], "names": []}
        if ignore:
            ignore_workspace_path = _dockerfile_workspace_path(ctx, ignore)
            excludes = _dockerfile_literal_excludes(
                host_file_read(workspace_root() + "/" + ignore_workspace_path),
            )
        excluded_names = _unique(excludes["names"] + [".once"])
        inputs = walk_files(
            context,
            excluded_paths = excludes["paths"],
            excluded_names = excluded_names,
            include_empty_directories = True,
        )
        for entry in walk_symlinks(context, excluded_paths = excludes["paths"], excluded_names = excluded_names):
            link, target = entry.split("\x00", 1)
            symlinks[link] = target
        discovery = "context"
    inputs.append(_dockerfile_workspace_path(ctx, dockerfile))
    if ignore:
        inputs.append(_dockerfile_workspace_path(ctx, ignore))
    return (sorted(_unique(inputs)), discovery, symlinks)

def _dockerfile_default_tag(ctx):
    name = ctx["label"]["id"].replace("/", "-").lower()
    return "once-" + name + ":latest"

def _dockerfile_auto_builder(docker, network):
    endpoint = host_command([docker, "context", "inspect", "--format", "{{json .Endpoints.docker}}"]).strip()
    fingerprint = content_sha256(_json_encode([
        endpoint,
        host_env("DOCKER_CONTEXT"),
        host_env("DOCKER_HOST"),
        host_env("DOCKER_CONFIG"),
        network,
    ]))
    name = "once-images-" + fingerprint[:16]
    create = [docker, "buildx", "create", "--name", name, "--driver", "docker-container"]
    if network == "host":
        create.extend(["--buildkitd-flags", "--allow-insecure-entitlement network.host"])
    host_command(create, check = False)
    return name

def _dockerfile_toolchain(ctx, auto_builder = False):
    docker = _resolve_host_executable("docker")
    if not docker:
        _dockerfile_fail("required_tool_not_found", None, ctx["label"]["id"] + ": Docker executable was not found", "Install Docker with Buildx and make sure the docker executable is on PATH")
    client = host_command([docker, "buildx", "version"]).strip()
    builder = _dockerfile_attr(ctx, "builder", "") or host_env("BUILDX_BUILDER") or ""
    inspect_args = [docker, "buildx", "inspect"]
    if builder:
        inspect_args.append(builder)
    inspect_args.append("--bootstrap")
    driver = ""
    inspected = host_command(inspect_args)
    for line in inspected.split("\n"):
        stripped = line.strip()
        if stripped.startswith("Driver:"):
            driver = stripped[len("Driver:"):].strip()
    if driver == "docker" and not builder and auto_builder:
        builder = _dockerfile_auto_builder(docker, _dockerfile_attr(ctx, "network", "default"))
        inspected = host_command([docker, "buildx", "inspect", builder, "--bootstrap"])
    buildkit = ""
    driver = ""
    for line in inspected.split("\n"):
        stripped = line.strip()
        if stripped.startswith("BuildKit version:"):
            buildkit = stripped
        elif stripped.startswith("Driver:"):
            driver = stripped[len("Driver:"):].strip()
    if not buildkit:
        _dockerfile_fail("dockerfile_builder_unusable", "builder", ctx["label"]["id"] + ": Docker Buildx did not report a BuildKit version", "Select a working Buildx builder with the builder attribute or BUILDX_BUILDER, and check that docker buildx inspect --bootstrap succeeds")
    identity = "once.dockerfile.v5\x00" + docker + "\x00" + client + "\x00" + builder + "\x00" + driver + "\x00" + buildkit
    return (docker, identity, driver, builder)

def _dockerfile_environment():
    environment = {}
    for name in [
        "BUILDKIT_HOST",
        "BUILDX_CONFIG",
        "BUILDX_BUILDER",
        "DOCKER_CERT_PATH",
        "DOCKER_CONFIG",
        "DOCKER_CONTEXT",
        "DOCKER_HOST",
        "DOCKER_TLS",
        "DOCKER_TLS_VERIFY",
        "HOME",
        "PATH",
        "SSH_AUTH_SOCK",
        "SSL_CERT_DIR",
        "SSL_CERT_FILE",
        "TMPDIR",
        "XDG_RUNTIME_DIR",
    ]:
        value = host_env(name)
        if value:
            environment[name] = value
    return environment

def _dockerfile_platform(value):
    if not value:
        return {"os": "", "architecture": "", "variant": ""}
    if "," in value:
        _dockerfile_fail("dockerfile_invalid_platform", "platform", "platform must name exactly one operating-system/architecture[/variant] value", "Set platform to a single value such as linux/arm64")
    parts = value.split("/")
    if len(parts) < 2 or len(parts) > 3:
        _dockerfile_fail("dockerfile_invalid_platform", "platform", "platform must use operating-system/architecture[/variant] form", "Set platform to a value such as linux/arm64")
    return {
        "os": _once_normalize_os(parts[0]),
        "architecture": _once_normalize_architecture(parts[1]),
        "variant": parts[2] if len(parts) == 3 else "",
    }

def _dockerfile_build_args(ctx, source_date_epoch):
    configured = dict(_dockerfile_attr(ctx, "build_args", {}))
    if configured.get("SOURCE_DATE_EPOCH") != None:
        _dockerfile_fail("dockerfile_reserved_build_arg", "build_args", "build_args must not override SOURCE_DATE_EPOCH; use source_date_epoch", "Remove SOURCE_DATE_EPOCH from build_args and set source_date_epoch")
    configured["SOURCE_DATE_EPOCH"] = str(source_date_epoch)
    return configured

def _dockerfile_cached_context_definition(ctx, dockerfile, context):
    recipe = declare_output("build-definition/" + dockerfile.split("/")[-1])
    ignore = recipe + ".dockerignore"
    source = _dockerfile_workspace_path(ctx, dockerfile)
    copy_path(source, recipe, inputs = [source], identifier = ctx["label"]["id"] + ":build-definition", source_files = _action_source_files([source]))
    effective_ignore = _dockerfile_existing_ignore(ctx, dockerfile, context)
    rules = ""
    if effective_ignore:
        rules = host_file_read(workspace_root() + "/" + _dockerfile_workspace_path(ctx, effective_ignore))
    write_path(ignore, rules + "\n**/.once\n")
    return (recipe, ignore)

def _dockerfile_cache_reference(reference):
    if not reference or "," in reference or "=" in reference or " " in reference or "\n" in reference:
        _dockerfile_fail("dockerfile_invalid_cache_reference", "cache_from", "cache_from and cache_to must contain registry image references, without exporter options", "Use plain references such as registry.example.com/app:cache")
    return reference

def _dockerfile_buildkit_only_attrs(ctx):
    if _dockerfile_attr(ctx, "cache_to", []):
        return "cache_to"
    if _dockerfile_attr(ctx, "export_cache", False):
        return "export_cache"
    if (ctx.get("deps_by_role") or {}).get("caches", []):
        return "caches dependencies"
    return None

def _dockerfile_load(ctx):
    dockerfile = _dockerfile_local_path(_dockerfile_attr(ctx, "dockerfile", "Dockerfile"), "dockerfile")
    context = _dockerfile_local_path(_dockerfile_attr(ctx, "context", "."), "context")
    context_target = host_symlink_target(workspace_root() + "/" + _dockerfile_workspace_path(ctx, context)) if context != "." else ""
    if context_target:
        _dockerfile_fail("dockerfile_context_is_symlink", "context", ctx["label"]["id"] + ": the build context " + context + " is a symbolic link to " + context_target, "Set context to the directory the link points to, " + context_target)
    source = host_file_read(workspace_root() + "/" + _dockerfile_workspace_path(ctx, dockerfile))
    return (dockerfile, context, source, _dockerfile_annotate(ctx, _dockerfile_instructions(source)))

def _dockerfile_fallback_reason(ctx, instructions):
    if _dockerfile_attr(ctx, "execution_mode", "auto") != "auto":
        return (None, 1)
    exclusive = _dockerfile_buildkit_only_attrs(ctx)
    if exclusive:
        return (exclusive + " requires whole-file execution", 1)
    unsupported = _dockerfile_first_unsupported(instructions)
    if unsupported:
        return ("line " + str(unsupported["line"]) + " uses " + unsupported["unsupported"], unsupported["line"])
    return (None, 1)

def _dockerfile_lint_impl(ctx):
    dockerfile, context, _, instructions = _dockerfile_load(ctx)
    findings = _dockerfile_findings(ctx, instructions)
    findings.extend(_dockerfile_missing_sources(ctx, instructions, context, _dockerfile_inputs(ctx, dockerfile, context)[0]))
    reason, line = _dockerfile_fallback_reason(ctx, instructions)
    if reason:
        findings.append(_dockerfile_finding(ctx, {"line": line, "opcode": "FROM"}, "whole_file_execution", "This image builds as one BuildKit action instead of per-instruction snapshots: " + reason + ".", "Remove the construct to get per-instruction caching, or set execution_mode = \"buildkit\" to make whole-file execution explicit.", "note"))
    return _dockerfile_lint(ctx, dockerfile, findings)

def _dockerfile_image_impl(ctx):
    if ctx["capability"] == "lint":
        return _dockerfile_lint_impl(ctx)
    mode = _dockerfile_attr(ctx, "execution_mode", "auto")
    if mode == "buildkit":
        return _dockerfile_buildkit_impl(ctx)
    if mode == "instructions":
        return _dockerfile_instruction_impl(ctx)
    if _dockerfile_buildkit_only_attrs(ctx):
        return _dockerfile_buildkit_impl(ctx, _dockerfile_fallback_reason(ctx, [])[0])
    reason = _dockerfile_fallback_reason(ctx, _dockerfile_load(ctx)[3])[0]
    if reason:
        return _dockerfile_buildkit_impl(ctx, reason)
    return _dockerfile_instruction_impl(ctx, True)

def _dockerfile_buildkit_impl(ctx, fallback_reason = None, inherits_onbuild = False):
    dockerfile = _dockerfile_local_path(
        _dockerfile_attr(ctx, "dockerfile", "Dockerfile"),
        "dockerfile",
    )
    context = _dockerfile_local_path(
        _dockerfile_attr(ctx, "context", "."),
        "context",
    )
    inputs, input_discovery, _ = _dockerfile_inputs(ctx, dockerfile, context)
    if ctx["capability"] == "metadata":
        return {
            "container_image": True,
            "dockerfile_image": True,
            "label_id": ctx["label"]["id"],
            "target_kind": "dockerfile_image",
            "execution_mode": "buildkit",
            "fallback_reason": fallback_reason,
            "input_discovery": input_discovery,
            "affected_inputs": inputs,
        }
    output_format = _dockerfile_attr(ctx, "format", "docker")
    cacheable = _dockerfile_attr(ctx, "cacheable", False)
    pull = _dockerfile_attr(ctx, "pull", True)
    if cacheable and pull:
        _dockerfile_fail("dockerfile_cacheable_requirements", "cacheable", "cacheable image export requires pull = false", "Set pull = false, or set cacheable = false")
    if cacheable:
        instructions = _dockerfile_load(ctx)[3]
        _dockerfile_require_pinned_bases(ctx, instructions, _dockerfile_selected_stages(ctx, instructions)[0])
    source_date_epoch = _dockerfile_attr(ctx, "source_date_epoch", 0)
    if source_date_epoch < 0:
        _dockerfile_fail("dockerfile_invalid_source_date_epoch", "source_date_epoch", "source_date_epoch must be non-negative", "Set source_date_epoch to 0 or a positive Unix timestamp")
    tags = _dockerfile_tags(ctx)
    tag = tags[0] if tags else ""
    extension = ".oci.tar" if output_format == "oci" else ".docker.tar"
    archive = declare_output(ctx["label"]["name"] + extension)
    metadata = declare_output("build-metadata.json")
    docker, toolchain_identity, driver, builder = _dockerfile_toolchain(ctx)
    if output_format == "oci" and driver == "docker":
        _dockerfile_fail("dockerfile_builder_unsupported", "builder", ctx["label"]["id"] + ": Open Container Initiative export requires a Docker Buildx builder whose driver supports archive export; select a docker-container or remote builder", "Set builder to a docker-container or remote Buildx builder, or set format = \"docker\"")
    direct_export = output_format == "oci" or (output_format == "docker" and driver and driver != "docker")
    if cacheable and not direct_export:
        _dockerfile_fail("dockerfile_builder_unsupported", "builder", "cacheable image export requires a container or remote builder with direct archive export", "Set builder to a docker-container or remote Buildx builder, or set cacheable = false")
    cache_to = _dockerfile_attr(ctx, "cache_to", [])
    if cacheable and cache_to:
        _dockerfile_fail("dockerfile_cacheable_requirements", "cache_to", "cache_to publishes remote state and cannot be combined with cacheable = true", "Remove cache_to or set cacheable = false")
    layer_cache = declare_output("layer-cache") if _dockerfile_attr(ctx, "export_cache", False) else None
    caches = (ctx.get("deps_by_role") or {}).get("caches", [])
    build_definition = dockerfile
    if caches:
        recipe, ignore = _dockerfile_cached_context_definition(ctx, dockerfile, context)
        inputs.extend([recipe, ignore])
        build_definition = execution_path(recipe)
    argv = [
        docker,
        "buildx",
        "build",
        "--file", build_definition,
        "--metadata-file", execution_path(metadata),
        "--progress", "plain",
        "--provenance=false",
        "--sbom=false",
        "--network", _dockerfile_attr(ctx, "network", "default"),
    ]
    if output_format == "docker":
        if not tags:
            _dockerfile_fail("dockerfile_empty_tag", "tag", "tag must not be empty for Docker archive output", "Set tag to an image reference such as name:latest")
        if direct_export:
            argv.extend([
                "--output",
                "type=docker,dest=" + execution_path(archive) + ",rewrite-timestamp=true",
            ])
        else:
            argv.append("--load")
    else:
        argv.extend([
            "--output",
            "type=oci,dest=" + execution_path(archive) + ",rewrite-timestamp=true",
        ])
    if builder:
        argv.extend(["--builder", builder])
    platform = _dockerfile_attr(ctx, "platform", "")
    if cacheable and not platform:
        _dockerfile_fail("dockerfile_cacheable_requirements", "platform", "cacheable image export requires an explicit platform so worker architecture participates in the cache key", "Set platform = \"linux/<architecture>\", or set cacheable = false")
    _dockerfile_platform(platform)
    if platform:
        argv.extend(["--platform", platform])
    target = _dockerfile_attr(ctx, "target", "")
    if target:
        argv.extend(["--target", target])
    if pull:
        argv.append("--pull")
    if _dockerfile_attr(ctx, "no_cache", False):
        argv.append("--no-cache")
    build_args = _dockerfile_build_args(ctx, source_date_epoch)
    for key in sorted(build_args.keys()):
        value = build_args[key]
        argv.extend(["--build-arg", key + "=" + value])
    labels = _dockerfile_attr(ctx, "labels", {})
    for key in sorted(labels.keys()):
        argv.extend(["--label", key + "=" + labels[key]])
    annotations = _dockerfile_attr(ctx, "annotations", {})
    for key in sorted(annotations.keys()):
        argv.extend(["--annotation", key + "=" + annotations[key]])
    for reference in tags:
        argv.extend(["--tag", reference])
    if _dockerfile_attr(ctx, "network", "default") == "host":
        argv.extend(["--allow", "network.host"])
    for reference in _dockerfile_attr(ctx, "cache_from", []):
        argv.extend(["--cache-from", "type=registry,ref=" + _dockerfile_cache_reference(reference)])
    for reference in cache_to:
        argv.extend(["--cache-to", "type=registry,ref=" + _dockerfile_cache_reference(reference) + ",mode=max"])
    for provider in caches:
        source = provider.get("layer_cache")
        if not source:
            _dockerfile_fail("dockerfile_invalid_cache_dependency", "caches", "caches dependencies must enable export_cache = true", "Set export_cache = true on every target listed in caches")
        inputs.append(source)
        argv.extend(["--cache-from", "type=local,src=" + execution_path(source)])
    if layer_cache:
        argv.extend(["--cache-to", "type=local,dest=" + execution_path(layer_cache) + ",mode=max"])
    layout = declare_output("layout") if direct_export else None
    if layout:
        argv.extend(["--output", "type=oci,tar=false,dest=" + execution_path(layout) + ",rewrite-timestamp=true"])
    named = _dockerfile_named_contexts(ctx)
    for entry in named.values():
        inputs.extend(entry["inputs"])
    argv.extend(_dockerfile_context_flags({name: {"path": execution_path(entry["path"]), "layout": entry["layout"], "ref": entry.get("ref", "")} for name, entry in named.items()}))
    argv.append(context)

    build_outputs = [metadata] if output_format == "docker" and not direct_export else [archive, metadata]
    if layout:
        build_outputs.append(layout)
    if layer_cache:
        build_outputs.append(layer_cache)
    run_action(
        display_name = "Build container image · " + ctx["label"]["name"],
        source_files = _action_source_files(inputs),
        argv = argv,
        inputs = inputs,
        outputs = build_outputs,
        clean_paths = build_outputs,
        cwd = ctx["label"]["package"] or None,
        env = _dockerfile_environment(),
        sandbox = "copied-inputs",
        cacheable = cacheable,
        toolchain_identity = toolchain_identity,
        identifier = ctx["label"]["id"] + ":dockerfile-build",
    )
    if output_format == "docker" and not direct_export:
        run_action(
            display_name = "Export container image · " + ctx["label"]["name"],
            source_files = _action_source_files([metadata]),
            argv = [
                docker,
                "image",
                "save",
                "--output", execution_path(archive),
            ] + tags,
            inputs = [metadata],
            outputs = [archive],
            cwd = ctx["label"]["package"] or None,
            env = _dockerfile_environment(),
            sandbox = "copied-inputs",
            cacheable = False,
            toolchain_identity = toolchain_identity,
            identifier = ctx["label"]["id"] + ":docker-save",
        )
    return {
        "container_image": True,
        "dockerfile_image": True,
        "label_id": ctx["label"]["id"],
        "target_kind": "dockerfile_image",
        "archive": archive,
        "layout": layout,
        "layer_cache": layer_cache,
        "metadata": metadata,
        "format": output_format,
        "tag": tag,
        "tags": tags,
        "platform": _dockerfile_platform(platform),
        "execution_mode": "buildkit",
        "fallback_reason": fallback_reason,
        "onbuild": inherits_onbuild or _dockerfile_has_onbuild(_dockerfile_load(ctx)[3]),
        "input_discovery": input_discovery,
        "affected_inputs": inputs,
        "default_output": archive,
    }

_DOCKERFILE_REFERENCES = [
    source_reference(
        "Docker Build",
        "Dockerfile",
        "https://docs.docker.com/reference/dockerfile/",
        "Translate instruction boundaries and stage references while delegating each instruction's execution semantics to the Dockerfile frontend.",
    ),
    source_reference(
        "Docker Buildx",
        "build",
        "https://docs.docker.com/reference/cli/docker/buildx/build/",
        "Use BuildKit for base-image resolution, RUN execution, package installation, and Docker or Open Container Initiative archive export.",
    ),
]

dockerfile_image = target_kind(
    docs = "Builds a Dockerfile, by default as independently declared image snapshot actions, and exports a Docker or Open Container Initiative archive.",
    attrs = [
        attr("execution_mode", "string", default = "\"auto\"", docs = "auto translates Dockerfile instructions into independently declared snapshot actions and falls back to a single whole-file build when the Dockerfile uses extended syntax or a whole-file-only option; the metadata provider reports the reason. instructions requires translatable syntax. buildkit always runs one whole-file build.", configurable = False, allowed_values = ["auto", "instructions", "buildkit"]),
        attr("dockerfile", "string", default = "\"Dockerfile\"", docs = "Dockerfile path relative to the target package.", configurable = False),
        attr("resolver_inputs", "list<string>", default = "[]", docs = "Dockerfile marker supplied by native project discovery. Explicit targets should use srcs to control the build context.", configurable = False),
        attr("context", "string", default = "\".\"", docs = "Build context directory relative to the target package.", configurable = False),
        attr("format", "string", default = "\"docker\"", docs = "Archive format accepted by the BuildKit exporter.", configurable = False, allowed_values = ["docker", "oci"]),
        attr("tag", "string", docs = "Image reference. Defaults to a package-qualified target name prefixed with once and tagged latest.", configurable = True),
        attr("platform", "string", docs = "Optional operating-system/architecture[/variant] build platform.", configurable = True),
        attr("target", "string", docs = "Optional named Dockerfile stage to export.", configurable = True),
        attr("build_args", "map<string,string>", default = "{}", docs = "Build arguments passed without shell interpolation.", configurable = True),
        attr("labels", "map<string,string>", default = "{}", docs = "Image labels added by BuildKit.", configurable = True),
        attr("annotations", "map<string,string>", default = "{}", docs = "Image annotations added by BuildKit.", configurable = True),
        attr("network", "string", default = "\"default\"", docs = "Network mode for Dockerfile RUN instructions.", configurable = False, allowed_values = ["default", "none", "host"]),
        attr("builder", "string", docs = "Optional Docker Buildx builder name.", configurable = False),
        attr("pull", "bool", default = "True", docs = "Check registries for current base-image records before building.", configurable = False),
        attr("no_cache", "bool", default = "False", docs = "Disable BuildKit layer-cache reuse.", configurable = False),
        attr("source_date_epoch", "int", default = "0", docs = "Fixed Unix timestamp used for the image creation time and, in whole-file mode, for exported layer file timestamps.", configurable = False),
        attr("cache_from", "list<string>", default = "[]", docs = "Registry image references containing BuildKit layer caches to import.", configurable = False),
        attr("cache_to", "list<string>", default = "[]", docs = "In buildkit mode, publish registry layer caches. Cannot be combined with cacheable because publication must run.", configurable = False),
        attr("export_cache", "bool", default = "False", docs = "In buildkit mode, export intermediate layers as a declared directory, transferable through the Once cache and reusable through caches dependencies.", configurable = False),
        attr("reproducible_layers", "bool", default = "False", docs = "In instruction mode, rewrite each instruction's layer timestamps to source_date_epoch so independent rebuilds produce identical layer bytes. Later instructions then observe normalized timestamps, which can change the decisions of tools that compare them.", configurable = False),
        attr("context_names", "map<string,string>", default = "{}", docs = "Names for the images and programs dependencies, from a target label to the name the Dockerfile uses. A registry reference such as alpine:3.23 replaces that reference wherever the Dockerfile mentions it.", configurable = False),
        attr("tags", "list<string>", default = "[]", docs = "Additional image references for the archive, besides tag.", configurable = False),
        attr("cacheable", "bool", default = "False", docs = "Cache instruction snapshots or the whole-file result when all remote inputs are immutable. Requires pull = false and an explicit platform.", configurable = False),
    ],
    deps = [
        dep("caches", ["dockerfile_image"], "Image targets with export_cache enabled, whose declared layer caches are restored before building."),
        dep("images", ["container_image"], "Images passed to the Dockerfile as named build contexts, so FROM name and COPY --from=name use a content-addressed layout instead of a registry."),
        dep("programs", ["once_executable"], "Native executables passed to the Dockerfile as named build contexts holding the executable under its file name."),
    ],
    providers = ["container_image", "dockerfile_image"],
    capabilities = [capability("build", ["archive", "metadata", "layer_cache", "layout", "plan"]), capability("lint", ["sarif", "results"])],
    tools = [_DOCKERFILE_TOOL],
    examples = [
        example(
            "dockerfile-image-instructions",
            name = "Dockerfile instructions as independent actions",
            use_when = "Use this to build an existing multi-stage Dockerfile with instruction snapshots, explicit stage dependencies, and advisory input checks. Once provisions a container builder when Docker's built-in builder is the implicit default.",
        ),
        example(
            "dockerfile-image-minimal",
            name = "Dockerfile image with a package installation",
            use_when = "Use this when BuildKit should pull a pinned base image, execute Dockerfile RUN instructions, and export a loadable image archive.",
        ),
        example(
            "dockerfile-image-contexts",
            name = "Dockerfile using other targets as build contexts",
            use_when = "Use this to build a Dockerfile whose base image is a digest-pinned oci_pull target and whose COPY --from source is a native executable built by Once.",
        ),
        example(
            "dockerfile-image-layer-cache",
            name = "Image targets sharing declared layer caches",
            use_when = "Use this to export intermediate layers and import them through a caches dependency without contacting a registry.",
        ),
    ],
    source_references = _DOCKERFILE_REFERENCES,
    impl = _dockerfile_image_impl,
)

dockerfile = native_project(
    target_kind = "dockerfile_image",
    docs = "Recognizes a Dockerfile and exposes an image build without once.toml.",
    markers = ["Dockerfile"],
    target_name = "image",
    exclude = _native_project_generated_dirs() + [".git", ".once", "node_modules", ".build"],
)
