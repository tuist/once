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
    if not resolved and requested == "bazel":
        resolved = _resolve_host_executable("bazelisk")
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

def _bazel_env(ctx):
    environment = {}
    # Discovery needs the host installation; owned actions use only declared
    # environment and the native action environment, not this host seed.
    for name in ["HOME", "PATH", "USER", "LOGNAME", "TMPDIR", "LANG", "LC_ALL", "SSL_CERT_FILE", "SSL_CERT_DIR", "JAVA_HOME"]:
        value = host_env(name)
        if value:
            environment[name] = value
    for name, value in (ctx["attr"].get("env") or {}).items():
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
    version = host_command([bazel, "--version"], cwd = workspace_dir, env = _bazel_env(ctx)).strip()
    expression = _bazel_query_expression(ctx)
    raw = host_command(
        [bazel, "query", expression, "--output=label_kind", "--noshow_progress"],
        cwd = workspace_dir,
        env = _bazel_env(ctx),
    )
    rules = _bazel_parse_query_output(raw)
    test_labels = None
    test_query = ctx["attr"].get("test_query")
    if test_query:
        test_rules = host_command(
            [bazel, "query", test_query, "--output=label_kind", "--noshow_progress"],
            cwd = workspace_dir,
            env = _bazel_env(ctx),
        )
        test_labels = {entry["label"]: True for entry in _bazel_parse_query_output(test_rules)}
    targets = []
    roots = []
    test_roots = []
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
                "env": ctx["attr"].get("env") or {},
                "toolchain_inputs": ctx["attr"].get("toolchain_inputs") or [],
                "test_env": ctx["attr"].get("test_env") or {},
                "_bazel_resolved": True,
            },
        })
        # Every buildable rule is a root: users can build any of them
        # directly. Tests remain reachable through the `test` capability but do
        # not become build roots automatically, matching cargo_workspace.
        if kind != "bazel_test":
            roots.append(name)
        elif test_labels == None or entry["label"] in test_labels:
            test_roots.append(name)
    return {
        "targets": targets,
        "roots": roots,
        "attrs": {
            "_bazel_resolved": True,
            "_bazel_version": version,
            "_default_test_roots": test_roots,
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

# --- aquery-based execution (Once owns the actions) -------------------------
#
# The impls below read Bazel's action graph via `bazel aquery --output=jsonproto`
# and execute the requested output closure in a Once-owned execution root.
# Bazel supplies analysis and external repository downloads, never generated
# build outputs. Unknown action contracts remain explicitly delegated.

def _bazel_path_of_pf(pf_by_id, pf_id):
    segments = []
    cursor = pf_id
    # Bazel's tree is bounded by the workspace depth; 4096 is a hard ceiling to
    # keep the loop terminating even for an unexpected cycle.
    for _ in range(4096):
        if not cursor:
            break
        pf = pf_by_id.get(cursor)
        if pf == None:
            break
        segments.insert(0, pf.get("label") or "")
        cursor = pf.get("parentId") or 0
    return "/".join(segments)

def _bazel_path_of_artifact(pf_by_id, art_by_id, art_id):
    art = art_by_id.get(art_id)
    if art == None:
        return ""
    return _bazel_path_of_pf(pf_by_id, art.get("pathFragmentId") or 0)

def _bazel_expand_depset(depset_by_id, ds_id, seen, out):
    if seen.get(ds_id):
        return
    seen[ds_id] = True
    ds = depset_by_id.get(ds_id)
    if ds == None:
        return
    for aid in ds.get("directArtifactIds") or []:
        out[aid] = True
    for tid in ds.get("transitiveDepSetIds") or []:
        _bazel_expand_depset(depset_by_id, tid, seen, out)

def _bazel_parse_aquery(text):
    doc = json_decode(text)
    pf_by_id = {}
    for pf in doc.get("pathFragments") or []:
        pf_by_id[pf.get("id")] = pf
    art_by_id = {}
    for art in doc.get("artifacts") or []:
        art_by_id[art.get("id")] = art
    depset_by_id = {}
    for ds in doc.get("depSetOfFiles") or []:
        depset_by_id[ds.get("id")] = ds
    actions = []
    for action in doc.get("actions") or []:
        input_ids = {}
        for ds_id in action.get("inputDepSetIds") or []:
            _bazel_expand_depset(depset_by_id, ds_id, {}, input_ids)
        inputs = []
        for aid in input_ids.keys():
            path = _bazel_path_of_artifact(pf_by_id, art_by_id, aid)
            if path:
                inputs.append(path)
        outputs = []
        for aid in action.get("outputIds") or []:
            path = _bazel_path_of_artifact(pf_by_id, art_by_id, aid)
            if path:
                outputs.append(path)
        env = {}
        for ev in action.get("environmentVariables") or []:
            key = ev.get("key")
            if key:
                env[key] = ev.get("value") or ""
        actions.append({
            "mnemonic": action.get("mnemonic") or "",
            "arguments": action.get("arguments") or [],
            "inputs": sorted(inputs),
            "outputs": sorted(outputs),
            "env": env,
            "file_contents": action.get("fileContents"),
            "no_cache": any([entry.get("key") == "no-cache" for entry in action.get("executionInfo") or []]),
        })
    return actions

# Bazel mnemonics that Once can reproduce without invoking `bazel build`.
# `spawn`  — action has argv in aquery output; Once shells it out directly.
# `symlink` — action creates one output symlink pointing at its input.
# `symlink_tree` — action creates a tree of output symlinks mirroring its inputs.
# `write` — action writes a fixed file whose content aquery exposes via
#           `--include_file_write_contents`.
_BAZEL_SYMLINK_MNEMONICS = ["Symlink", "ExecutableSymlink", "SolibSymlink"]
_BAZEL_SYMLINK_TREE_MNEMONICS = ["SymlinkTree", "RunfilesTree"]
_BAZEL_WRITE_MNEMONICS = ["FileWrite", "RepoMappingManifest", "SourceSymlinkManifest"]

def _bazel_classify_action(action):
    # aquery omits the test environment and runfiles mapping that Bazel's
    # test strategy supplies when it invokes test-setup.sh.
    if action["mnemonic"] == "TestRunner":
        return "test" if len(action["arguments"]) >= 2 and action["inputs"] and action["outputs"] else "unsupported"
    if action["arguments"]:
        return "spawn"
    mnemonic = action["mnemonic"]
    if mnemonic in _BAZEL_SYMLINK_MNEMONICS and len(action["inputs"]) == 1 and len(action["outputs"]) == 1:
        return "symlink"
    if mnemonic in _BAZEL_SYMLINK_TREE_MNEMONICS and len(action["outputs"]) == 1:
        return "symlink_tree"
    if mnemonic in _BAZEL_WRITE_MNEMONICS and action["file_contents"] != None and len(action["outputs"]) == 1:
        return "write"
    if mnemonic == "Middleman" and len(action["outputs"]) == 1:
        # A middleman is Bazel's virtual artifact used to group inputs during
        # scheduling. The artifact itself has no downstream reader, so an
        # empty placeholder is enough to satisfy Once's action DAG.
        return "empty_write"
    return "unsupported"

def _bazel_shadow_dir(ctx):
    return (ctx["label"]["package"] + "/" if ctx["label"]["package"] else "") + ".once/bazel-actions"

def _bazel_prepare_shadow(ctx, bazel, workspace_abs):
    execution_root = host_command(
        [bazel, "info", "execution_root"],
        cwd = workspace_abs,
        env = _bazel_env(ctx),
    ).strip()
    return {
        "execution_root": execution_root,
        "output_base": host_command([bazel, "info", "output_base"], cwd = workspace_abs, env = _bazel_env(ctx)).strip(),
        "shadow_abs": workspace_root() + "/" + _bazel_shadow_dir(ctx),
    }

def _bazel_toolchain_identity(ctx):
    paths = sorted(ctx["attr"].get("toolchain_inputs") or [])
    if not paths:
        return None
    identities = []
    for path in paths:
        digest = host_file_sha256(path) if host_file_exists(path) else host_tree_sha256(path)
        identities.append([path, digest])
    return "once.bazel.toolchain.v1\x00" + content_sha256(_json_encode(identities))

def _bazel_stage_inputs(ctx, actions, shadow_rel, prep):
    produced = {output: True for action in actions for output in action["outputs"]}
    sources = {}
    for action in actions:
        for path in action["inputs"]:
            if _bazel_input_producer(path, produced) == None:
                sources[path] = True
    for path in sorted(sources.keys()):
        if path.startswith("external/"):
            materialize_host_file(prep["output_base"] + "/" + path, shadow_rel + "/" + path)
        elif path.startswith("bazel-out/"):
            fail("Bazel generated input has no declared producer: " + path)
        else:
            copy_path(_package_relative(ctx, path), shadow_rel + "/" + path)

def _bazel_bazel_flags():
    # `-module_maps` disables the CppModuleMap actions that Bazel's C++ rules
    # emit for every target: they generate a Clang modulemap file whose
    # content Bazel builds in-process and does not expose in aquery output,
    # so leaving them in forces every C++ target to fall back. Turning the
    # feature off is safe unless a workspace opts in to Clang header modules
    # explicitly, which is rare outside Chromium-style repos. Both the
    # target and host configurations need the toggle for it to cover
    # transitive `-sys`-style crates whose module maps are otherwise
    # generated in the exec configuration.
    #
    # Analysis must not rewrite the user's native output convenience links.
    return [
        "--features=-module_maps",
        "--host_features=-module_maps",
        "--experimental_convenience_symlinks=ignore",
    ]

# A `deps()` aquery for a real workspace carries one record per transitive
# action with its full argv, so its JSON is routinely tens of mebibytes and
# grows with the dependency closure. Raise the discovery bound for this one
# command instead of lowering it for every host probe.
_BAZEL_AQUERY_MAX_OUTPUT_BYTES = 512 * 1024 * 1024

def _bazel_input_producer(path, producers):
    for _ in range(64):
        producer = producers.get(path)
        if producer != None:
            return producer
        path = _parent_dir(path)
        if not path:
            return None
    return None

def _bazel_required_actions(actions, outputs):
    producers = {output: index for index in range(len(actions)) for output in actions[index]["outputs"]}
    needed = {}
    pending = list(outputs)
    for index in range(len(actions)):
        if actions[index]["mnemonic"] == "TestRunner" or not actions[index]["outputs"]:
            needed[index] = True
            pending.extend(actions[index]["inputs"])
    for _ in range(len(actions) + 1):
        if not pending:
            break
        following = []
        for path in pending:
            producer = _bazel_input_producer(path, producers)
            if producer == None or producer in needed:
                continue
            needed[producer] = True
            following.extend(actions[producer]["inputs"])
        pending = following
    return [actions[index] for index in range(len(actions)) if index in needed]

def _bazel_aquery(ctx, bazel, workspace_abs):
    label = ctx["attr"]["bazel_label"]
    # `--include_file_write_contents` is what lets Once own the FileWrite,
    # RepoMappingManifest, and SourceSymlinkManifest actions; without it the
    # payload aquery would need to emit is redacted and every Bazel target
    # with a runfiles tree falls back.
    # `--include_param_files` inlines the argv Bazel would otherwise fan
    # out through a `@file` reference. Actions like rules_rs's
    # cargo_build_script_runner pass their real arguments through param
    # files, so without this flag Once would run them with a truncated
    # argv and the runner would panic on the missing arguments.
    argv = [bazel, "aquery", "deps(" + label + ")", "--output=jsonproto", "--include_file_write_contents", "--include_param_files", "--noshow_progress"] + _bazel_bazel_flags()
    text = host_command(
        argv,
        cwd = workspace_abs,
        env = _bazel_env(ctx),
        max_output_bytes = _BAZEL_AQUERY_MAX_OUTPUT_BYTES,
    )
    actions = _bazel_parse_aquery(text)
    if not any([action["outputs"] for action in actions]):
        return actions
    # aquery includes unused output groups, including opaque stamping actions.
    # Select native default outputs, runfiles and validation rather than
    # synthesizing those unrelated actions or reading native build products.
    outputs = host_command(
        [bazel, "cquery", label, "--output=starlark", "--starlark:expr=" +
         "'\\n'.join([f.path for f in target.files.to_list()] + " +
         "[f.path for f in getattr(providers(target).get('OutputGroupInfo'), '_hidden_top_level_INTERNAL_', depset()).to_list()] + " +
         "[f.path for f in getattr(providers(target).get('OutputGroupInfo'), '_validation', depset()).to_list()])",
         "--noshow_progress"] + _bazel_bazel_flags(),
        cwd = workspace_abs,
        env = _bazel_env(ctx),
        max_output_bytes = _BAZEL_AQUERY_MAX_OUTPUT_BYTES,
    )
    return _bazel_required_actions(actions, [line for line in outputs.split("\n") if line])

def _bazel_delegate_action(ctx, bazel, capability):
    label = ctx["attr"]["bazel_label"]
    flags = _bazel_bazel_flags()
    if capability == "build":
        argv = [bazel, "build", label, "--noshow_progress"] + flags
    elif capability == "test":
        argv = [bazel, "test", label, "--noshow_progress", "--test_output=errors"] + flags
    elif capability == "run":
        argv = [bazel, "run", label, "--noshow_progress"] + flags + ["--"]
    else:
        fail(ctx["label"]["id"] + ": bazel target does not support capability `" + capability + "`")
    build_package = label[2:].split(":")[0] if label.startswith("//") else ""
    build_prefix = build_package + "/" if build_package else ""
    run_action(
        display_name = ("Test" if capability == "test" else "Run" if capability == "run" else "Build") + " with Bazel · " + label,
        source_files = _action_source_files([_package_relative(ctx, path) for path in [build_prefix + "BUILD.bazel", build_prefix + "BUILD", "MODULE.bazel", "WORKSPACE.bazel", "WORKSPACE"]]),
        argv = argv,
        inputs = [],
        outputs = [],
        cwd = _bazel_workspace_dir(ctx),
        env = _bazel_env(ctx),
        sandbox = "off",
        network = "unrestricted",
        cacheable = False,
        inherit_parent_env = True,
        toolchain_identity = "once.bazel.delegate.v1\x00" + bazel,
        identifier = ctx["label"]["id"] + ":bazel-delegate-" + capability,
    )

def _bazel_emit_spawn_action(ctx, action, index, shadow_rel, toolchain_identity):
    merged_env = dict(ctx["attr"].get("env") or {})
    for key, value in action["env"].items():
        merged_env[key] = value
    parent_dirs = _unique([
        shadow_rel + "/" + _parent_dir(output)
        for output in action["outputs"]
        if _parent_dir(output)
    ])
    run_action(
        display_name = "Bazel " + action["mnemonic"] + " · " + ctx["label"]["name"],
        presentation = _action_metadata(context = [_action_context("bazel.mnemonic", action["mnemonic"])]),
        source_files = _action_source_files([_package_relative(ctx, path) for path in action["inputs"] if not path.startswith("bazel-out/") and not path.startswith("external/")]),
        argv = action["arguments"],
        inputs = [shadow_rel + "/" + path for path in action["inputs"]],
        outputs = [shadow_rel + "/" + path for path in action["outputs"]],
        cwd = shadow_rel,
        env = merged_env,
        sandbox = "off",
        network = "unrestricted",
        cacheable = toolchain_identity != None and not action.get("no_cache") and bool(action["outputs"]),
        inherit_parent_env = False,
        depends_on_prior_actions = False,
        clean_paths = [shadow_rel + "/" + path for path in action["outputs"]],
        create_dirs = parent_dirs,
        toolchain_identity = toolchain_identity or "once.bazel.uncached.v1",
        identifier = "bazel:" + action["mnemonic"] + ":" + ",".join(sorted(action["outputs"])),
    )

def _bazel_emit_symlink_action(ctx, action, index, shadow_rel):
    source = action["inputs"][0]
    destination = action["outputs"][0]
    # Materialize aliases by value so their bytes, executable mode and restore
    # are owned by Once rather than a link into Bazel's mutable output tree.
    copy_path(
        shadow_rel + "/" + source,
        shadow_rel + "/" + destination,
        source_files = _action_source_files([_package_relative(ctx, source)]),
    )

def _bazel_manifest_entries(content, prep, workspace_abs):
    entries = []
    for line in content.split("\n"):
        if not line:
            continue
        escaped = line.startswith(" ")
        fields = (line[1:] if escaped else line).split(" ", 1)
        name = fields[0]
        source = fields[1] if len(fields) > 1 else ""
        if escaped:
            name = name.replace("\\s", " ").replace("\\n", "\n").replace("\\b", "\\")
            source = source.replace("\\s", " ").replace("\\n", "\n").replace("\\b", "\\")
        if source.startswith(prep["execution_root"] + "/"):
            source = source[len(prep["execution_root"]) + 1:]
        elif source.startswith(prep["output_base"] + "/external/"):
            source = source[len(prep["output_base"]) + 1:]
        elif source.startswith(workspace_abs + "/"):
            source = source[len(workspace_abs) + 1:]
        elif source.startswith("/"):
            fail("Bazel runfiles source is outside the declared execution root: " + source)
        entries.append([name, source])
    return entries

def _bazel_manifest_content(entries, shadow_abs):
    lines = []
    for name, source in entries:
        target = shadow_abs + "/" + source if source else ""
        if " " in name or "\n" in name or "\\" in name or " " in target or "\n" in target or "\\" in target:
            name = name.replace("\\", "\\b").replace(" ", "\\s").replace("\n", "\\n")
            target = target.replace("\\", "\\b").replace(" ", "\\s").replace("\n", "\\n")
            lines.append(" " + name + " " + target)
        else:
            lines.append(name + " " + target)
    return "\n".join(lines) + "\n"

def _bazel_emit_symlink_tree_action(ctx, action, index, shadow_rel, manifests):
    destination = action["outputs"][0]
    if action["mnemonic"] == "SymlinkTree":
        copy_path(shadow_rel + "/" + action["inputs"][0], shadow_rel + "/" + destination)
        return
    entries = manifests.get(destination + "_manifest")
    if entries == None:
        fail("Bazel runfiles tree has no exposed source manifest: " + destination)
    staging = shadow_rel + "/.runfiles/" + content_sha256(destination + _json_encode(entries))
    for name, source in entries:
        if source:
            copy_path(shadow_rel + "/" + source, staging + "/" + name)
        else:
            write_path(staging + "/" + name, "")
    write_path(staging + "/MANIFEST", _bazel_manifest_content(entries, workspace_root() + "/" + shadow_rel))
    # The directory copy owns replacement and cache restoration of the whole
    # tree, including removal of runfiles that disappeared from the manifest.
    copy_path(staging, shadow_rel + "/" + destination, kind = "tree", inputs = [shadow_rel + "/" + path for path in action["inputs"]])

def _bazel_emit_write_action(ctx, action, index, shadow_rel):
    # FileWrite, RepoMappingManifest, and SourceSymlinkManifest emit a fixed
    # payload aquery exposes via `--include_file_write_contents`.
    destination = action["outputs"][0]
    write_path(shadow_rel + "/" + destination, action["file_contents"] or "")

def _bazel_emit_empty_write_action(ctx, action, index, shadow_rel):
    destination = action["outputs"][0]
    write_path(shadow_rel + "/" + destination, "")

def _bazel_emit_test_action(ctx, bazel, action, shadow_rel, manifests):
    label = ctx["attr"]["bazel_label"]
    rule = json_decode(host_command(
        [bazel, "query", label, "--output=streamed_jsonproto", "--noshow_progress"],
        cwd = _bazel_workspace_absolute_dir(ctx),
        env = _bazel_env(ctx),
    ))["rule"]
    attrs = {attr["name"]: attr for attr in rule.get("attribute") or []}
    shards = max(1, attrs.get("shard_count", {}).get("intValue") or 0)
    timeout_name = attrs.get("timeout", {}).get("stringValue") or "moderate"
    timeout = {"short": 60, "moderate": 300, "long": 900, "eternal": 3600}.get(timeout_name)
    if timeout == None:
        fail("Bazel test has an unsupported timeout: " + timeout_name)
    timeout_tool = host_which_optional("timeout") or host_which_optional("gtimeout")
    if not timeout_tool:
        fail("Once Bazel tests require GNU timeout (timeout or gtimeout) on PATH")
    trees = [path for path in action["inputs"] if path.endswith(".runfiles")]
    if len(trees) != 1:
        fail("Bazel test needs exactly one exposed runfiles tree: " + label)
    runfiles = trees[0]
    entries = manifests.get(runfiles + "_manifest")
    if entries == None:
        fail("Bazel test has no exposed runfiles manifest: " + label)
    workspace = "_main"
    for name, source in entries:
        if source.endswith("/" + action["arguments"][1]):
            workspace = name.split("/")[0]
    native_env = dict(ctx["attr"].get("env") or {})
    native_env.update(action["env"])
    for pair in attrs.get("env", {}).get("stringDictValue") or []:
        native_env[pair["key"]] = pair["value"]
    for name in attrs.get("env_inherit", {}).get("stringListValue") or []:
        native_env[name] = host_env(name) or ""
    native_env.update(ctx["attr"].get("test_env") or {})
    for shard in range(shards):
        tail = _parent_dir(action["outputs"][0]) + "/once-shard-" + str(shard)
        environment = dict(native_env)
        environment.update({
            "HOME": tail + "/tmp",
            "TEST_TARGET": label,
            "TEST_WORKSPACE": workspace,
            "TEST_BINARY": action["arguments"][1],
            "TEST_SIZE": attrs.get("size", {}).get("stringValue") or "medium",
            "TEST_TIMEOUT": str(timeout),
            "TEST_RANDOM_SEED": "1",
            "TEST_SRCDIR": runfiles,
            "RUNFILES_DIR": runfiles,
            "JAVA_RUNFILES": runfiles,
            "PYTHON_RUNFILES": runfiles,
            "TEST_TMPDIR": tail + "/tmp",
            "XML_OUTPUT_FILE": tail + "/test.xml",
            "TEST_PREMATURE_EXIT_FILE": tail + "/test.exited_prematurely",
            "TEST_WARNINGS_OUTPUT_FILE": tail + "/test.warnings",
            "TEST_LOGSPLITTER_OUTPUT_FILE": tail + "/test.raw_splitlogs",
            "TEST_INFRASTRUCTURE_FAILURE_FILE": tail + "/test.infrastructure_failure",
            "TEST_UNUSED_RUNFILES_LOG_FILE": tail + "/test.unused_runfiles",
            "TEST_UNDECLARED_OUTPUTS_DIR": tail + "/test.outputs",
            "TEST_UNDECLARED_OUTPUTS_MANIFEST": tail + "/test.outputs_manifest/MANIFEST",
            "TEST_UNDECLARED_OUTPUTS_ANNOTATIONS": tail + "/test.outputs_manifest/ANNOTATIONS",
            "TEST_UNDECLARED_OUTPUTS_ANNOTATIONS_DIR": tail + "/test.outputs_manifest",
        })
        if shards > 1:
            environment.update({"TEST_SHARD_INDEX": str(shard), "TEST_TOTAL_SHARDS": str(shards), "TEST_SHARD_STATUS_FILE": tail + "/test.shard_status"})
        log = shadow_rel + "/" + tail + "/test.log"
        run_action(
            display_name = "Bazel test · " + label + " (shard " + str(shard + 1) + "/" + str(shards) + ")",
            argv = [timeout_tool, "--kill-after=5s", str(timeout)] + action["arguments"],
            inputs = [shadow_rel + "/" + path for path in action["inputs"]] + [shadow_rel + "/" + runfiles + "/" + name for name, source in entries],
            outputs = [log],
            stdout = log,
            cwd = shadow_rel,
            env = environment,
            clean_paths = [shadow_rel + "/" + tail],
            create_dirs = [shadow_rel + "/" + tail + "/tmp", shadow_rel + "/" + tail + "/test.outputs_manifest"],
            sandbox = "off",
            cacheable = False,
            inherit_parent_env = False,
            depends_on_prior_actions = False,
        )

def _bazel_topological_sort(actions):
    # aquery emits actions in a depth-first order rooted at the requested
    # label, which means the top-level target's link action appears before
    # the compile actions that produce its inputs. Once executes declared
    # actions in the order they arrive, so we need to hand it a
    # producer-before-consumer sequence. Build the DAG from output→input
    # dependencies and Kahn-sort so every producer runs before any consumer.
    producer_of = {}
    for index in range(len(actions)):
        for output in actions[index]["outputs"]:
            producer_of[output] = index
    dependencies = []
    dependents = []
    remaining = []
    for _ in range(len(actions)):
        dependencies.append({})
        dependents.append([])
        remaining.append(0)
    for index in range(len(actions)):
        for input_path in actions[index]["inputs"]:
            producer = _bazel_input_producer(input_path, producer_of)
            if producer == None or producer == index:
                continue
            if not dependencies[index].get(producer):
                dependencies[index][producer] = True
                dependents[producer].append(index)
                remaining[index] = remaining[index] + 1
    order = []
    ready = [index for index in range(len(actions)) if remaining[index] == 0]
    # Kahn iteration: pop a ready node, emit it, decrement its dependents'
    # counts. A cycle would leave some nodes with a non-zero count; fall
    # back to the raw aquery order for those to preserve forward progress.
    for _ in range(len(actions)):
        if not ready:
            break
        index = ready[0]
        ready = ready[1:]
        order.append(index)
        for dependent in dependents[index]:
            remaining[dependent] = remaining[dependent] - 1
            if remaining[dependent] == 0:
                ready.append(dependent)
    if len(order) < len(actions):
        emitted = {index: True for index in order}
        for index in range(len(actions)):
            if not emitted.get(index):
                order.append(index)
    return order

def _bazel_own_execution_or_fallback(ctx, bazel, workspace_abs, capability):
    actions = _bazel_aquery(ctx, bazel, workspace_abs)
    unsupported = {}
    classes = []
    for action in actions:
        cls = _bazel_classify_action(action)
        classes.append(cls)
        if cls == "unsupported":
            key = action["mnemonic"] or "unknown"
            unsupported[key] = (unsupported.get(key) or 0) + 1
    if unsupported:
        _bazel_delegate_action(ctx, bazel, capability)
        return {"mode": "delegated", "unsupported": unsupported, "action_count": len(actions)}
    prep = _bazel_prepare_shadow(ctx, bazel, workspace_abs)
    shadow_rel = _bazel_shadow_dir(ctx)
    toolchain_identity = _bazel_toolchain_identity(ctx)
    manifests = {}
    for action in actions:
        if action["mnemonic"] == "SourceSymlinkManifest" and action["file_contents"] != None:
            entries = _bazel_manifest_entries(action["file_contents"], prep, workspace_abs)
            manifests[action["outputs"][0]] = entries
            action["file_contents"] = _bazel_manifest_content(entries, prep["shadow_abs"])
    _bazel_stage_inputs(ctx, actions, shadow_rel, prep)
    order = _bazel_topological_sort(actions)
    for index in order:
        action = actions[index]
        cls = classes[index]
        if cls == "spawn":
            _bazel_emit_spawn_action(ctx, action, index, shadow_rel, toolchain_identity)
        elif cls == "symlink":
            _bazel_emit_symlink_action(ctx, action, index, shadow_rel)
        elif cls == "symlink_tree":
            _bazel_emit_symlink_tree_action(ctx, action, index, shadow_rel, manifests)
        elif cls == "write":
            _bazel_emit_write_action(ctx, action, index, shadow_rel)
        elif cls == "empty_write":
            _bazel_emit_empty_write_action(ctx, action, index, shadow_rel)
        elif cls == "test":
            pass
        else:
            fail(ctx["label"]["id"] + ": internal classification error for action `" + action["mnemonic"] + "`")
    if capability == "test":
        emitted_tests = {}
        for action in actions:
            if action["mnemonic"] == "TestRunner":
                key = _json_encode([action["arguments"], action["inputs"], action["env"]])
                if key in emitted_tests:
                    continue
                emitted_tests[key] = True
                _bazel_emit_test_action(ctx, bazel, action, shadow_rel, manifests)
    return {"mode": "own", "action_count": len(actions)}

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
    workspace_abs = _bazel_workspace_absolute_dir(ctx)
    outcome = _bazel_own_execution_or_fallback(ctx, bazel, workspace_abs, capability)
    return {
        "label_id": ctx["label"]["id"],
        "bazel_label": ctx["attr"]["bazel_label"],
        "bazel_rule_kind": ctx["attr"]["bazel_rule_kind"],
        "execution_mode": outcome["mode"],
        "action_count": outcome.get("action_count") or 0,
        "unsupported_mnemonics": outcome.get("unsupported") or {},
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
    attr("env", "map<string,string>", default = "{}", docs = "Declared environment forwarded from bazel_workspace. Empty values override inherited host variables.", configurable = False),
    attr("toolchain_inputs", "list<string>", default = "[]", docs = "Absolute host toolchain files and trees whose contents complete the native action input contract. Without this declaration spawn actions are uncached.", configurable = False),
    attr("test_env", "map<string,string>", default = "{}", docs = "Test runtime environment, including native test command options not reported by aquery. Rule environment is applied first.", configurable = False),
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
    docs = "Native Bazel workspace seed. Discovers rules with bazel query and executes their selected action closures in Once. Complete toolchain_inputs enable owned spawn caching; test execution remains uncached. Unsupported required action contracts delegate to Bazel and are reported on the provider.",
    attrs = [
        attr("bazel", "string", default = "\"bazel\"", docs = "Bazel executable name or workspace-relative executable path. Defaults to `bazel`, which resolves through `bazelisk` when installed.", configurable = False),
        attr("env", "map<string,string>", default = "{}", docs = "Environment for Bazel discovery, analysis, and execution. Empty values override inherited host variables. Native action environments reported by aquery take precedence.", configurable = False),
        attr("toolchain_inputs", "list<string>", default = "[]", docs = "Absolute host toolchain files and trees omitted by aquery, including system headers, compiler subprocesses and link libraries. Contents participate in every spawn action key. Without a complete declaration spawn actions remain uncached. These host inputs do not enable remote execution.", configurable = False),
        attr("test_env", "map<string,string>", default = "{}", docs = "Declared runtime environment for owned tests. Declare native test command options such as test_env here; aquery does not report them. Test execution is always uncached.", configurable = False),
        attr("query", "string", docs = "Bazel query expression used to enumerate rules. Defaults to `kind(\"rule\", //...)`.", configurable = False),
        attr("test_query", "string", docs = "Optional Bazel query selecting default test roots from the enumerated graph without removing build targets. When omitted, every enumerated test rule is selected. Bazel test command filters are not inferred.", configurable = False),
        attr("exclude_packages", "list<string>", default = "[]", docs = "Bazel package prefixes to strip from the default query. Each entry excludes both `//<prefix>/...` and `//<prefix>:*` from the enumerated rules.", configurable = False),
        attr("resolver_inputs", "list<string>", default = "[]", docs = "Package-relative text globs supplied to native integration resolution. Defaults to srcs when empty.", configurable = False),
        attr("_bazel_resolved", "bool", default = "false", docs = "Resolver-owned marker indicating that Bazel targets were expanded into graph targets.", configurable = False),
        attr("_bazel_version", "string", docs = "Resolver-recorded Bazel version banner.", configurable = False),
        attr("_default_test_roots", "list<string>", default = "[]", docs = "Resolver-owned first-party test target names used by targetless test selection.", configurable = False),
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
            "aquery reference",
            "https://bazel.build/query/aquery",
            "Use `aquery` to obtain the concrete action graph Once executes for each Bazel target.",
        ),
        source_reference(
            "Bazel",
            "command-line reference",
            "https://bazel.build/reference/command-line-reference",
            "Preserve the documented `bazel build`, `bazel test`, and `bazel run` semantics for the delegating fallback path.",
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
    docs = "Resolver-generated Bazel test rule (rule class ending in `_test`) materialized as a Once graph target. Exposes `build` and `test` capabilities.",
    attrs = _BAZEL_TARGET_ATTRS,
    providers = ["bazel_test", "bazel_target"],
    capabilities = [capability("build", []), capability("test", [])],
    tools = [_BAZEL_TOOL],
    examples = _BAZEL_EXAMPLES,
    impl = _bazel_test_impl,
)

bazel_binary = target_kind(
    docs = "Resolver-generated Bazel binary rule (rule class ending in `_binary`) materialized as a Once graph target. Exposes `build` and `run` capabilities.",
    attrs = _BAZEL_TARGET_ATTRS,
    providers = ["bazel_binary", "bazel_target"],
    capabilities = [capability("build", []), capability("run", [])],
    tools = [_BAZEL_TOOL],
    examples = _BAZEL_EXAMPLES,
    impl = _bazel_binary_impl,
)

bazel = native_project(
    target_kind = "bazel_workspace",
    docs = "Recognizes MODULE.bazel and exposes rules as Once targets. Once owns supported action closures; unsupported required contracts delegate to Bazel.",
    markers = ["MODULE.bazel"],
    target_name = "bazel",
    inputs = ["WORKSPACE", "WORKSPACE.bazel", "MODULE.bazel.lock", "**/BUILD", "**/BUILD.bazel", "**/*.bzl"],
    exclude = _native_project_generated_dirs() + ["bazel-bin", "bazel-out", "bazel-testlogs", "node_modules"],
    input_exclude = ["bazel-bin", "bazel-out", "bazel-testlogs", ".git", ".once"],
    on_match = "stop",
    requires_tools = ["bazel"],
    owns_descendants = True,
)

bazel_workspace_file = native_project(
    target_kind = "bazel_workspace",
    docs = "Recognizes WORKSPACE and exposes rules as Once targets. Once owns supported action closures; unsupported required contracts delegate to Bazel.",
    markers = ["WORKSPACE"],
    target_name = "bazel",
    inputs = ["WORKSPACE.bazel", "MODULE.bazel.lock", "**/BUILD", "**/BUILD.bazel", "**/*.bzl"],
    exclude = _native_project_generated_dirs() + ["bazel-bin", "bazel-out", "bazel-testlogs", "node_modules"],
    input_exclude = ["bazel-bin", "bazel-out", "bazel-testlogs", ".git", ".once"],
    on_match = "stop",
    requires_tools = ["bazel"],
    owns_descendants = True,
)

bazel_workspace_bazel_file = native_project(
    target_kind = "bazel_workspace",
    docs = "Recognizes WORKSPACE.bazel and exposes rules as Once targets. Once owns supported action closures; unsupported required contracts delegate to Bazel.",
    markers = ["WORKSPACE.bazel"],
    target_name = "bazel",
    inputs = ["WORKSPACE", "MODULE.bazel.lock", "**/BUILD", "**/BUILD.bazel", "**/*.bzl"],
    exclude = _native_project_generated_dirs() + ["bazel-bin", "bazel-out", "bazel-testlogs", "node_modules"],
    input_exclude = ["bazel-bin", "bazel-out", "bazel-testlogs", ".git", ".once"],
    on_match = "stop",
    requires_tools = ["bazel"],
    owns_descendants = True,
)
