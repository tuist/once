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
        fail(attribute + " must be a non-empty package-relative path")
    parts = []
    for part in normalized.split("/"):
        if part == "..":
            fail(attribute + " must stay inside the package")
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
    if ctx["srcs"]:
        inputs = _file_globs(ctx["srcs"])
        discovery = "explicit"
    else:
        excludes = {"paths": [], "names": []}
        if ignore:
            ignore_workspace_path = _dockerfile_workspace_path(ctx, ignore)
            excludes = _dockerfile_literal_excludes(
                host_file_read(workspace_root() + "/" + ignore_workspace_path),
            )
        inputs = walk_files(
            context,
            excluded_paths = excludes["paths"],
            excluded_names = _unique(excludes["names"] + [".once"]),
        )
        discovery = "context"
    inputs.append(_dockerfile_workspace_path(ctx, dockerfile))
    if ignore:
        inputs.append(_dockerfile_workspace_path(ctx, ignore))
    return (sorted(_unique(inputs)), discovery)

def _dockerfile_default_tag(ctx):
    name = ctx["label"]["id"].replace("/", "-").lower()
    return "once-" + name + ":latest"

def _dockerfile_toolchain(ctx):
    docker = _resolve_host_executable("docker")
    if not docker:
        fail(ctx["label"]["id"] + ": Docker executable was not found")
    client = host_command([docker, "buildx", "version"]).strip()
    inspect_args = [docker, "buildx", "inspect"]
    builder = _dockerfile_attr(ctx, "builder", "")
    if builder:
        inspect_args.append(builder)
    inspect_args.append("--bootstrap")
    buildkit = ""
    driver = ""
    for line in host_command(inspect_args).split("\n"):
        stripped = line.strip()
        if stripped.startswith("BuildKit version:"):
            buildkit = stripped
        elif stripped.startswith("Driver:"):
            driver = stripped[len("Driver:"):].strip()
    if not buildkit:
        fail(ctx["label"]["id"] + ": Docker Buildx did not report a BuildKit version")
    identity = "once.dockerfile.v2\x00" + docker + "\x00" + client + "\x00" + driver + "\x00" + buildkit
    return (docker, identity, driver)

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
        fail("platform must name exactly one operating-system/architecture[/variant] value")
    parts = value.split("/")
    if len(parts) < 2 or len(parts) > 3:
        fail("platform must use operating-system/architecture[/variant] form")
    return {
        "os": _once_normalize_os(parts[0]),
        "architecture": _once_normalize_architecture(parts[1]),
        "variant": parts[2] if len(parts) == 3 else "",
    }

def _dockerfile_build_args(ctx, source_date_epoch):
    configured = dict(_dockerfile_attr(ctx, "build_args", {}))
    if configured.get("SOURCE_DATE_EPOCH") != None:
        fail("build_args must not override SOURCE_DATE_EPOCH; use source_date_epoch")
    configured["SOURCE_DATE_EPOCH"] = str(source_date_epoch)
    return configured

