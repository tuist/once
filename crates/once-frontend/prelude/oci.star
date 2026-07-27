_OCI_PYTHON_TOOL = tool("python", executables = ["python3", "python"])
_OCI_LAYER_MEDIA_TYPE = "application/vnd.oci.image.layer.v1.tar"

def _oci_attr(ctx, name, default):
    return _configured_attr(ctx, name, default)

def _oci_mode(value, attribute):
    if len(value) != 4 or value[0] != "0":
        fail(attribute + " must be a four-digit octal string such as 0755")
    mode = 0
    for digit in value.elems():
        number = ord(digit) - ord("0")
        if number < 0 or number > 7:
            fail(attribute + " must be an octal string")
        mode = mode * 8 + number
    return mode

def _oci_archive_path(value, allow_empty = False):
    normalized = value.replace("\\", "/")
    for _ in range(len(normalized) + 1):
        if not normalized.startswith("/"):
            break
        normalized = normalized[1:]
    parts = []
    for part in normalized.split("/"):
        if not part or part == ".":
            continue
        if part == "..":
            fail("container paths must not contain `..`: " + value)
        parts.append(part)
    if not parts and not allow_empty:
        fail("container path must not be empty")
    return "/".join(parts)

def _oci_join(directory, name):
    directory = _oci_archive_path(directory, allow_empty = True)
    name = _oci_archive_path(name)
    return directory + "/" + name if directory else name

def _oci_parent_directories(path):
    parts = path.split("/")
    return ["/".join(parts[:i]) for i in range(1, len(parts))]

def _oci_add_file(files, seen, source, destination, mode, directory_mode, owner_id, group_id, mtime):
    existing = seen.get(destination)
    identity = source + "\x00" + str(mode)
    if existing:
        if existing != identity:
            fail("oci_layer has more than one file at /" + destination)
        return
    seen[destination] = identity
    files.append({
        "kind": "file",
        "source": source,
        "path": destination,
        "mode": mode,
        "directory_mode": directory_mode,
        "owner_id": owner_id,
        "group_id": group_id,
        "mtime": mtime,
    })

def _oci_platform_from_programs(programs):
    os = ""
    architecture = ""
    variant = ""
    for program in programs:
        executable = program.get("executable") or {}
        candidate_os = _once_normalize_os(executable.get("os") or "")
        candidate_architecture = _once_normalize_architecture(executable.get("architecture") or "")
        candidate_variant = executable.get("variant") or ""
        if os and candidate_os and os != candidate_os:
            fail("oci_layer programs must target one operating system")
        if architecture and candidate_architecture and architecture != candidate_architecture:
            fail("oci_layer programs must target one architecture")
        if variant and candidate_variant and variant != candidate_variant:
            fail("oci_layer programs must target one architecture variant")
        os = os or candidate_os
        architecture = architecture or candidate_architecture
        variant = variant or candidate_variant
    return {
        "os": os,
        "architecture": architecture,
        "variant": variant,
    }

