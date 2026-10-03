_OCI_LAYER_MEDIA_TYPE = "application/vnd.oci.image.layer.v1.tar"
_OCI_GZIP_LAYER_MEDIA_TYPE = "application/vnd.oci.image.layer.v1.tar+gzip"

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

def _oci_file_name(path):
    name = path.split("/")[-1]
    if "\\" in name:
        _diagnostic_fail("oci_layer_backslash_in_path", None, "file name `" + name + "` contains a backslash, which archive paths cannot carry", "Rename the file")
    return name

def _oci_archive_path(value, allow_empty = False):
    if "\\" in value:
        _diagnostic_fail("oci_layer_backslash_in_path", None, "container path `" + value + "` contains a backslash, which archive paths cannot carry", "Rename the file or choose a destination without a backslash")
    normalized = value
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
    compress = _oci_attr(ctx, "compress", "none")
    symlinks = _oci_attr(ctx, "symlinks", {})
    if prebuilt:
        if programs or ctx["srcs"] or symlinks:
            fail("oci_layer archive cannot be combined with programs, srcs, or symlinks")
        if compress != "none":
            _diagnostic_fail("oci_layer_prebuilt_compression", "compress", "a prebuilt archive layer is stored as given and cannot be compressed here", "Remove compress, or pass the files as programs and srcs so Once builds the archive")
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
        destination = _oci_join(program_dir, _oci_file_name(source))
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
                _oci_join(data_dir, _oci_file_name(runtime_file)),
                file_mode,
                directory_mode,
                owner_id,
                group_id,
                mtime,
            )
    for source in glob(ctx["srcs"]):
        destination = _oci_join(data_dir, _oci_file_name(source))
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
    links = []
    for link in sorted(symlinks.keys()):
        destination = _oci_archive_path(link)
        if destination in seen:
            fail("oci_layer has more than one entry at /" + destination)
        seen[destination] = "symlink"
        links.append({
            "kind": "symlink",
            "path": destination,
            "target": symlinks[link],
            "owner_id": owner_id,
            "group_id": group_id,
            "mtime": mtime,
        })
    directories = {}
    for file in files + links:
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
    ] + files + links
    gzip = compress == "gzip"
    layer = declare_output(ctx["label"]["name"] + (".tar.gz" if gzip else ".tar"))
    digest = declare_output(ctx["label"]["name"] + ".sha256")
    diff_id = declare_output(ctx["label"]["name"] + ".diff-id.sha256") if gzip else digest
    archive_options = {"sha256_output": digest, "format": "tar.gz" if gzip else "tar"}
    if gzip:
        archive_options["uncompressed_sha256_output"] = diff_id
    write_archive(
        entries,
        layer,
        identifier = ctx["label"]["id"] + ":oci-layer",
        **archive_options
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
        "diff_id": diff_id,
        "annotations": _oci_attr(ctx, "annotations", {}),
        "media_type": _OCI_GZIP_LAYER_MEDIA_TYPE if gzip else _OCI_LAYER_MEDIA_TYPE,
        "program_paths": program_paths,
        "platform": {
            "os": selected_os,
            "architecture": selected_architecture,
            "variant": selected_variant,
        },
        "affected_inputs": [file["source"] for file in files],
        "default_output": layer,
    }

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

def _oci_utf8_length(text):
    total = 0
    for character in text.elems():
        code = ord(character)
        total += 1 if code < 128 else (2 if code < 2048 else (3 if code < 65536 else 4))
    return total

def _oci_ascii_digits(text):
    if not text:
        return False
    for character in text.elems():
        if character < "0" or character > "9":
            return False
    return True

