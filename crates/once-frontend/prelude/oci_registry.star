_OCI_CRANE_TOOL = tool("crane", executables = ["crane"])

def _oci_registry_attr(ctx, name, default):
    return _configured_attr(ctx, name, default)

def _oci_registry_environment():
    environment = {}
    for name in [
        "DOCKER_CONFIG",
        "HOME",
        "PATH",
        "REGISTRY_AUTH_FILE",
        "SSL_CERT_DIR",
        "SSL_CERT_FILE",
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "NO_PROXY",
        "https_proxy",
        "http_proxy",
        "no_proxy",
        "XDG_RUNTIME_DIR",
    ]:
        value = host_env(name)
        if value:
            environment[name] = value
    return environment

def _oci_crane(ctx):
    crane = _resolve_host_executable("crane")
    if not crane:
        _diagnostic_fail("required_tool_not_found", None, ctx["label"]["id"] + ": crane was not found", "Install crane (for example with mise use crane) and make sure it is on PATH")
    version = host_command([crane, "version"]).strip()
    return (crane, "once.oci.crane.v1\x00" + crane + "\x00" + version)

def _oci_pull_impl(ctx):
    image = _oci_registry_attr(ctx, "image", "")
    digest = _oci_registry_attr(ctx, "digest", "")
    if not image or image.startswith("-") or " " in image or "@" in image or image.split("/")[-1].count(":") > 0:
        _diagnostic_fail("oci_pull_invalid_image", "image", "image must be a repository without a tag or digest, such as gcr.io/distroless/static", "Set image to the repository and put the content digest in digest")
    if len(digest) != 71 or not digest.startswith("sha256:"):
        _diagnostic_fail("oci_pull_invalid_digest", "digest", "digest must be a sha256 content digest of the form sha256:<64 hex characters>", "Resolve it with: crane digest " + image + ":<tag>")
    platform = _oci_registry_attr(ctx, "platform", "") or "linux/" + _once_normalize_architecture(host_arch())
    parts = platform.split("/")
    if platform != "all" and (len(parts) < 2 or len(parts) > 3):
        _diagnostic_fail("oci_pull_invalid_platform", "platform", "platform must use os/architecture[/variant] form, or `all` to keep every platform", "Set platform to a value such as linux/amd64")
    layout = declare_output("layout")
    archive = declare_output(ctx["label"]["name"] + ".oci.tar")
    archive_digest = declare_output(ctx["label"]["name"] + ".oci.tar.sha256")
    reference = image + "@" + digest
    provider = {
        "container_image": True,
        "oci_pull": True,
        "label_id": ctx["label"]["id"],
        "target_kind": "oci_pull",
        "layout": layout,
        "archive": archive,
        "archive_sha256": archive_digest,
        "reference": reference,
        "tag": reference,
        "tags": [reference],
        "default_output": archive,
    }
    if platform != "all":
        provider["platform"] = {"os": parts[0], "architecture": parts[1], "variant": parts[2] if len(parts) == 3 else ""}
        provider["platforms"] = [platform]
    if ctx["capability"] == "metadata":
        return provider
    crane, identity = _oci_crane(ctx)
    argv = [crane, "pull", "--format", "oci"]
    if platform != "all":
        argv.extend(["--platform", platform])
    if _oci_registry_attr(ctx, "insecure", False):
        argv.append("--insecure")
    argv.extend([reference, layout])
    run_action(
        display_name = "Pull container image · " + ctx["label"]["name"],
        argv = argv,
        outputs = [layout],
        clean_paths = [layout],
        env = _oci_registry_environment(),
        network = "unrestricted",
        sandbox = "copied-inputs",
        cacheable = True,
        depends_on_prior_actions = False,
        toolchain_identity = identity,
        identifier = ctx["label"]["id"] + ":oci-pull",
    )
    write_archive(
        [{"kind": "tree", "source": layout, "path": "", "mode": 420, "directory_mode": 493, "owner_id": 0, "group_id": 0, "mtime": 0}],
        archive,
        sha256_output = archive_digest,
        format = "tar",
        inputs = [layout],
        identifier = ctx["label"]["id"] + ":oci-pull-archive",
    )
    return provider