def _oci_layer_impl(ctx):
    programs = (ctx.get("deps_by_role") or {}).get("programs") or []
    prebuilt = _oci_attr(ctx, "archive", "")
    if prebuilt:
        if programs or ctx["srcs"]:
            fail("oci_layer archive cannot be combined with programs or srcs")
        source = _package_relative(ctx, prebuilt)
        layer = declare_output(ctx["label"]["name"] + ".tar")
        digest = declare_output(ctx["label"]["name"] + ".sha256")
        copy_path(
            source,
            layer,
            inputs = [source],
            identifier = ctx["label"]["id"] + ":oci-prebuilt-layer",
        )
        write_path(digest, host_file_sha256(workspace_root() + "/" + source) + "\n")
        return {
            "oci_layer": True,
            "label_id": ctx["label"]["id"],
            "target_kind": "oci_layer",
            "blob": layer,
            "sha256": digest,
            "media_type": _OCI_LAYER_MEDIA_TYPE,
            "program_paths": [],
            "platform": {
                "os": _once_normalize_os(_oci_attr(ctx, "os", "")),
                "architecture": _once_normalize_architecture(_oci_attr(ctx, "architecture", "")),
                "variant": _oci_attr(ctx, "variant", ""),
            },
            "affected_inputs": [source],
            "default_output": layer,
        }
    program_dir = _oci_attr(ctx, "program_dir", "/usr/local/bin")
    data_dir = _oci_attr(ctx, "data_dir", "/app")
    program_mode = _oci_mode(_oci_attr(ctx, "program_mode", "0755"), "program_mode")
    file_mode = _oci_mode(_oci_attr(ctx, "file_mode", "0644"), "file_mode")
    directory_mode = _oci_mode(_oci_attr(ctx, "directory_mode", "0755"), "directory_mode")
    owner_id = _oci_attr(ctx, "owner_id", 0)
    group_id = _oci_attr(ctx, "group_id", 0)
    mtime = _oci_attr(ctx, "mtime", 0)
    if owner_id < 0 or group_id < 0 or mtime < 0:
        fail("oci_layer owner_id, group_id, and mtime must be non-negative")

    files = []
    program_paths = []
    seen = {}
    for program in programs:
        executable = program.get("executable") or {}
        source = executable.get("path") or ""
        if not source:
            fail((program.get("label_id") or "program") + " has no executable path")
        destination = _oci_join(program_dir, _basename(source))
        _oci_add_file(
            files,
            seen,
            source,
            destination,
            program_mode,
            directory_mode,
            owner_id,
            group_id,
            mtime,
        )
        if "/" + destination not in program_paths:
            program_paths.append("/" + destination)
        for runtime_file in executable.get("runtime_files") or []:
            _oci_add_file(
                files,
                seen,
                runtime_file,
                _oci_join(data_dir, _basename(runtime_file)),
                file_mode,
                directory_mode,
                owner_id,
                group_id,
                mtime,
            )
    for source in glob(ctx["srcs"]):
        destination = _oci_join(data_dir, _basename(source))
        _oci_add_file(
            files,
            seen,
            source,
            destination,
            file_mode,
            directory_mode,
            owner_id,
            group_id,
            mtime,
        )
    directories = {}
    for file in files:
        for path in _oci_parent_directories(file["path"]):
            directories[path] = True
    entries = [
        {
            "kind": "directory",
            "path": path,
            "mode": directory_mode,
            "directory_mode": directory_mode,
            "owner_id": owner_id,
            "group_id": group_id,
            "mtime": mtime,
        }
        for path in sorted(directories.keys())
    ] + files
    layer = declare_output(ctx["label"]["name"] + ".tar")
    digest = declare_output(ctx["label"]["name"] + ".sha256")
    write_archive(
        entries,
        layer,
        sha256_output = digest,
        format = "tar",
        identifier = ctx["label"]["id"] + ":oci-layer",
    )
    platform = _oci_platform_from_programs(programs)
    selected_os = _once_normalize_os(_oci_attr(ctx, "os", platform["os"]))
    selected_architecture = _once_normalize_architecture(_oci_attr(
        ctx,
        "architecture",
        platform["architecture"],
    ))
    selected_variant = _oci_attr(ctx, "variant", platform["variant"])
    if platform["os"] and selected_os != platform["os"]:
        fail("oci_layer operating system does not match its executable")
    if platform["architecture"] and selected_architecture != platform["architecture"]:
        fail("oci_layer architecture does not match its executable")
    return {
        "oci_layer": True,
        "label_id": ctx["label"]["id"],
        "target_kind": "oci_layer",
        "blob": layer,
        "sha256": digest,
        "media_type": _OCI_LAYER_MEDIA_TYPE,
        "program_paths": program_paths,
        "platform": {
            "os": selected_os,
            "architecture": selected_architecture,
            "variant": selected_variant,
        },
        "affected_inputs": [file["source"] for file in files],
        "default_output": layer,
    }

def _oci_python_toolchain():
    python = host_which("python3")
    version = host_command([python, "--version"], merge_stderr = True).strip()
    return (python, "once.oci.python.v1\x00" + python + "\x00" + version)

