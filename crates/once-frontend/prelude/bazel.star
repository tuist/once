_BAZEL_TOOL = tool("bazel", executables = ["bazel", "bazelisk"])

# Bazel labels use characters (`/`, `:`) that Once target names cannot carry, so
# the resolver folds them into underscores when it materializes a Bazel target
# as a graph node. The original label is preserved verbatim on the emitted
# target's `bazel_label` attribute so the impl can hand it back to Bazel.
def _bazel_sanitize(value):
    out = []
    for ch in value.elems():
        if ch == "/" or ch == "\\" or ch == ":" or ch == "@" or ch == "+":
            out.append("_")
        else:
            out.append(ch)
    return "".join(out)

def _bazel_target_name(label):
    stripped = label
    if stripped.startswith("//"):
        stripped = stripped[2:]
    # A root-package label spells itself `//:name`, whose stripped form starts
    # with `:`. Drop that leading separator so the resulting Once name is
    # `bz_<name>` instead of the double-underscored `bz__<name>`, and normalise
    # the remaining slashes and colons the same way for every deeper label.
    if stripped.startswith(":"):
        stripped = stripped[1:]
    return ("bz_" + _bazel_sanitize(stripped)) if stripped else "bz"

def _bazel_resolve_executable(ctx):
    requested = ctx["attr"].get("bazel") or "bazel"
    resolved = _resolve_host_executable(requested)
    if not resolved:
        fail(ctx["label"]["id"] + ": Bazel executable `" + requested + "` was not found on PATH")
    return resolved

def _bazel_workspace_dir(ctx):
    # `run_action` requires a workspace-relative cwd. The resolver sees the
    # Bazel workspace at the target's package directory, so returning that
    # directly (or `None` for the workspace root) is the right shape.
    return ctx["label"]["package"] or None

def _bazel_workspace_absolute_dir(ctx):
    # `host_command` runs on the host during resolution, so it takes an
    # absolute directory. Keep the two helpers separate to stay explicit
    # about which surface each string is for.
    root = workspace_root()
    package = ctx["label"]["package"]
    return (root + "/" + package) if package else root

def _bazel_env():
    environment = {}
    # Bazel needs a real HOME (repository cache, install base) and the caller's
    # PATH (bazelisk, cc, and the mise-managed toolchain shims). Everything else
    # is forwarded from the caller's environment at execution time by
    # inherit_parent_env, since we cannot cache Bazel actions anyway.
    for name in ["HOME", "PATH", "USER", "LOGNAME", "TMPDIR", "LANG", "LC_ALL", "SSL_CERT_FILE", "SSL_CERT_DIR", "JAVA_HOME"]:
        value = host_env(name)
        if value:
            environment[name] = value
    return environment

def _bazel_parse_query_output(text):
    entries = []
    for raw in text.split("\n"):
        line = raw.strip()
        if not line:
            continue
        parts = line.split(" ")
        if len(parts) < 3:
            continue
        rule_kind = parts[0]
        # `--output=label_kind` prints "<rule_kind> rule <label>". Anything else
        # (source files, package groups, environments) is skipped: those are
        # graph metadata, not buildable targets.
        if parts[1] != "rule":
            continue
        label = parts[2]
        if not label.startswith("//"):
            continue
        entries.append({"kind": rule_kind, "label": label})
    return entries

def _bazel_query_expression(ctx):
    expression = ctx["attr"].get("query") or "kind(\"rule\", //...)"
    excludes = ctx["attr"].get("exclude_packages") or []
    for pattern in excludes:
        if not pattern:
            continue
        expression = expression + " except (//" + pattern + "/... union //" + pattern + ":*)"
    return expression

def _bazel_kind_for_rule(rule_kind):
    # Every rule is buildable. Only rules whose Bazel class ends in `_test`
    # get the `test` capability, and only `_binary` rules get `run`, so each
    # instance advertises exactly what Bazel will accept.
    if _ends_with(rule_kind, "_test"):
        return "bazel_test"
    if _ends_with(rule_kind, "_binary"):
        return "bazel_binary"
    return "bazel_target"

