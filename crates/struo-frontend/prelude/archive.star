def _archive_download_impl(ctx):
    attrs = ctx["attr"]
    destination = declare_output("archive")
    identifier = "archive_download:" + ctx["label"]["id"]
    authorization_env = attrs.get("authorization_env") or ""
    if authorization_env:
        download_and_extract(
            attrs["url"],
            attrs["sha256"],
            destination,
            authorization_env = authorization_env,
            identifier = identifier,
        )
    else:
        download_and_extract(
            attrs["url"],
            attrs["sha256"],
            destination,
            identifier = identifier,
        )
    return {
        "label_id": ctx["label"]["id"],
        "artifact_root": destination,
    }

archive_download = target_kind(
    docs = "Downloads a checksum-pinned compressed archive into a cacheable directory output.",
    impl = _archive_download_impl,
    attrs = [
        attr("url", "string", required = True, docs = "Web archive URL", configurable = False),
        attr("sha256", "string", required = True, docs = "Expected SHA-256 archive checksum", configurable = False),
        attr("authorization_env", "string", docs = "Optional environment-variable name that supplies a web Authorization header only while downloading", configurable = False),
    ],
    providers = ["artifact"],
    capabilities = [capability("build", ["default", "artifact"])],
    examples = [
        example(
            "archive-download-minimal",
            name = "Checksum-pinned archive download",
            use_when = "You need a verified compressed archive as the cached input to another target.",
        ),
    ],
)