def _oci_import_impl(ctx):
    source = _oci_registry_attr(ctx, "archive", "")
    lowered = source.lower()
    if not source or not (lowered.endswith(".tar") or lowered.endswith(".tar.gz") or lowered.endswith(".tgz")):
        _diagnostic_fail("oci_import_invalid_archive", "archive", "archive must be a .tar, .tar.gz, or .tgz file in Open Container Initiative image layout form", "Set archive to a file such as images/base.oci.tar, produced by docker save, docker buildx build --output type=oci, or crane pull --format oci")
    path = _package_relative(ctx, source)
    if not host_file_exists(workspace_root() + "/" + path):
        _diagnostic_fail("oci_import_archive_not_found", "archive", ctx["label"]["id"] + ": " + path + " does not exist", "Create the archive or fix the archive path")
    layout = declare_output("layout")
    archive = declare_output(ctx["label"]["name"] + ".oci.tar")
    archive_digest = declare_output(ctx["label"]["name"] + ".oci.tar.sha256")
    provider = {
        "container_image": True,
        "oci_import": True,
        "label_id": ctx["label"]["id"],
        "target_kind": "oci_import",
        "layout": layout,
        "archive": archive,
        "archive_sha256": archive_digest,
        "tag": "",
        "tags": [],
        "default_output": archive,
    }
    if ctx["capability"] == "metadata":
        return provider
    tar = _resolve_host_executable("tar")
    if not tar:
        _diagnostic_fail("required_tool_not_found", None, ctx["label"]["id"] + ": tar was not found", "Install tar and make sure it is on PATH")
    version = host_command([tar, "--version"], check = False).split("\n")[0].strip()
    staging = ctx["scratch_dir"] + "/import"
    run_action(
        display_name = "Import container image · " + ctx["label"]["name"],
        source_files = _action_source_files([path]),
        argv = [tar, "-xf", path, "-C", staging],
        inputs = [path],
        outputs = [staging],
        clean_paths = [staging],
        create_dirs = [staging],
        sandbox = "copied-inputs",
        cacheable = True,
        depends_on_prior_actions = False,
        toolchain_identity = "once.oci.tar.v1\x00" + tar + "\x00" + version,
        identifier = ctx["label"]["id"] + ":oci-import",
    )
    copy_path(staging, layout, kind = "tree", inputs = [staging], identifier = ctx["label"]["id"] + ":oci-import-layout", source_files = _action_source_files([staging]))
    write_archive(
        [{"kind": "tree", "source": layout, "path": "", "mode": 420, "directory_mode": 493, "owner_id": 0, "group_id": 0, "mtime": 0}],
        archive,
        sha256_output = archive_digest,
        format = "tar",
        inputs = [layout],
        identifier = ctx["label"]["id"] + ":oci-import-archive",
    )
    return provider

def _oci_single_image(ctx):
    images = (ctx.get("deps_by_role") or {}).get("image") or []
    if len(images) != 1:
        _diagnostic_fail("oci_requires_one_image", "image", ctx["label"]["id"] + " needs exactly one image dependency", "Set the image dependency role to one container image target")
    return images[0]

def _oci_load_impl(ctx):
    images = (ctx.get("deps_by_role") or {}).get("image") or []
    if not images:
        _diagnostic_fail("oci_requires_one_image", "image", ctx["label"]["id"] + " needs at least one image dependency", "Set the image dependency role to one or more container image targets")
    archives = []
    tags = []
    for image in images:
        if not image.get("archive"):
            _diagnostic_fail("oci_load_without_archive", "image", (image.get("label_id") or "image") + " does not provide an archive", "Depend on an image target that exports an archive")
        archives.append(image["archive"])
        tags.extend(image.get("tags") or [])
    daemon = _oci_registry_attr(ctx, "daemon", "docker")
    provider = {
        "oci_load": True,
        "label_id": ctx["label"]["id"],
        "target_kind": "oci_load",
        "images": [image.get("label_id") for image in images],
        "daemon": daemon,
        "tags": tags,
        "default_output": archives[0],
    }
    if ctx["capability"] != "run":
        return provider
    executable = _resolve_host_executable(daemon)
    if not executable:
        _diagnostic_fail("required_tool_not_found", None, ctx["label"]["id"] + ": " + daemon + " was not found", "Install " + daemon + " or set daemon to an installed container engine")
    for index, archive in enumerate(archives):
        run_action(
            display_name = "Load container image · " + ctx["label"]["name"],
            source_files = _action_source_files([archive]),
            argv = [executable, "load", "--input", archive],
            inputs = [archive],
            sandbox = "off",
            cacheable = False,
            inherit_parent_env = True,
            identifier = ctx["label"]["id"] + ":oci-load" + (":" + str(index) if len(archives) > 1 else ""),
        )
    return provider