def _oci_valid_timestamp(value):
    if len(value) < 20 or value[4] != "-" or value[7] != "-" or value[10] != "T" or value[13] != ":" or value[16] != ":":
        return False
    for part in [value[0:4], value[5:7], value[8:10], value[11:13], value[14:16], value[17:19]]:
        if not _oci_ascii_digits(part):
            return False
    year = int(value[0:4])
    month = int(value[5:7])
    day = int(value[8:10])
    leap = year % 4 == 0 and (year % 100 != 0 or year % 400 == 0)
    days = [31, 29 if leap else 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    if month < 1 or month > 12 or day < 1 or day > days[month - 1] or int(value[11:13]) > 23 or int(value[14:16]) > 59 or int(value[17:19]) > 59:
        return False
    rest = value[19:]
    if rest.startswith("."):
        digits = 0
        for character in rest[1:].elems():
            if character < "0" or character > "9":
                break
            digits += 1
        if digits == 0 or digits > 9:
            return False
        rest = rest[1 + digits:]
    if rest == "Z":
        return True
    return len(rest) == 6 and rest[0] in ["+", "-"] and rest[3] == ":" and _oci_ascii_digits(rest[1:3]) and _oci_ascii_digits(rest[4:6]) and int(rest[1:3]) <= 23 and int(rest[4:6]) <= 59

def _oci_valid_tag(tag):
    if not tag or len(tag) > 128:
        return False
    for index, character in enumerate(tag.elems()):
        letter = (character >= "a" and character <= "z") or (character >= "A" and character <= "Z")
        digit = character >= "0" and character <= "9"
        if not (letter or digit or character == "_" or (index > 0 and character in [".", "-"])):
            return False
    return True

def _oci_reference(reference):
    name = reference
    tag = "latest"
    if name.rfind(":") > name.rfind("/"):
        tag = name[name.rfind(":") + 1:]
        name = name[:name.rfind(":")]
    parts = name.split("/")
    if len(parts) == 1 or not ("." in parts[0] or ":" in parts[0] or parts[0] == "localhost"):
        name = "docker.io/" + ("library/" if len(parts) == 1 else "") + name
    return (name + ":" + tag, tag)

def _oci_indexed_descriptors(descriptor, tags):
    if not tags:
        return [descriptor]
    entries = []
    for tag in tags:
        qualified, short = _oci_reference(tag)
        entry = dict(descriptor)
        entry["annotations"] = {"io.containerd.image.name": qualified, "org.opencontainers.image.ref.name": short}
        entries.append(entry)
    return entries

def _oci_hex(digest):
    return digest.split(":", 1)[1] if ":" in digest else digest

def _oci_blob_path(digest):
    return "blobs/sha256/" + _oci_hex(digest)

def _oci_tag_list(ctx):
    tags = []
    primary = _oci_attr(ctx, "tag", ctx["label"]["name"] + ":latest")
    if primary:
        tags.append(primary)
    for extra in _oci_attr(ctx, "tags", []):
        if extra and extra not in tags:
            tags.append(extra)
    return tags

def _oci_image_impl(ctx):
    roles = ctx.get("deps_by_role") or {}
    layers = roles.get("layers") or []
    bases = roles.get("base") or []
    if len(bases) > 1:
        _diagnostic_fail("oci_image_multiple_bases", "base", "oci_image accepts at most one base image", "Keep one target in the base dependency role")
    base = bases[0] if bases else None
    if base != None and not base.get("layout"):
        _diagnostic_fail("oci_image_base_without_layout", "base", (base.get("label_id") or "base") + " does not provide an image layout", "Use a base that exports a layout, such as an oci_pull, oci_image, or instruction-mode dockerfile_image target")
    if not layers and base == None:
        _diagnostic_fail("oci_image_without_layers", "layers", "oci_image requires at least one layer or a base image", "Add a layers dependency or a base dependency")
    platform = _oci_layer_platform(layers)
    base_platform = (base.get("platform") or {}) if base else {}
    os = _once_normalize_os(_oci_attr(ctx, "os", platform["os"] or base_platform.get("os") or "linux"))
    architecture = _once_normalize_architecture(_oci_attr(
        ctx,
        "architecture",
        platform["architecture"] or base_platform.get("architecture") or host_arch(),
    ))
    variant = _oci_attr(ctx, "variant", platform["variant"] or base_platform.get("variant") or "")
    if platform["os"] and os != platform["os"]:
        _diagnostic_fail("oci_image_platform_mismatch", "os", "oci_image operating system does not match its executable layer", "Set os to " + platform["os"] + " or remove it")
    if platform["architecture"] and architecture != platform["architecture"]:
        _diagnostic_fail("oci_image_platform_mismatch", "architecture", "oci_image architecture does not match its executable layer", "Set architecture to " + platform["architecture"] + " or remove it")
    tags = _oci_tag_list(ctx)
    name = ctx["label"]["name"]
    layout = declare_output(name + ".oci")
    descriptor = declare_output("image-descriptor.json")
    manifest = declare_output("image-manifest.json")
    config = declare_output("image-config.json")
    archive = declare_output(name + ".oci.tar")
    archive_digest = declare_output(name + ".oci.tar.sha256")
    entrypoint = _oci_attr(ctx, "entrypoint", None)
    if entrypoint == None and base == None:
        entrypoint = _oci_default_entrypoint(layers)
    created = _oci_attr(ctx, "created", "")
    if created and not _oci_valid_timestamp(created):
        _diagnostic_fail("oci_image_invalid_created", "created", "created must be an RFC 3339 timestamp such as 2026-01-02T03:04:05Z, got " + created, "Set created to a valid timestamp or remove it")
    spec = {
        "label": ctx["label"]["id"],
        "layout": layout,
        "descriptor": descriptor,
        "manifest": manifest,
        "config": config,
        "archive": archive,
        "archive_sha256": archive_digest,
        "architecture": architecture,
        "os": os,
        "variant": variant,
        "entrypoint": entrypoint,
        "cmd": _oci_attr(ctx, "cmd", None),
        "env": _oci_attr(ctx, "env", {}),
        "user": _oci_attr(ctx, "user", ""),
        "working_dir": _oci_attr(ctx, "working_dir", ""),
        "stop_signal": _oci_attr(ctx, "stop_signal", ""),
        "labels": _oci_attr(ctx, "labels", {}),
        "annotations": _oci_attr(ctx, "annotations", {}),
        "exposed_ports": _oci_attr(ctx, "exposed_ports", []),
        "volumes": _oci_attr(ctx, "volumes", []),
        "author": _oci_attr(ctx, "author", ""),
        "created": created,
        "tags": tags,
        "base": base["layout"] if base else "",
        "layers": [
            {
                "blob": layer["blob"],
                "sha256": layer["sha256"],
                "diff_id": layer.get("diff_id") or layer["sha256"],
                "annotations": layer.get("annotations") or {},
                "media_type": layer.get("media_type") or _OCI_LAYER_MEDIA_TYPE,
            }
            for layer in layers
        ],
    }
    planner_inputs = _unique([layer["sha256"] for layer in layers] + [layer.get("diff_id") or layer["sha256"] for layer in layers]) + ([base["layout"] + "/index.json"] if base else [])
    expand_actions(
        implementation = "oci_image_plan",
        inputs = planner_inputs,
        outputs = [layout + "/index.json", layout + "/oci-layout", layout + "/manifest.json", descriptor, manifest, config, archive, archive_digest],
        args = spec,
    )
    onbuild_source = (base.get("onbuild_source") or ({"reference": base["reference"], "platforms": base.get("platforms") or []} if base.get("reference") else None)) if base else None
    return {
        "container_image": True,
        "oci_image": True,
        "onbuild": bool(base and base.get("onbuild")),
        "onbuild_source": onbuild_source,
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
        "tag": tags[0] if tags else "",
        "tags": tags,
        "default_output": archive,
    }

def _oci_leaf_manifests(root, layout, index, label, depth = 0):
    leaves = []
    for entry in index.get("manifests") or []:
        if "index" in (entry.get("mediaType") or "") or "manifest.list" in (entry.get("mediaType") or ""):
            if depth >= 3:
                _diagnostic_fail("oci_base_nested_index", "base", label + ": image indexes nest too deeply", "Use a base layout with at most a few levels of indexes")
            nested = json_decode(host_file_read(root + layout + "/" + _oci_blob_path(entry["digest"])))
            leaves.extend(_oci_leaf_manifests(root, layout, nested, label, depth + 1))
        else:
            leaves.append(entry)
    return leaves

def _oci_pick_manifest(leaves, os, architecture, variant, label):
    if not leaves:
        _diagnostic_fail("oci_base_empty", "base", label + ": the base layout has no manifest", "Rebuild the base image")
    if len(leaves) == 1:
        return leaves[0]
    for entry in leaves:
        candidate = entry.get("platform") or {}
        if candidate.get("os") == os and candidate.get("architecture") == architecture and (not variant or candidate.get("variant") == variant):
            return entry
    _diagnostic_fail("oci_base_platform_not_found", "base", label + ": the base image has no " + os + "/" + architecture + " manifest", "Select a base for this platform or set os and architecture")

def _oci_merge_env(base_env, overrides):
    result = []
    replaced = {}
    for entry in base_env:
        key = entry.split("=", 1)[0]
        if key in overrides:
            result.append(key + "=" + overrides[key])
            replaced[key] = True
        else:
            result.append(entry)
    for key in sorted(overrides.keys()):
        if key not in replaced:
            result.append(key + "=" + overrides[key])
    return result

def _oci_layout_archive_entries(layout, written):
    entries = [
        {"kind": "directory", "path": "blobs", "mode": 493, "owner_id": 0, "group_id": 0, "mtime": 0},
        {"kind": "directory", "path": "blobs/sha256", "mode": 493, "owner_id": 0, "group_id": 0, "mtime": 0},
    ]
    for path in sorted(written):
        entries.append({"kind": "file", "source": path, "path": path[len(layout) + 1:], "mode": 420, "owner_id": 0, "group_id": 0, "mtime": 0})
    return entries

def oci_image_plan(ctx):
    spec = ctx["args"]
    root = workspace_root() + "/"
    layout = spec["layout"]
    label = spec["label"]
    blobs = {}
    new_layers = []
    new_diff_ids = []
    for layer in spec["layers"]:
        digest = host_file_read(root + layer["sha256"]).strip()
        new_diff_ids.append("sha256:" + host_file_read(root + layer["diff_id"]).strip())
        descriptor = {
            "mediaType": layer["media_type"],
            "digest": "sha256:" + digest,
            "size": host_file_size(root + layer["blob"]),
        }
        if layer.get("annotations"):
            descriptor["annotations"] = layer["annotations"]
        new_layers.append(descriptor)
        blobs[digest] = layer["blob"]
    base_config = {}
    base_layers = []
    if spec["base"]:
        index = json_decode(host_file_read(root + spec["base"] + "/index.json"))
        leaves = _oci_leaf_manifests(root, spec["base"], index, label)
        chosen = _oci_pick_manifest(leaves, spec["os"], spec["architecture"], spec["variant"], label)
        base_manifest = json_decode(host_file_read(root + spec["base"] + "/" + _oci_blob_path(chosen["digest"])))
        base_config = json_decode(host_file_read(root + spec["base"] + "/" + _oci_blob_path(base_manifest["config"]["digest"])))
        for field, wanted in [("os", spec["os"]), ("architecture", spec["architecture"]), ("variant", spec["variant"])]:
            actual = base_config.get(field) or ""
            if actual and wanted and actual != wanted:
                _diagnostic_fail("oci_base_platform_mismatch", field, label + ": the base image is " + (base_config.get("os") or "?") + "/" + (base_config.get("architecture") or "?") + " but this image is " + spec["os"] + "/" + spec["architecture"], "Pull a base for " + spec["os"] + "/" + spec["architecture"] + " or set os and architecture to the base's")
        base_layers = base_manifest.get("layers") or []
        for descriptor in base_layers:
            blobs[_oci_hex(descriptor["digest"])] = spec["base"] + "/" + _oci_blob_path(descriptor["digest"])
    runtime = dict(base_config.get("config") or {})
    if spec["entrypoint"] != None:
        if spec["entrypoint"]:
            runtime["Entrypoint"] = spec["entrypoint"]
        else:
            runtime.pop("Entrypoint", None)
        if spec["cmd"] == None:
            runtime.pop("Cmd", None)
    if spec["cmd"] != None:
        if spec["cmd"]:
            runtime["Cmd"] = spec["cmd"]
        else:
            runtime.pop("Cmd", None)
    if spec["env"]:
        runtime["Env"] = _oci_merge_env(runtime.get("Env") or [], spec["env"])
    for field, value in [("User", spec["user"]), ("WorkingDir", spec["working_dir"]), ("StopSignal", spec["stop_signal"])]:
        if value:
            runtime[field] = value
    if spec["labels"]:
        merged = dict(runtime.get("Labels") or {})
        merged.update(spec["labels"])
        runtime["Labels"] = merged
    if spec["exposed_ports"]:
        merged = dict(runtime.get("ExposedPorts") or {})
        for port in spec["exposed_ports"]:
            merged[port] = {}
        runtime["ExposedPorts"] = merged
    if spec["volumes"]:
        merged = dict(runtime.get("Volumes") or {})
        for volume in spec["volumes"]:
            merged[volume] = {}
        runtime["Volumes"] = merged
    diff_ids = list((base_config.get("rootfs") or {}).get("diff_ids") or []) + new_diff_ids
    history = list(base_config.get("history") or []) + [{"created_by": "once oci_layer"} for _ in new_layers]
    config = dict(base_config)
    config["architecture"] = spec["architecture"]
    config["os"] = spec["os"]
    if spec["variant"]:
        config["variant"] = spec["variant"]
    if spec["author"]:
        config["author"] = spec["author"]
    if spec["created"]:
        config["created"] = spec["created"]
    config["config"] = runtime
    config["rootfs"] = {"type": "layers", "diff_ids": diff_ids}
    config["history"] = history
    config_text = _json_encode(config)
    config_digest = content_sha256(config_text)
    manifest = {
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "digest": "sha256:" + config_digest,
            "size": _oci_utf8_length(config_text),
        },
        "layers": list(base_layers) + new_layers,
    }
    annotations = dict(spec["annotations"])
    if annotations:
        manifest["annotations"] = annotations
    manifest_text = _json_encode(manifest)
    manifest_digest = content_sha256(manifest_text)
    descriptor = {
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "digest": "sha256:" + manifest_digest,
        "size": _oci_utf8_length(manifest_text),
        "platform": {"architecture": spec["architecture"], "os": spec["os"]},
    }
    if spec["variant"]:
        descriptor["platform"]["variant"] = spec["variant"]
    indexed = _oci_indexed_descriptors(descriptor, spec["tags"])
    written = []
    for digest in sorted(blobs.keys()):
        destination = layout + "/blobs/sha256/" + digest
        copy_path(blobs[digest], destination, inputs = [blobs[digest]], identifier = label + ":oci-blob:" + digest[:12])
        written.append(destination)
    for digest, text in [(config_digest, config_text), (manifest_digest, manifest_text)]:
        destination = layout + "/blobs/sha256/" + digest
        write_path(destination, text)
        written.append(destination)
    docker_manifest = [{
        "Config": _oci_blob_path(config_digest),
        "RepoTags": spec["tags"] if spec["tags"] else None,
        "Layers": [_oci_blob_path(layer["digest"]) for layer in list(base_layers) + new_layers],
        "LayerSources": {layer["digest"]: layer for layer in list(base_layers) + new_layers},
    }]
    for path, text in [
        (layout + "/index.json", _json_encode({"schemaVersion": 2, "manifests": indexed})),
        (layout + "/oci-layout", _json_encode({"imageLayoutVersion": "1.0.0"})),
        (layout + "/manifest.json", _json_encode(docker_manifest)),
        (spec["descriptor"], _json_encode(descriptor)),
        (spec["manifest"], manifest_text),
        (spec["config"], config_text),
    ]:
        write_path(path, text)
        written.append(path)
    write_archive(
        _oci_layout_archive_entries(layout, written),
        spec["archive"],
        sha256_output = spec["archive_sha256"],
        format = "tar",
        inputs = written,
        identifier = label + ":oci-archive",
    )
    return None

