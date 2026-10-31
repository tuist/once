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
        expand_actions(
            implementation = "apple_artifact_metadata_plan",
            inputs = _unique(provider["app_files"] + [info_plist]),
            outputs = [metadata],
            args = {"application": provider["app_path"], "executable": provider["app_executable"], "info_plist": info_plist, "metadata": metadata, "platform": platform, "sdk_variant": sdk_variant, "developer_dir": provider["xcode_developer_dir"]},
        )
        payloads["metadata"] = {"path": metadata, "media_type": "application/json"}
        interfaces["dev.once.apple-install.v2"] = {"payload": "bundle", "spec": {"metadata_payload": "metadata"}}
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
    write_path(spec["metadata"], _json_encode({
        "schema": "once.apple_application.v1",
        "display_name": plist.get("CFBundleDisplayName") or plist.get("CFBundleName") or "",
        "bundle_identifier": plist.get("CFBundleIdentifier") or "",
        "version": plist.get("CFBundleShortVersionString") or "",
        "build_version": plist.get("CFBundleVersion") or "",
        "mach_o_uuids": uuids,
        "platform": platform,
        "sdk_variant": spec["sdk_variant"],
        "signing": "ad_hoc",
        "signature_verified": True,
    }))