def _oci_valid_repository(repository):
    if not repository or repository.startswith("/") or repository.startswith("-") or repository.endswith("/") or "@" in repository or " " in repository:
        return False
    for character in repository.elems():
        if ord(character) < 33 or ord(character) > 126:
            return False
    return ":" not in repository.split("/")[-1]

def _oci_push_impl(ctx):
    image = _oci_single_image(ctx)
    layout = image.get("layout")
    if not layout:
        _diagnostic_fail("oci_push_without_layout", "image", (image.get("label_id") or "image") + " does not provide an image layout", "Depend on an image target that exports a layout")
    repository = _oci_registry_attr(ctx, "repository", "")
    if not _oci_valid_repository(repository):
        _diagnostic_fail("oci_push_invalid_repository", "repository", "repository must name a registry repository without a tag or digest, such as registry.example.com/team/app", "Put tags in remote_tags")
    tags = _oci_registry_attr(ctx, "remote_tags", ["latest"])
    if not tags:
        _diagnostic_fail("oci_push_without_tags", "remote_tags", "remote_tags must contain at least one tag", "Set remote_tags to a list such as [\"latest\"]")
    for tag in tags:
        if not _oci_valid_tag(tag):
            _diagnostic_fail("oci_push_invalid_tag", "remote_tags", "`" + tag + "` is not a valid tag", "A tag is up to 128 letters, digits, underscores, periods, or dashes, and does not start with a period or dash")
    provider = {
        "oci_push": True,
        "label_id": ctx["label"]["id"],
        "target_kind": "oci_push",
        "repository": repository,
        "remote_tags": tags,
        "image": image.get("label_id"),
        "default_output": layout,
    }
    if ctx["capability"] != "run":
        return provider
    crane, identity = _oci_crane(ctx)
    cosign = ""
    cosign_identity = ""
    sign = _oci_registry_attr(ctx, "sign", False)
    if sign:
        cosign = _resolve_host_executable("cosign")
        if not cosign:
            _diagnostic_fail("required_tool_not_found", "sign", ctx["label"]["id"] + ": cosign was not found", "Install cosign (for example with mise use cosign), or set sign = false")
        cosign_identity = "once.oci.cosign.v1\x00" + cosign + "\x00" + host_command([cosign, "version"], merge_stderr = True).strip()
    digest_file = declare_output("push/digest.txt")
    signature = declare_output("push/signature.txt")
    expand_actions(
        implementation = "oci_push_plan",
        inputs = [layout + "/index.json"],
        outputs = [digest_file] + ([signature] if sign else []),
        args = {
            "label": ctx["label"]["id"],
            "layout": layout,
            "view": declare_output("push/view"),
            "repository": repository,
            "tags": tags,
            "crane": crane,
            "identity": identity,
            "insecure": _oci_registry_attr(ctx, "insecure", False),
            "env": _oci_registry_environment(),
            "digest_file": digest_file,
            "sign": sign,
            "cosign": cosign,
            "cosign_identity": cosign_identity,
            "cosign_key": _oci_registry_attr(ctx, "cosign_key", ""),
            "cosign_args": _oci_registry_attr(ctx, "cosign_args", []),
            "signature": signature,
        },
    )
    provider["digest_file"] = digest_file
    if sign:
        provider["signature"] = signature
    return provider