def _oci_image_helper_source():
    return """import hashlib
import json
import os
import shutil
import sys

def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")

def digest_bytes(value):
    return hashlib.sha256(value).hexdigest()

def digest_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        while True:
            chunk = handle.read(1024 * 1024)
            if not chunk:
                break
            digest.update(chunk)
    return digest.hexdigest()

def write_bytes(path, value):
    with open(path, "wb") as handle:
        handle.write(value)

spec_path, layout, descriptor_out, manifest_out, config_out = sys.argv[1:]
with open(spec_path, "r", encoding="utf-8") as handle:
    spec = json.load(handle)

layer_descriptors = []
diff_ids = []
for layer in spec["layers"]:
    digest = digest_file(layer["path"])
    size = os.path.getsize(layer["path"])
    shutil.copyfile(layer["path"], os.path.join(layout, "blobs", "sha256", digest))
    layer_descriptors.append({
        "mediaType": layer["media_type"],
        "digest": "sha256:" + digest,
        "size": size,
    })
    diff_ids.append("sha256:" + digest)

runtime = {}
for source, destination in [
    ("entrypoint", "Entrypoint"),
    ("cmd", "Cmd"),
    ("env", "Env"),
]:
    if spec[source]:
        runtime[destination] = spec[source]
for source, destination in [
    ("user", "User"),
    ("working_dir", "WorkingDir"),
    ("stop_signal", "StopSignal"),
]:
    if spec[source]:
        runtime[destination] = spec[source]
if spec["labels"]:
    runtime["Labels"] = spec["labels"]
if spec["exposed_ports"]:
    runtime["ExposedPorts"] = {port: {} for port in spec["exposed_ports"]}

config = {
    "architecture": spec["architecture"],
    "os": spec["os"],
    "config": runtime,
    "rootfs": {"type": "layers", "diff_ids": diff_ids},
    "history": [{"created_by": "once oci_layer"} for _ in layer_descriptors],
}
if spec["variant"]:
    config["variant"] = spec["variant"]
config_bytes = canonical(config)
config_digest = digest_bytes(config_bytes)
write_bytes(config_out, config_bytes)
write_bytes(os.path.join(layout, "blobs", "sha256", config_digest), config_bytes)

manifest = {
    "schemaVersion": 2,
    "mediaType": "application/vnd.oci.image.manifest.v1+json",
    "config": {
        "mediaType": "application/vnd.oci.image.config.v1+json",
        "digest": "sha256:" + config_digest,
        "size": len(config_bytes),
    },
    "layers": layer_descriptors,
}
if spec["annotations"]:
    manifest["annotations"] = spec["annotations"]
manifest_bytes = canonical(manifest)
manifest_digest = digest_bytes(manifest_bytes)
write_bytes(manifest_out, manifest_bytes)
write_bytes(os.path.join(layout, "blobs", "sha256", manifest_digest), manifest_bytes)

platform = {"architecture": spec["architecture"], "os": spec["os"]}
if spec["variant"]:
    platform["variant"] = spec["variant"]
descriptor = {
    "mediaType": "application/vnd.oci.image.manifest.v1+json",
    "digest": "sha256:" + manifest_digest,
    "size": len(manifest_bytes),
    "platform": platform,
}
if spec["tag"]:
    descriptor["annotations"] = {"org.opencontainers.image.ref.name": spec["tag"]}
descriptor_bytes = canonical(descriptor)
write_bytes(descriptor_out, descriptor_bytes)
write_bytes(os.path.join(layout, "index.json"), canonical({"schemaVersion": 2, "manifests": [descriptor]}))
write_bytes(os.path.join(layout, "oci-layout"), canonical({"imageLayoutVersion": "1.0.0"}))
docker_manifest = [{
    "Config": "blobs/sha256/" + config_digest,
    "RepoTags": [spec["tag"]] if spec["tag"] else None,
    "Layers": ["blobs/sha256/" + layer["digest"].split(":", 1)[1] for layer in layer_descriptors],
    "LayerSources": {
        layer["digest"]: {
            "mediaType": layer["mediaType"],
            "size": layer["size"],
            "digest": layer["digest"],
        }
        for layer in layer_descriptors
    },
}]
write_bytes(os.path.join(layout, "manifest.json"), canonical(docker_manifest))
"""

def _oci_layer_platform(layers):
    platform = {"os": "", "architecture": "", "variant": ""}
    for layer in layers:
        candidate = layer.get("platform") or {}
        for key in ["os", "architecture", "variant"]:
            value = candidate.get(key) or ""
            if platform[key] and value and platform[key] != value:
                fail("oci_image layers disagree on platform " + key)
            platform[key] = platform[key] or value
    return platform

def _oci_default_entrypoint(layers):
    programs = []
    for layer in layers:
        programs.extend(layer.get("program_paths") or [])
    return programs if len(programs) == 1 else []

def _oci_env_list(environment):
    return [key + "=" + environment[key] for key in sorted(environment.keys())]

