def _artifact_impl(ctx):
    attrs = _configured_attrs(ctx)
    files = glob(ctx["srcs"])
    if not files:
        _diagnostic_fail("artifact_sources_required", "srcs", "An artifact needs explicitly selected source files", "Add source patterns to srcs")
    archive = declare_output("artifact.zip")
    package = ctx["label"]["package"]
    prefix = package + "/" if package else ""
    entries = []
    for source in files:
        path = source[len(prefix):] if prefix and source.startswith(prefix) else source
        target = host_symlink_target(workspace_root() + "/" + source)
        if target:
            entries.append({"kind": "symlink", "path": path, "target": target})
        else:
            entries.append(_artifact_entry(source, path, mode = attrs.get("file_mode") or 420))
    write_archive(entries, archive, format = "zip", identifier = "artifact:" + ctx["label"]["id"])
    return {
        "label_id": ctx["label"]["id"],
        "default_output": archive,
        "artifact_info": _artifact_info([{
            "name": "product",
            "payloads": {"content": {"path": archive, "media_type": "application/zip"}},
            "interfaces": attrs.get("interfaces") or {},
            "selectors": attrs.get("selectors") or {},
        }]),
    }

artifact = target_kind(
    docs = "Packages explicitly selected files as a portable, publishable artifact with optional typed consumption interfaces.",
    impl = _artifact_impl,
    attrs = [
        attr("interfaces", "map<string,any>", default = "{}", docs = "Namespaced versioned contracts, each with payload = content and a typed spec object", configurable = False),
        attr("selectors", "map<string,string>", default = "{}", docs = "Explicit variant selection facts", configurable = False),
        attr("file_mode", "int", default = "420", docs = "Declared UNIX file permissions. Use 493 for executable files", configurable = False),
    ],
    providers = ["struo_artifact_info"],
    capabilities = [capability("build", ["default", "artifact"]), capability("artifact", ["default", "artifact"])],
    examples = [
        example("artifact-static-site", name = "Static site artifact", use_when = "You want to publish an explicitly selected static site without starting a server."),
        example("artifact-download", name = "Downloadable artifact", use_when = "You want to distribute arbitrary data without claiming it can run."),
    ],
)