def oci_push_plan(ctx):
    spec = ctx["args"]
    root = workspace_root() + "/"
    label = spec["label"]
    entries = json_decode(host_file_read(root + spec["layout"] + "/index.json")).get("manifests") or []
    if not entries:
        _diagnostic_fail("oci_push_empty_layout", "image", label + ": the image layout has no manifest", "Rebuild the image target")
    first = entries[0]
    for entry in entries:
        if entry["digest"] != first["digest"]:
            _diagnostic_fail("oci_push_ambiguous_layout", "image", label + ": the image layout holds several different images", "Depend on a single image or on an oci_index")
    view = spec["view"]
    descriptor = {key: first[key] for key in ["mediaType", "digest", "size", "platform"] if key in first}
    write_path(view + "/index.json", _json_encode({"schemaVersion": 2, "manifests": [descriptor]}))
    write_path(view + "/oci-layout", _json_encode({"imageLayoutVersion": "1.0.0"}))
    link_path(spec["layout"] + "/blobs", view + "/blobs", identifier = label + ":oci-push-blobs")
    insecure = ["--insecure"] if spec["insecure"] else []
    first_reference = spec["repository"] + ":" + spec["tags"][0]
    pinned = spec["repository"] + "@" + first["digest"]
    run_action(
        display_name = "Push container image · " + label,
        source_files = _action_source_files([spec["layout"], view + "/index.json", view + "/oci-layout", view + "/blobs"]),
        argv = [spec["crane"], "push"] + insecure + [view, first_reference],
        inputs = [spec["layout"], view + "/index.json", view + "/oci-layout", view + "/blobs"],
        outputs = [spec["digest_file"]],
        clean_paths = [spec["digest_file"]],
        stdout = spec["digest_file"],
        env = spec["env"],
        sandbox = "off",
        cacheable = False,
        inherit_parent_env = True,
        toolchain_identity = spec["identity"],
        identifier = label + ":oci-push",
    )
    for tag in spec["tags"][1:]:
        run_action(
            display_name = "Tag container image · " + tag,
            argv = [spec["crane"], "tag"] + insecure + [pinned, tag],
            env = spec["env"],
            sandbox = "off",
            cacheable = False,
            inherit_parent_env = True,
            toolchain_identity = spec["identity"],
            identifier = label + ":oci-tag:" + tag,
        )
    if spec["sign"]:
        argv = [spec["cosign"], "sign", "--yes"]
        if spec["cosign_key"]:
            argv.extend(["--key", spec["cosign_key"]])
        if spec["insecure"]:
            argv.append("--allow-insecure-registry")
        argv.extend(spec["cosign_args"])
        argv.append(pinned)
        run_action(
            display_name = "Sign container image · " + label,
            argv = argv,
            outputs = [spec["signature"]],
            clean_paths = [spec["signature"]],
            stdout = spec["signature"],
            env = spec["env"],
            sandbox = "off",
            cacheable = False,
            inherit_parent_env = True,
            toolchain_identity = spec["cosign_identity"],
            identifier = label + ":oci-sign",
        )
    return None

_OCI_REGISTRY_REFERENCES = [
    source_reference(
        "Bazel rules_oci",
        "oci_pull oci_load oci_push",
        "https://github.com/bazel-contrib/rules_oci/tree/main/oci/private",
        "Pull base images by content digest, load an image into a local daemon, and push exact image bytes to a registry.",
    ),
    source_reference(
        "crane",
        "pull push tag",
        "https://github.com/google/go-containerregistry/blob/main/cmd/crane/doc/crane.md",
        "Transfer Open Container Initiative image layouts and indexes to and from registries without changing their digests.",
    ),
]