def _dockerfile_image_impl(ctx):
    dockerfile = _dockerfile_local_path(
        _dockerfile_attr(ctx, "dockerfile", "Dockerfile"),
        "dockerfile",
    )
    context = _dockerfile_local_path(
        _dockerfile_attr(ctx, "context", "."),
        "context",
    )
    inputs, input_discovery = _dockerfile_inputs(ctx, dockerfile, context)
    if ctx["capability"] == "metadata":
        return {
            "container_image": True,
            "dockerfile_image": True,
            "label_id": ctx["label"]["id"],
            "target_kind": "dockerfile_image",
            "input_discovery": input_discovery,
            "affected_inputs": inputs,
        }
    output_format = _dockerfile_attr(ctx, "format", "docker")
    cacheable = _dockerfile_attr(ctx, "cacheable", False)
    pull = _dockerfile_attr(ctx, "pull", True)
    if cacheable and output_format != "oci":
        fail("cacheable may only be enabled for direct Open Container Initiative export")
    if cacheable and pull:
        fail("cacheable Open Container Initiative export requires pull = false")
    source_date_epoch = _dockerfile_attr(ctx, "source_date_epoch", 0)
    if source_date_epoch < 0:
        fail("source_date_epoch must be non-negative")
    tag = _dockerfile_attr(ctx, "tag", _dockerfile_default_tag(ctx))
    extension = ".oci.tar" if output_format == "oci" else ".docker.tar"
    archive = declare_output(ctx["label"]["name"] + extension)
    metadata = declare_output("build-metadata.json")
    docker, toolchain_identity, driver = _dockerfile_toolchain(ctx)
    if output_format == "oci" and driver == "docker":
        fail(ctx["label"]["id"] + ": Open Container Initiative export requires a Docker Buildx builder whose driver supports archive export; select a docker-container or remote builder")
    direct_export = output_format == "oci" or (output_format == "docker" and driver and driver != "docker")
    argv = [
        docker,
        "buildx",
        "build",
        "--file", dockerfile,
        "--metadata-file", execution_path(metadata),
        "--progress", "plain",
        "--provenance=false",
        "--sbom=false",
        "--network", _dockerfile_attr(ctx, "network", "default"),
    ]
    if output_format == "docker":
        if not tag:
            fail("tag must not be empty for Docker archive output")
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
    builder = _dockerfile_attr(ctx, "builder", "")
    if builder:
        argv.extend(["--builder", builder])
    platform = _dockerfile_attr(ctx, "platform", "")
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
    if tag:
        argv.extend(["--tag", tag])
    if _dockerfile_attr(ctx, "network", "default") == "host":
        argv.extend(["--allow", "network.host"])
    argv.append(context)

    build_outputs = [metadata] if output_format == "docker" and not direct_export else [archive, metadata]
    run_action(
        argv = argv,
        inputs = inputs,
        outputs = build_outputs,
        cwd = ctx["label"]["package"] or None,
        env = _dockerfile_environment(),
        sandbox = "copied-inputs",
        cacheable = cacheable,
        toolchain_identity = toolchain_identity,
        identifier = ctx["label"]["id"] + ":dockerfile-build",
    )
    if output_format == "docker" and not direct_export:
        run_action(
            argv = [
                docker,
                "image",
                "save",
                "--output", execution_path(archive),
                tag,
            ],
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
        "metadata": metadata,
        "format": output_format,
        "tag": tag,
        "platform": _dockerfile_platform(platform),
        "input_discovery": input_discovery,
        "affected_inputs": inputs,
        "default_output": archive,
    }

_DOCKERFILE_REFERENCES = [
    source_reference(
        "Docker Build",
        "Dockerfile",
        "https://docs.docker.com/reference/dockerfile/",
        "Preserve complete Dockerfile syntax, multi-stage behavior, build arguments, and shell semantics instead of maintaining a partial parser.",
    ),
    source_reference(
        "Docker Buildx",
        "build",
        "https://docs.docker.com/reference/cli/docker/buildx/build/",
        "Use BuildKit for base-image resolution, RUN execution, package installation, and Docker or Open Container Initiative archive export.",
    ),
]

dockerfile_image = target_kind(
    docs = "Builds a Docker or Open Container Initiative image archive from a Dockerfile through BuildKit.",
    attrs = [
        attr("dockerfile", "string", default = "\"Dockerfile\"", docs = "Dockerfile path relative to the target package.", configurable = False),
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
        attr("source_date_epoch", "int", default = "0", docs = "Fixed Unix timestamp used for reproducible exported layers.", configurable = False),
        attr("cacheable", "bool", default = "False", docs = "Enable the Once action cache for direct Open Container Initiative export only when every remote Dockerfile input is immutable.", configurable = False),
    ],
    providers = ["container_image", "dockerfile_image"],
    capabilities = [capability("build", ["archive", "metadata"])],
    tools = [_DOCKERFILE_TOOL],
    examples = [
        example(
            "dockerfile-image-minimal",
            name = "Dockerfile image with a package installation",
            use_when = "Use this when BuildKit should pull a pinned base image, execute Dockerfile RUN instructions, and export a loadable image archive.",
        ),
    ],
    source_references = _DOCKERFILE_REFERENCES,
    impl = _dockerfile_image_impl,
)