def _oci_image_impl(ctx):
    layers = (ctx.get("deps_by_role") or {}).get("layers") or []
    if not layers:
        fail("oci_image requires at least one layer")
    platform = _oci_layer_platform(layers)
    os = _once_normalize_os(_oci_attr(ctx, "os", platform["os"] or "linux"))
    architecture = _once_normalize_architecture(_oci_attr(
        ctx,
        "architecture",
        platform["architecture"] or host_arch(),
    ))
    variant = _oci_attr(ctx, "variant", platform["variant"])
    if platform["os"] and os != platform["os"]:
        fail("oci_image operating system does not match its executable layer")
    if platform["architecture"] and architecture != platform["architecture"]:
        fail("oci_image architecture does not match its executable layer")
    entrypoint = _oci_attr(ctx, "entrypoint", _oci_default_entrypoint(layers))
    environment = _oci_attr(ctx, "env", {})
    tag = _oci_attr(ctx, "tag", ctx["label"]["name"] + ":latest")
    spec = {
        "architecture": architecture,
        "os": os,
        "variant": variant,
        "entrypoint": entrypoint,
        "cmd": _oci_attr(ctx, "cmd", []),
        "env": _oci_env_list(environment),
        "user": _oci_attr(ctx, "user", ""),
        "working_dir": _oci_attr(ctx, "working_dir", ""),
        "stop_signal": _oci_attr(ctx, "stop_signal", ""),
        "labels": _oci_attr(ctx, "labels", {}),
        "annotations": _oci_attr(ctx, "annotations", {}),
        "exposed_ports": _oci_attr(ctx, "exposed_ports", []),
        "tag": tag,
        "layers": [
            {
                "path": layer["blob"],
                "media_type": layer.get("media_type") or _OCI_LAYER_MEDIA_TYPE,
            }
            for layer in layers
        ],
    }

    helper = declare_output("oci_image.py")
    spec_path = declare_output("image-spec.json")
    layout = declare_output(ctx["label"]["name"] + ".oci")
    descriptor = declare_output("image-descriptor.json")
    manifest = declare_output("image-manifest.json")
    config = declare_output("image-config.json")
    archive = declare_output(ctx["label"]["name"] + ".oci.tar")
    archive_digest = declare_output(ctx["label"]["name"] + ".oci.tar.sha256")
    helper_source = _oci_image_helper_source()
    write_path(helper, helper_source)
    write_path(spec_path, _json_encode(spec) + "\n")
    python, toolchain_identity = _oci_python_toolchain()
    run_action(
        argv = [python, helper, spec_path, layout, descriptor, manifest, config],
        inputs = [helper, spec_path] + [layer["blob"] for layer in layers],
        outputs = [layout, descriptor, manifest, config],
        clean_paths = [layout],
        create_dirs = [layout + "/blobs/sha256"],
        toolchain_identity = toolchain_identity,
        identifier = ctx["label"]["id"] + ":oci-image",
    )
    write_archive(
        [{
            "kind": "tree",
            "source": layout,
            "path": "",
            "mode": 420,
            "directory_mode": 493,
            "owner_id": 0,
            "group_id": 0,
            "mtime": 0,
        }],
        archive,
        sha256_output = archive_digest,
        format = "tar",
        identifier = ctx["label"]["id"] + ":oci-archive",
    )
    return {
        "container_image": True,
        "oci_image": True,
        "label_id": ctx["label"]["id"],
        "target_kind": "oci_image",
        "layout": layout,
        "archive": archive,
        "archive_sha256": archive_digest,
        "descriptor": descriptor,
        "manifest": manifest,
        "config": config,
        "layers": [layer["blob"] for layer in layers],
        "platform": {
            "os": os,
            "architecture": architecture,
            "variant": variant,
        },
        "tag": tag,
        "default_output": archive,
    }

_OCI_REFERENCES = [
    source_reference(
        "Bazel rules_oci",
        "oci_image",
        "https://raw.githubusercontent.com/bazel-contrib/rules_oci/75ff28238f0a135903882f78cc19bf3ba2ef280e/oci/private/image.bzl",
        "Keep layers as independently cacheable artifacts and assemble an Open Container Initiative image layout from typed layer metadata.",
        content_digest = "307e9cd999c5234b7c1ce979682772b1cfaca4453b2cb68ebe19ff04278def02",
    ),
    source_reference(
        "Buck2 OCI image rules",
        "oci_image",
        "https://raw.githubusercontent.com/AaronFriel/buck2-oci-image/d5dd37a9a6d0b64e8ac599d7f647e3ca339439d6/rules/oci/defs.bzl",
        "Expose explicit layer and image providers with layout, descriptor, platform, and archive outputs.",
        content_digest = "4d1cafd4cf74ac6cf1073d58d2f85b7cb9c27fd7ed76ae545c50eb4f3bbf257d",
    ),
]