def _bazel_workspace_resolver(ctx):
    bazel = _bazel_resolve_executable(ctx)
    workspace_dir = _bazel_workspace_absolute_dir(ctx)
    version = host_command([bazel, "--version"]).strip()
    expression = _bazel_query_expression(ctx)
    raw = host_command(
        [bazel, "query", expression, "--output=label_kind", "--noshow_progress"],
        cwd = workspace_dir,
        env = _bazel_env(),
    )
    rules = _bazel_parse_query_output(raw)
    targets = []
    roots = []
    seen = {}
    for entry in rules:
        name = _bazel_target_name(entry["label"])
        prior = seen.get(name)
        if prior != None:
            # Two distinct Bazel labels sanitise to the same Once name. Fail
            # loudly rather than silently drop one so a user with such a
            # workspace sees the conflict and can rename or exclude.
            fail(ctx["label"]["id"] + ": Bazel labels `" + prior + "` and `" + entry["label"] + "` both map to Once target name `" + name + "`. Use `exclude_packages` on the seed to drop one, or rename the Bazel target.")
        seen[name] = entry["label"]
        kind = _bazel_kind_for_rule(entry["kind"])
        targets.append({
            "name": name,
            "kind": kind,
            "deps": [],
            "srcs": [],
            "attrs": {
                "bazel_label": entry["label"],
                "bazel_rule_kind": entry["kind"],
                "bazel": ctx["attr"].get("bazel") or "bazel",
                "_bazel_resolved": True,
            },
        })
        # Every buildable rule is a root: users can build any of them
        # directly. Tests remain reachable through the `test` capability but do
        # not become build roots automatically, matching cargo_workspace.
        if kind != "bazel_test":
            roots.append(name)
    return {
        "targets": targets,
        "roots": roots,
        "attrs": {
            "_bazel_resolved": True,
            "_bazel_version": version,
        },
    }

def _bazel_workspace_impl(ctx):
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "bazel_workspace",
        "bazel_workspace": True,
        "bazel_version": ctx["attr"].get("_bazel_version") or "",
        "targets": ctx["deps"],
    }

def _bazel_capability_argv(ctx, bazel, capability):
    label = ctx["attr"]["bazel_label"]
    if capability == "build":
        return [bazel, "build", label, "--noshow_progress"]
    if capability == "test":
        return [bazel, "test", label, "--noshow_progress", "--test_output=errors"]
    if capability == "run":
        return [bazel, "run", label, "--noshow_progress", "--"]
    fail(ctx["label"]["id"] + ": bazel target does not support capability `" + capability + "`")

def _bazel_common_impl(ctx, providers):
    if not ctx["attr"].get("_bazel_resolved"):
        fail(ctx["label"]["id"] + ": " + providers[0] + " must be materialized by a bazel_workspace resolver")
    capability = ctx["capability"]
    if capability == "metadata":
        return {
            "label_id": ctx["label"]["id"],
            "bazel_label": ctx["attr"]["bazel_label"],
            "bazel_rule_kind": ctx["attr"]["bazel_rule_kind"],
            providers[0]: True,
        }
    bazel = _bazel_resolve_executable(ctx)
    workspace_dir = _bazel_workspace_dir(ctx)
    argv = _bazel_capability_argv(ctx, bazel, capability)
    run_action(
        argv = argv,
        inputs = [],
        outputs = [],
        cwd = workspace_dir,
        env = _bazel_env(),
        sandbox = "off",
        network = "unrestricted",
        cacheable = False,
        inherit_parent_env = True,
        toolchain_identity = "once.bazel.v1\x00" + bazel,
        identifier = ctx["label"]["id"] + ":bazel-" + capability,
    )
    return {
        "label_id": ctx["label"]["id"],
        "bazel_label": ctx["attr"]["bazel_label"],
        "bazel_rule_kind": ctx["attr"]["bazel_rule_kind"],
        providers[0]: True,
    }

def _bazel_target_impl(ctx):
    return _bazel_common_impl(ctx, ["bazel_target"])

def _bazel_test_impl(ctx):
    return _bazel_common_impl(ctx, ["bazel_test", "bazel_target"])

def _bazel_binary_impl(ctx):
    result = _bazel_common_impl(ctx, ["bazel_binary", "bazel_target"])
    if ctx["capability"] == "run":
        result["once_executable"] = True
    return result

_BAZEL_TARGET_ATTRS = [
    attr("bazel_label", "string", required = True, docs = "Fully qualified Bazel label of the underlying rule, for example `//src:kura`.", configurable = False),
    attr("bazel_rule_kind", "string", required = True, docs = "Bazel rule class reported by `bazel query --output=label_kind`, for example `rust_binary`.", configurable = False),
    attr("bazel", "string", default = "\"bazel\"", docs = "Bazel executable name or workspace-relative executable path forwarded from bazel_workspace.", configurable = False),
    attr("_bazel_resolved", "bool", default = "false", docs = "Resolver-owned marker preventing direct manifest authoring.", configurable = False),
]

