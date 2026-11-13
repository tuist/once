def _apple_export_artifact(ctx, provider, info_plist, minimum_os, architecture):
    platform = "macos" if provider["platform"] in ["macos", "macosx"] else provider["platform"]
    platform = "visionos" if platform == "xros" else platform
    sdk_variant = "device" if platform == "macos" else provider["sdk_variant"]
    archive = declare_output("artifact/" + provider["product_name"] + ".zip")
    write_archive(
        [_artifact_entry(provider["app_path"], _basename(provider["app_path"]), kind = "tree", mode = 493)],
        archive,
        format = "zip",
        inputs = provider["app_files"],
        identifier = ctx["label"]["id"] + ":artifact-package",
    )
    interfaces = {
        "dev.once.apple-install.v1": {
            "payload": "bundle",
            "spec": {"platform": platform, "sdk_variant": sdk_variant, "architecture": architecture, "minimum_os": minimum_os, "bundle_identifier": provider["bundle_id"]},
        },
    }
    payloads = {"bundle": {"path": archive, "media_type": "application/zip"}}
    if host_os() == "macos":
        metadata = declare_output("artifact/metadata.json")
        icon = declare_output("artifact/icon.png")
        icon_set = provider.get("app_icon_files") or []
        expand_actions(
            implementation = "apple_artifact_metadata_plan",
            inputs = _unique(provider["app_files"] + [info_plist] + icon_set),
            outputs = [metadata, icon],
            args = {"application": provider["app_path"], "executable": provider["app_executable"], "info_plist": info_plist, "metadata": metadata, "icon": icon, "icon_set": icon_set, "platform": platform, "sdk_variant": sdk_variant, "developer_dir": provider["xcode_developer_dir"], "signing": provider.get("signing") or "ad_hoc"},
        )
        payloads["metadata"] = {"path": metadata, "media_type": "application/json"}
        payloads["icon"] = {"path": icon, "media_type": "image/png", "optional": True}
        interfaces["dev.once.apple-install.v2"] = {"payload": "bundle", "spec": {"metadata_payload": "metadata", "icon_payload": "icon"}}
    provider["artifact_info"] = _artifact_info([{
        "name": "product",
        "payloads": payloads,
        "interfaces": interfaces,
        "selectors": {"platform": platform, "sdk_variant": sdk_variant, "architecture": architecture},
    }], automatic_publication = host_os() == "macos" and (platform == "macos" or sdk_variant == "simulator"))
    return provider

def apple_artifact_metadata_plan(ctx):
    spec = ctx["args"]
    root = workspace_root() + "/"
    plist = json_decode(host_command([host_which("plutil"), "-convert", "json", "-o", "-", root + spec["info_plist"]]))
    developer = spec["developer_dir"]
    env = {"DEVELOPER_DIR": developer} if developer else {}
    host_command([host_which("codesign"), "--verify", "--deep", "--strict", root + spec["application"]], env = env)
    uuid_output = host_command([host_which("xcrun"), "dwarfdump", "--uuid", root + spec["executable"]], env = env)
    uuids = [line.split()[1] for line in uuid_output.split("\n") if line.startswith("UUID: ")]
    if not uuids:
        _diagnostic_fail("artifact_binary_identity_missing", None, "The application has no Mach-O UUID", "Build the application with a linker that emits UUIDs")
    platform = spec["platform"]
    _apple_artifact_icon(spec, plist, ctx["inputs"])
    write_path(spec["metadata"], _json_encode({
        "schema": "once.apple_application.v1",
        "display_name": plist.get("CFBundleDisplayName") or plist.get("CFBundleName") or "",
        "bundle_identifier": plist.get("CFBundleIdentifier") or "",
        "version": plist.get("CFBundleShortVersionString") or "",
        "build_version": plist.get("CFBundleVersion") or "",
        "mach_o_uuids": uuids,
        "platform": platform,
        "sdk_variant": spec["sdk_variant"],
        "signing": spec.get("signing") or "ad_hoc",
        "signature_verified": True,
    }))

def _apple_app_icon_files(asset_catalogs, app_icon):
    if not app_icon:
        return []
    files = []
    for catalog in asset_catalogs:
        directory = catalog + "/" + app_icon + ".appiconset"
        files.extend([directory + "/" + name for name in host_read_dir(_apple_host_path(directory)) if name.endswith(".png")])
    return files

def _apple_artifact_icon_sources(spec, plist, inputs):
    application = spec["application"]
    if spec["platform"] == "macos":
        name = plist.get("CFBundleIconFile") or ""
        icns = application + "/Contents/Resources/" + (name if name.endswith(".icns") else name + ".icns")
        return [icns] if name and icns in inputs else []
    names = []
    for key in ["CFBundleIcons", "CFBundleIcons~ipad"]:
        icons = plist.get(key)
        primary = icons.get("CFBundlePrimaryIcon") if type(icons) == "dict" else None
        if type(primary) == "dict":
            names.extend([name[:-len(".png")] if name.endswith(".png") else name for name in primary.get("CFBundleIconFiles") or []])
    prefix = application + "/"
    entries = [path[len(prefix):] for path in inputs if path.startswith(prefix) and "/" not in path[len(prefix):]]
    return [prefix + entry for entry in entries if entry.endswith(".png") and any([entry == name + ".png" or entry.startswith(name + "@") or entry.startswith(name + "~") for name in names])]

_APPLE_ARTIFACT_ICON_SOURCE_MAX_BYTES = 16 * 1024 * 1024

def _apple_artifact_icon_readable(path):
    # Xcode's CgBI PNG optimization produces files only Apple's decoders read.
    if host_file_size(path) > _APPLE_ARTIFACT_ICON_SOURCE_MAX_BYTES:
        return False
    return not path.endswith(".png") or not host_file_contains(path, "CgBI")

def _apple_host_path(path):
    return path if path.startswith("/") else workspace_root() + "/" + path

def _apple_artifact_icon(spec, plist, inputs):
    sources = [source for source in _apple_artifact_icon_sources(spec, plist, inputs) if _apple_artifact_icon_readable(_apple_host_path(source))]
    if not sources:
        sources = [source for source in spec.get("icon_set") or [] if host_file_exists(_apple_host_path(source)) and _apple_artifact_icon_readable(_apple_host_path(source))]
    if not sources:
        write_path(spec["icon"], "")
        return
    sips = host_which("sips")
    size, source = 0, ""
    for candidate in sources:
        output = host_command([sips, "-g", "pixelWidth", "-g", "pixelHeight", _apple_host_path(candidate)], check = False)
        values = [line.split(":", 1)[1].strip() for line in output.split("\n") if line.strip().startswith("pixel")]
        dimensions = [int(value) for value in values if value.isdigit()]
        if len(dimensions) == 2 and max(dimensions) > size:
            size, source = max(dimensions), candidate
    if not source:
        write_path(spec["icon"], "")
        return
    run_action(
        display_name = "Convert application icon",
        argv = [sips, "-s", "format", "png"] + (["--resampleHeightWidthMax", "512"] if size > 512 else []) + [source, "--out", spec["icon"]],
        inputs = [source],
        outputs = [spec["icon"]],
        toolchain_identity = "once.apple.artifact_icon.v1\x00" + sips,
        identifier = "apple_artifact_icon:" + spec["icon"],
    )