oci_pull = target_kind(
    docs = "Pulls one image by content digest into an Open Container Initiative image layout that other container targets can extend, copy from, or publish. Needs crane, not Docker.",
    attrs = [
        attr("image", "string", required = True, docs = "Registry repository without tag or digest, such as gcr.io/distroless/static.", configurable = False),
        attr("digest", "string", required = True, docs = "Content digest of the image or index, as sha256:<64 hex characters>. Pulling by digest makes the result immutable, so Once caches it.", configurable = False),
        attr("platform", "string", docs = "Platform to keep, as os/architecture[/variant], or `all` to keep every platform of an index. Defaults to linux on the host architecture.", configurable = False),
        attr("insecure", "bool", default = "False", docs = "Allow a registry without TLS.", configurable = False),
    ],
    providers = ["container_image", "oci_pull"],
    capabilities = [capability("build", ["layout", "archive"])],
    tools = [_OCI_CRANE_TOOL],
    examples = [
        example(
            "oci-pull-and-extend",
            name = "Extend a digest-pinned base image",
            use_when = "Use this to start an image from a pinned registry base and add a native executable layer.",
        ),
    ],
    source_references = _OCI_REGISTRY_REFERENCES,
    impl = _oci_pull_impl,
)

oci_import = target_kind(
    docs = "Imports an existing image archive in Open Container Initiative layout form (docker save, docker buildx build --output type=oci, crane pull --format oci) as a container image that other container targets can extend, load, or publish. The archive is a tracked input, so changing it rebuilds dependents.",
    attrs = [
        attr("archive", "string", required = True, docs = "Package-relative path of the .tar, .tar.gz, or .tgz image layout archive.", configurable = False),
    ],
    providers = ["container_image", "oci_import"],
    capabilities = [capability("build", ["layout", "archive"])],
    tools = [tool("tar", executables = ["tar"])],
    examples = [
        example(
            "oci-import",
            name = "Import an image archive",
            use_when = "Use this to bring an image exported by another build into the Once graph.",
        ),
    ],
    source_references = _OCI_REGISTRY_REFERENCES,
    impl = _oci_import_impl,
)

oci_load = target_kind(
    docs = "Loads one or more image archives into a local container engine when run.",
    attrs = [
        attr("daemon", "string", default = "\"docker\"", docs = "Container engine executable that loads the archive.", configurable = False, allowed_values = ["docker", "podman"]),
    ],
    deps = [dep("image", ["container_image"], "The images whose archives are loaded, in order. List several to load a bundle with one command.")],
    providers = ["oci_load"],
    capabilities = [capability("build", []), capability("run", [])],
    tools = [tool("docker", executables = ["docker"])],
    examples = [
        example(
            "oci-pull-and-extend",
            name = "Extend a digest-pinned base image",
            use_when = "Use this to load a built image into the local engine with once run.",
        ),
    ],
    source_references = _OCI_REGISTRY_REFERENCES,
    impl = _oci_load_impl,
)

oci_push = target_kind(
    docs = "Pushes an image or multi-platform index to a registry when run, preserving its manifest digest. Needs crane, not Docker.",
    attrs = [
        attr("repository", "string", required = True, docs = "Registry repository to push to, without tag or digest.", configurable = False),
        attr("remote_tags", "list<string>", default = "[\"latest\"]", docs = "Tags to publish.", configurable = False),
        attr("insecure", "bool", default = "False", docs = "Allow a registry without TLS. Registry credentials come from the Docker configuration.", configurable = False),
        attr("sign", "bool", default = "False", docs = "Sign the pushed digest with cosign after publishing. Needs cosign.", configurable = False),
        attr("cosign_args", "list<string>", default = "[]", docs = "Extra arguments passed to cosign sign, such as --tlog-upload=false for a private signing setup.", configurable = False),
        attr("cosign_key", "string", docs = "Key reference passed to cosign: a key file, a KMS URI, or empty for keyless signing. A key password comes from COSIGN_PASSWORD in the environment.", configurable = False),
    ],
    deps = [dep("image", ["container_image"], "The image or index whose layout is pushed.")],
    providers = ["oci_push"],
    capabilities = [capability("build", []), capability("run", [])],
    tools = [_OCI_CRANE_TOOL],
    examples = [
        example(
            "oci-pull-and-extend",
            name = "Extend a digest-pinned base image",
            use_when = "Use this to publish a built image with once run.",
        ),
    ],
    source_references = _OCI_REGISTRY_REFERENCES,
    impl = _oci_push_impl,
)