_BAZEL_EXAMPLES = [
    example(
        "bazel-workspace-native-project",
        name = "Bazel native integration seed",
        use_when = "Use this when a Bazel workspace should expose its rules as Once targets while Bazel remains the executor.",
    ),
]

bazel_workspace = target_kind(
    docs = "Native Bazel workspace seed. Its resolver runs `bazel query` to enumerate every rule in the workspace and materializes each one as a bazel_target, bazel_test, or bazel_binary that forwards its capabilities to Bazel.",
    attrs = [
        attr("bazel", "string", default = "\"bazel\"", docs = "Bazel executable name or workspace-relative executable path. Defaults to `bazel`, which resolves through `bazelisk` when installed.", configurable = False),
        attr("query", "string", docs = "Bazel query expression used to enumerate rules. Defaults to `kind(\"rule\", //...)`.", configurable = False),
        attr("exclude_packages", "list<string>", default = "[]", docs = "Bazel package prefixes to strip from the default query. Each entry excludes both `//<prefix>/...` and `//<prefix>:*` from the enumerated rules.", configurable = False),
        attr("resolver_inputs", "list<string>", default = "[]", docs = "Package-relative text globs supplied to native integration resolution. Defaults to srcs when empty.", configurable = False),
        attr("_bazel_resolved", "bool", default = "false", docs = "Resolver-owned marker indicating that Bazel targets were expanded into graph targets.", configurable = False),
        attr("_bazel_version", "string", docs = "Resolver-recorded Bazel version banner.", configurable = False),
    ],
    resolver = _bazel_workspace_resolver,
    deps = [dep("deps", ["bazel_target"], "Resolver-generated Bazel rules materialized as graph targets.")],
    providers = ["bazel_workspace"],
    capabilities = [capability("build", [])],
    tools = [_BAZEL_TOOL],
    examples = _BAZEL_EXAMPLES,
    source_references = [
        source_reference(
            "Bazel",
            "query reference",
            "https://bazel.build/query/language",
            "Use the authoritative Bazel query language for enumerating rules and filtering by kind.",
        ),
        source_reference(
            "Bazel",
            "command-line reference",
            "https://bazel.build/reference/command-line-reference",
            "Preserve the documented `bazel build`, `bazel test`, and `bazel run` semantics rather than reimplementing a subset.",
        ),
    ],
    impl = _bazel_workspace_impl,
)

bazel_target = target_kind(
    docs = "Resolver-generated Bazel rule materialized as a Once graph target. Exposes only the `build` capability; use bazel_test for test rules and bazel_binary for binary rules.",
    attrs = _BAZEL_TARGET_ATTRS,
    providers = ["bazel_target"],
    capabilities = [capability("build", [])],
    tools = [_BAZEL_TOOL],
    examples = _BAZEL_EXAMPLES,
    impl = _bazel_target_impl,
)

bazel_test = target_kind(
    docs = "Resolver-generated Bazel test rule (rule class ending in `_test`) materialized as a Once graph target. Exposes `build` and `test` capabilities, both forwarded to Bazel.",
    attrs = _BAZEL_TARGET_ATTRS,
    providers = ["bazel_test", "bazel_target"],
    capabilities = [capability("build", []), capability("test", [])],
    tools = [_BAZEL_TOOL],
    examples = _BAZEL_EXAMPLES,
    impl = _bazel_test_impl,
)

bazel_binary = target_kind(
    docs = "Resolver-generated Bazel binary rule (rule class ending in `_binary`) materialized as a Once graph target. Exposes `build` and `run` capabilities, both forwarded to Bazel.",
    attrs = _BAZEL_TARGET_ATTRS,
    providers = ["bazel_binary", "bazel_target"],
    capabilities = [capability("build", []), capability("run", [])],
    tools = [_BAZEL_TOOL],
    examples = _BAZEL_EXAMPLES,
    impl = _bazel_binary_impl,
)

bazel = native_project(
    target_kind = "bazel_workspace",
    docs = "Recognizes a Bazel workspace from MODULE.bazel and exposes its rules as Once targets that delegate execution to Bazel.",
    markers = ["MODULE.bazel"],
    target_name = "bazel",
    inputs = ["WORKSPACE", "WORKSPACE.bazel", "MODULE.bazel.lock", "**/BUILD", "**/BUILD.bazel", "**/*.bzl"],
    exclude = _native_project_generated_dirs() + ["bazel-bin", "bazel-out", "bazel-testlogs", "node_modules"],
    input_exclude = ["bazel-bin", "bazel-out", "bazel-testlogs", ".git", ".once"],
    on_match = "stop",
    requires_tools = ["bazel"],
)