oci_layer = target_kind(
    docs = "Deterministic uncompressed Open Container Initiative image layer assembled from native executable providers and source files.",
    attrs = [
        attr("archive", "string", docs = "Package-relative uncompressed tar layer to pass through instead of assembling files.", configurable = False),
        attr("os", "string", docs = "Optional operating system metadata, required when a prebuilt layer must constrain the image platform.", configurable = True),
        attr("architecture", "string", docs = "Optional architecture metadata, required when a prebuilt layer must constrain the image platform.", configurable = True),
        attr("variant", "string", docs = "Optional architecture variant metadata.", configurable = True),
        attr("program_dir", "string", default = "\"/usr/local/bin\"", docs = "Container directory where executable dependencies are placed.", configurable = False),
        attr("data_dir", "string", default = "\"/app\"", docs = "Container directory where source and executable runtime files are placed.", configurable = False),
        attr("program_mode", "string", default = "\"0755\"", docs = "Fixed octal mode for executable files.", configurable = False),
        attr("file_mode", "string", default = "\"0644\"", docs = "Fixed octal mode for source files.", configurable = False),
        attr("directory_mode", "string", default = "\"0755\"", docs = "Fixed octal mode for parent directories.", configurable = False),
        attr("owner_id", "int", default = "0", docs = "Numeric owner identifier written to archive headers.", configurable = False),
        attr("group_id", "int", default = "0", docs = "Numeric group identifier written to archive headers.", configurable = False),
        attr("mtime", "int", default = "0", docs = "Fixed Unix timestamp written to archive headers.", configurable = False),
    ],
    deps = [dep("programs", ["once_executable"], "Native executables placed in program_dir.")],
    providers = ["oci_layer"],
    capabilities = [capability("build", ["blob", "sha256"])],
    examples = [
        example(
            "oci-image-minimal",
            name = "Minimal native container image",
            use_when = "Use this to package one native executable into a Docker-compatible image archive.",
        ),
    ],
    source_references = _OCI_REFERENCES,
    impl = _oci_layer_impl,
)

oci_image = target_kind(
    docs = "Docker-compatible Open Container Initiative image layout and archive assembled from ordered layer providers.",
    attrs = [
        attr("os", "string", docs = "Image operating system. Defaults to the layer executable platform or linux.", configurable = True),
        attr("architecture", "string", docs = "Image architecture. Defaults to the layer executable platform or host architecture.", configurable = True),
        attr("variant", "string", docs = "Optional image architecture variant.", configurable = True),
        attr("entrypoint", "list<string>", docs = "Executable and fixed arguments. Defaults to the only packaged executable.", configurable = True),
        attr("cmd", "list<string>", default = "[]", docs = "Default arguments appended to the entrypoint.", configurable = True),
        attr("env", "map<string,string>", default = "{}", docs = "Runtime environment variables.", configurable = True),
        attr("user", "string", docs = "Default runtime user.", configurable = True),
        attr("working_dir", "string", docs = "Default runtime working directory.", configurable = True),
        attr("stop_signal", "string", docs = "Default runtime stop signal.", configurable = True),
        attr("labels", "map<string,string>", default = "{}", docs = "Image configuration labels.", configurable = True),
        attr("annotations", "map<string,string>", default = "{}", docs = "Image manifest annotations.", configurable = True),
        attr("exposed_ports", "list<string>", default = "[]", docs = "Exposed ports such as 8080/tcp.", configurable = True),
        attr("tag", "string", docs = "Archive reference name. Defaults to the target name with latest.", configurable = True),
    ],
    deps = [dep("layers", ["oci_layer"], "Ordered filesystem layers from base to top.")],
    providers = ["container_image", "oci_image"],
    capabilities = [capability("build", ["archive", "layout", "descriptor", "manifest", "config"])],
    tools = [_OCI_PYTHON_TOOL],
    examples = [
        example(
            "oci-image-minimal",
            name = "Minimal native container image",
            use_when = "Use this to assemble and load a Docker-compatible image from a native executable layer.",
        ),
    ],
    source_references = _OCI_REFERENCES,
    impl = _oci_image_impl,
)