def _oci_platform_text(platform):
    if not platform or not platform.get("os") or not platform.get("architecture"):
        return ""
    text = platform["os"] + "/" + platform["architecture"]
    return text + "/" + platform["variant"] if platform.get("variant") else text

def _oci_platform_key(platform):
    return (platform.get("os") or "") + "/" + (platform.get("architecture") or "") + ("/" + platform["variant"] if platform.get("variant") else "")

def _oci_index_impl(ctx):
    images = (ctx.get("deps_by_role") or {}).get("images") or []
    if not images:
        _diagnostic_fail("oci_index_without_images", "images", "oci_index requires at least one image", "Add image dependencies, one per platform")
    for image in images:
        if not image.get("layout"):
            _diagnostic_fail("oci_index_image_without_layout", "images", (image.get("label_id") or "image") + " does not provide an image layout", "Depend on image targets that export a layout")
    tags = _oci_tag_list(ctx)
    name = ctx["label"]["name"]
    layout = declare_output(name + ".oci")
    descriptor = declare_output("index-descriptor.json")
    archive = declare_output(name + ".oci.tar")
    archive_digest = declare_output(name + ".oci.tar.sha256")
    platforms = []
    for image in images:
        for platform in image.get("platforms") or [_oci_platform_text(image.get("platform") or {})]:
            if platform and platform not in platforms:
                platforms.append(platform)
    expand_actions(
        implementation = "oci_index_plan",
        inputs = [image["layout"] + "/index.json" for image in images],
        outputs = [layout + "/index.json", layout + "/oci-layout", descriptor, archive, archive_digest],
        args = {
            "label": ctx["label"]["id"],
            "layout": layout,
            "descriptor": descriptor,
            "archive": archive,
            "archive_sha256": archive_digest,
            "annotations": _oci_attr(ctx, "annotations", {}),
            "tags": tags,
            "images": [{"layout": image["layout"], "platform": image.get("platform") or {}} for image in images],
        },
    )
    return {
        "container_image": True,
        "oci_index": True,
        "label_id": ctx["label"]["id"],
        "target_kind": "oci_index",
        "layout": layout,
        "archive": archive,
        "archive_sha256": archive_digest,
        "descriptor": descriptor,
        "platforms": sorted(platforms),
        "tag": tags[0] if tags else "",
        "tags": tags,
        "default_output": archive,
    }

def oci_index_plan(ctx):
    spec = ctx["args"]
    root = workspace_root() + "/"
    layout = spec["layout"]
    label = spec["label"]
    blobs = {}
    entries = {}
    for image in spec["images"]:
        index = json_decode(host_file_read(root + image["layout"] + "/index.json"))
        for entry in _oci_leaf_manifests(root, image["layout"], index, label):
            manifest = json_decode(host_file_read(root + image["layout"] + "/" + _oci_blob_path(entry["digest"])))
            platform = entry.get("platform") or image["platform"]
            key = _oci_platform_key(platform)
            if not platform.get("os") or not platform.get("architecture"):
                _diagnostic_fail("oci_index_unknown_platform", "images", label + ": an image has no platform", "Set os and architecture on the image target")
            if key in entries:
                if entries[key]["digest"] == entry["digest"]:
                    continue
                _diagnostic_fail("oci_index_duplicate_platform", "images", label + ": more than one image targets " + key, "Keep one image per platform")
            entries[key] = {
                "mediaType": entry["mediaType"],
                "digest": entry["digest"],
                "size": entry["size"],
                "platform": platform,
            }
            for digest in [entry["digest"], manifest["config"]["digest"]] + [layer["digest"] for layer in manifest.get("layers") or []]:
                blobs[_oci_hex(digest)] = image["layout"] + "/" + _oci_blob_path(digest)
    index_document = {
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "manifests": [entries[key] for key in sorted(entries.keys())],
    }
    if spec["annotations"]:
        index_document["annotations"] = spec["annotations"]
    index_text = _json_encode(index_document)
    index_digest = content_sha256(index_text)
    descriptor = {
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "digest": "sha256:" + index_digest,
        "size": _oci_utf8_length(index_text),
    }
    indexed = _oci_indexed_descriptors(descriptor, spec["tags"])
    written = []
    for digest in sorted(blobs.keys()):
        destination = layout + "/blobs/sha256/" + digest
        copy_path(blobs[digest], destination, inputs = [blobs[digest]], identifier = label + ":oci-index-blob:" + digest[:12])
        written.append(destination)
    for path, text in [
        (layout + "/blobs/sha256/" + index_digest, index_text),
        (layout + "/index.json", _json_encode({"schemaVersion": 2, "manifests": indexed})),
        (layout + "/oci-layout", _json_encode({"imageLayoutVersion": "1.0.0"})),
        (spec["descriptor"], _json_encode(descriptor)),
    ]:
        write_path(path, text)
        written.append(path)
    write_archive(
        _oci_layout_archive_entries(layout, written),
        spec["archive"],
        sha256_output = spec["archive_sha256"],
        format = "tar",
        inputs = written,
        identifier = label + ":oci-index-archive",
    )
    return None

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
        attr("annotations", "map<string,string>", default = "{}", docs = "Annotations recorded on this layer's descriptor in the image manifest.", configurable = False),
        attr("compress", "string", default = "\"none\"", docs = "Layer compression. gzip stores the layer as a deterministic gzip stream, which registries transfer and store smaller.", configurable = False, allowed_values = ["none", "gzip"]),
        attr("symlinks", "map<string,string>", default = "{}", docs = "Symbolic links to create, from the container path of each link to its destination.", configurable = False),
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
        attr("entrypoint", "list<string>", docs = "Executable and fixed arguments. Defaults to the only packaged executable when there is no base. An empty list clears an inherited entrypoint.", configurable = True),
        attr("cmd", "list<string>", docs = "Default arguments appended to the entrypoint. An empty list clears an inherited command; leaving it unset keeps the base's, unless entrypoint is set.", configurable = True),
        attr("env", "map<string,string>", default = "{}", docs = "Runtime environment variables.", configurable = True),
        attr("user", "string", docs = "Default runtime user.", configurable = True),
        attr("working_dir", "string", docs = "Default runtime working directory.", configurable = True),
        attr("stop_signal", "string", docs = "Default runtime stop signal.", configurable = True),
        attr("labels", "map<string,string>", default = "{}", docs = "Image configuration labels.", configurable = True),
        attr("annotations", "map<string,string>", default = "{}", docs = "Image manifest annotations.", configurable = True),
        attr("exposed_ports", "list<string>", default = "[]", docs = "Exposed ports such as 8080/tcp.", configurable = True),
        attr("volumes", "list<string>", default = "[]", docs = "Container paths declared as volumes.", configurable = True),
        attr("author", "string", docs = "Image author recorded in the configuration.", configurable = True),
        attr("created", "string", docs = "RFC 3339 creation time recorded in the configuration. Omitted by default so identical inputs produce identical images.", configurable = True),
        attr("tag", "string", docs = "Archive reference name. Defaults to the target name with latest.", configurable = True),
        attr("tags", "list<string>", default = "[]", docs = "Additional archive reference names for the same image.", configurable = True),
    ],
    deps = [
        dep("layers", ["oci_layer"], "Ordered filesystem layers from base to top."),
        dep("base", ["container_image"], "Optional base image whose layers and configuration the new layers extend. It must export an image layout."),
    ],
    providers = ["container_image", "oci_image"],
    capabilities = [capability("build", ["archive", "layout", "descriptor", "manifest", "config"])],
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

oci_index = target_kind(
    docs = "Combines per-platform images into one multi-platform Open Container Initiative image index.",
    attrs = [
        attr("annotations", "map<string,string>", default = "{}", docs = "Index annotations.", configurable = True),
        attr("tag", "string", docs = "Archive reference name. Defaults to the target name with latest.", configurable = True),
        attr("tags", "list<string>", default = "[]", docs = "Additional archive reference names.", configurable = True),
    ],
    deps = [dep("images", ["container_image"], "Single-platform images, one per operating system and architecture.")],
    providers = ["container_image", "oci_index"],
    capabilities = [capability("build", ["archive", "layout", "descriptor"])],
    examples = [
        example(
            "oci-pull-and-extend",
            name = "Extend a digest-pinned base image",
            use_when = "Use this to publish one image per platform behind a single multi-platform reference.",
        ),
    ],
    source_references = _OCI_REFERENCES,
    impl = _oci_index_impl,
)
