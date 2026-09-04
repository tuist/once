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
                "_bazel_resolved": True,
            },
        })
        # Every buildable rule is a root: users can build any of them
        # directly. Tests remain reachable through the `test` capability but do
        # not become build roots automatically, matching cargo_workspace.
        if kind != "bazel_test":
            roots.append(name)
        else:
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
# and emit one Once `run_action` per spawn action. The action's cwd is a shadow
# exec root under `.once/bazel-shadow/<target>/` that Once populates itself:
#   .once/bazel-shadow/<target>/
#     external -> $OUTPUT_BASE/external            (Bazel's external repos)
#     bazel-out/                                    (populated as Once runs actions)
#     <every workspace file> -> <workspace>/<file> (symlinks to sources)
# Bazel supplies analysis and the external-repo download; Once runs every action
# from that shadow. Fully non-spawn action classes (Symlink, FileWrite,
# RunfilesTree, SymlinkTree, RepoMappingManifest) are Bazel-internal and have
# no argv, so any target whose action graph contains them falls back to
# `bazel build|test|run` for that capability with a diagnostic so the user knows
# what to expect. Follow-up work implements those mnemonics as native Once
# primitives; each one is small in isolation.

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
            "inputs": inputs,
            "outputs": outputs,
            "env": env,
            "file_contents": action.get("fileContents"),
        })
    return actions

# Bazel mnemonics that Once can reproduce without invoking `bazel build`.
# `spawn`  — action has argv in aquery output; Once shells it out directly.
# `symlink` — action creates one output symlink pointing at its input.
# `symlink_tree` — action creates a tree of output symlinks mirroring its inputs.
# `write` — action writes a fixed file whose content aquery exposes via
#           `--include_file_write_contents`.
_BAZEL_SYMLINK_MNEMONICS = ["Symlink", "ExecutableSymlink"]
_BAZEL_SYMLINK_TREE_MNEMONICS = ["SymlinkTree", "RunfilesTree"]
_BAZEL_WRITE_MNEMONICS = ["FileWrite", "RepoMappingManifest", "SourceSymlinkManifest"]

def _bazel_classify_action(action):
    if action["arguments"]:
        return "spawn"
    mnemonic = action["mnemonic"]
    if mnemonic in _BAZEL_SYMLINK_MNEMONICS and len(action["inputs"]) == 1 and len(action["outputs"]) == 1:
        return "symlink"
    if mnemonic in _BAZEL_SYMLINK_TREE_MNEMONICS and len(action["outputs"]) == 1:
        return "symlink_tree"
    if mnemonic in _BAZEL_WRITE_MNEMONICS and action["file_contents"] != None and len(action["outputs"]) == 1:
        return "write"
    if mnemonic == "TranslateBuildInfo" and len(action["outputs"]) == 1:
        # Without `--stamp` the workspace status is empty, so Bazel's
        # TranslateBuildInfo action writes a header with no key/value macros.
        # Reproducing an empty file is correct for the unstamped default;
        # workspaces that require stamped build info fall back to Bazel.
        return "empty_write"
    if mnemonic == "Middleman" and len(action["outputs"]) == 1:
        # A middleman is Bazel's virtual artifact used to group inputs during
        # scheduling. The artifact itself has no downstream reader, so an
        # empty placeholder is enough to satisfy Once's action DAG.
        return "empty_write"
    if mnemonic == "FileWrite" and len(action["outputs"]) == 1:
        # Some FileWrite actions have no content in aquery (for example, an
        # empty `.dwp` debug package on Mach-O). Producing an empty file
        # matches what Bazel would materialise for these.
        return "empty_write"
    return "unsupported"

def _bazel_shadow_dir(ctx):
    # Workspace-relative shadow path; the run_action `cwd` field only accepts
    # workspace-relative values. Under it lives the exec-root layout Bazel
    # would set up itself: `external` symlinked into the Bazel output base and
    # a fresh `bazel-out` directory that our actions populate.
    return ".once/bazel-shadow/" + ctx["label"]["id"]

def _bazel_prepare_shadow(ctx, bazel, workspace_abs):
    # Ask Bazel to fetch the target's external repositories. This does not
    # execute the target's actions; it only populates `$OUTPUT_BASE/external`
    # with the same content Bazel would materialize during a real build.
    label = ctx["attr"]["bazel_label"]
    host_command(
        [bazel, "fetch", label, "--noshow_progress"],
        cwd = workspace_abs,
        env = _bazel_env(),
    )
    output_base = host_command(
        [bazel, "info", "output_base"],
        cwd = workspace_abs,
        env = _bazel_env(),
    ).strip()
    shadow_abs = workspace_abs + "/" + _bazel_shadow_dir(ctx)
    # Rebuild the shadow deterministically on each analysis pass so an old
    # layout cannot survive across queries: remove, then materialize.
    host_command(["/bin/sh", "-c",
        "rm -rf " + _shell_quote(shadow_abs) +
        " && mkdir -p " + _shell_quote(shadow_abs + "/bazel-out") +
        " && ln -sfn " + _shell_quote(output_base + "/external") + " " + _shell_quote(shadow_abs + "/external"),
    ])
    return {
        "output_base": output_base,
        "shadow_abs": shadow_abs,
    }

def _bazel_link_workspace_sources(shadow_abs, workspace_abs):
    # Symlink every workspace entry into the shadow root so relative paths in
    # aquery argv resolve to real sources. `.once` is skipped so the shadow
    # cannot recurse into itself, and hidden git state is skipped to keep the
    # shadow small.
    host_command(["/bin/sh", "-c",
        "for entry in \"" + workspace_abs + "\"/*; do " +
        "  name=$(basename \"$entry\");" +
        "  case \"$name\" in .once|bazel-bin|bazel-out|bazel-testlogs|external) continue ;; esac;" +
        "  ln -sfn \"$entry\" \"" + shadow_abs + "/$name\";" +
        "done",
    ])

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
    return ["--features=-module_maps", "--host_features=-module_maps"]

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
        env = _bazel_env(),
    )
    return _bazel_parse_aquery(text)

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
    run_action(
        argv = argv,
        inputs = [],
        outputs = [],
        cwd = _bazel_workspace_dir(ctx),
        env = _bazel_env(),
        sandbox = "off",
        network = "unrestricted",
        cacheable = False,
        inherit_parent_env = True,
        toolchain_identity = "once.bazel.delegate.v1\x00" + bazel,
        identifier = ctx["label"]["id"] + ":bazel-delegate-" + capability,
    )

def _bazel_emit_spawn_action(ctx, action, index, shadow_rel):
    merged_env = _bazel_env()
    for key, value in action["env"].items():
        merged_env[key] = value
    parent_dirs = _unique([
        shadow_rel + "/" + _parent_dir(output)
        for output in action["outputs"]
        if _parent_dir(output)
    ])
    run_action(
        argv = action["arguments"],
        inputs = [],
        outputs = [],
        cwd = shadow_rel,
        env = merged_env,
        sandbox = "off",
        network = "unrestricted",
        cacheable = False,
        inherit_parent_env = True,
        create_dirs = parent_dirs,
        toolchain_identity = "once.bazel.action.v1\x00" + action["mnemonic"],
        identifier = ctx["label"]["id"] + ":bazel-action-" + str(index) + ":" + action["mnemonic"],
    )

def _bazel_relative_target(source, destination):
    # `ln -s <target>` records `<target>` verbatim as the symlink content.
    # `<target>` is then resolved relative to the symlink's own parent
    # directory when it is dereferenced, so a shadow-relative source has to
    # be rewritten to walk from the symlink's parent back to the shadow root
    # and then down the source path.
    parent = _parent_dir(destination)
    depth = 0
    if parent:
        for segment in parent.split("/"):
            if segment and segment != ".":
                depth = depth + 1
    prefix = "".join(["../" for _ in range(depth)])
    return prefix + source

def _bazel_emit_symlink_action(ctx, action, index, shadow_rel):
    # Bazel's Symlink and ExecutableSymlink actions materialise one output as
    # a symlink pointing at their sole input. `ln -sfn` reproduces both
    # variants (the -f -n combination replaces an existing entry and never
    # dereferences a target directory). The source is shadow-relative, so
    # rewrite it to be relative to the symlink's own parent directory or
    # the resulting symlink dangles when a consumer dereferences it.
    source = action["inputs"][0]
    destination = action["outputs"][0]
    parent = _parent_dir(destination)
    parent_dirs = [shadow_rel + "/" + parent] if parent else []
    target = _bazel_relative_target(source, destination)
    run_action(
        argv = ["/bin/sh", "-c", "ln -sfn " + _shell_quote(target) + " " + _shell_quote(destination)],
        inputs = [],
        outputs = [],
        cwd = shadow_rel,
        env = _bazel_env(),
        sandbox = "off",
        cacheable = False,
        inherit_parent_env = True,
        create_dirs = parent_dirs,
        toolchain_identity = "once.bazel.symlink.v1",
        identifier = ctx["label"]["id"] + ":bazel-action-" + str(index) + ":" + action["mnemonic"],
    )

def _bazel_emit_symlink_tree_action(ctx, action, index, shadow_rel):
    # SymlinkTree / RunfilesTree materialise a directory whose entries mirror
    # the action's declared inputs. Each input path becomes a symlink under
    # the output directory, joined by the input's exec-root-relative path so
    # the tree matches what Bazel would build. Runfiles trees can list tens
    # of thousands of entries, well past the OS `ARG_MAX` limit for a single
    # `sh -c` invocation, so the shell script goes to a file that the action
    # then runs.
    destination = action["outputs"][0]
    # write_path expects a workspace-relative path, but the action's `cwd`
    # is the shadow root, so the argv only carries the tail of the path.
    script_tail = ".bazel-tree-scripts/" + str(index) + ".sh"
    script_workspace_path = shadow_rel + "/" + script_tail
    lines = ["#!/bin/sh", "set -e", "mkdir -p " + _shell_quote(destination)]
    for input_path in action["inputs"]:
        entry = destination + "/" + input_path
        parent = _parent_dir(entry)
        if parent and parent != destination:
            lines.append("mkdir -p " + _shell_quote(parent))
        target = _bazel_relative_target(input_path, entry)
        lines.append("ln -sfn " + _shell_quote(target) + " " + _shell_quote(entry))
    write_path(script_workspace_path, "\n".join(lines) + "\n")
    run_action(
        argv = ["/bin/sh", script_tail],
        inputs = [],
        outputs = [],
        cwd = shadow_rel,
        env = _bazel_env(),
        sandbox = "off",
        cacheable = False,
        inherit_parent_env = True,
        create_dirs = [shadow_rel + "/" + destination],
        toolchain_identity = "once.bazel.symlink_tree.v1",
        identifier = ctx["label"]["id"] + ":bazel-action-" + str(index) + ":" + action["mnemonic"],
    )

def _bazel_emit_write_action(ctx, action, index, shadow_rel):
    # FileWrite, RepoMappingManifest, and SourceSymlinkManifest emit a fixed
    # payload aquery exposes via `--include_file_write_contents`.
    destination = action["outputs"][0]
    write_path(shadow_rel + "/" + destination, action["file_contents"] or "")

def _bazel_emit_empty_write_action(ctx, action, index, shadow_rel):
    destination = action["outputs"][0]
    write_path(shadow_rel + "/" + destination, "")

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
            producer = producer_of.get(input_path)
            if producer == None and input_path.startswith("bazel-out/"):
                # A `bazel-out/` path with no exact producer is likely a file
                # inside a tree-artifact output (aquery lists the directory
                # but not its individual files). Walk the parent chain and
                # link to the ancestor directory's producer instead of doing
                # an O(N) scan over every action's outputs, which on a
                # workspace like kura would take orders of magnitude longer
                # than the actions themselves.
                parent = _parent_dir(input_path)
                for _ in range(64):
                    if not parent:
                        break
                    candidate = producer_of.get(parent)
                    if candidate != None and candidate != index:
                        producer = candidate
                        break
                    parent = _parent_dir(parent)
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
    # aquery-based ownership: read the action graph and run every action from
    # the shadow exec root in a producer-before-consumer order. Spawn actions
    # execute their aquery argv directly; Symlink, ExecutableSymlink,
    # SymlinkTree, RunfilesTree, FileWrite, RepoMappingManifest,
    # SourceSymlinkManifest, TranslateBuildInfo, and Middleman are
    # reproduced from their aquery-declared inputs, outputs, and (for the
    # write mnemonics) file contents. Anything else falls back to
    # `bazel <capability>` for the whole target and is recorded on the
    # provider so the gap is visible.
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
    _bazel_link_workspace_sources(prep["shadow_abs"], workspace_abs)
    shadow_rel = _bazel_shadow_dir(ctx)
    order = _bazel_topological_sort(actions)
    for index in order:
        action = actions[index]
        cls = classes[index]
        if cls == "spawn":
            _bazel_emit_spawn_action(ctx, action, index, shadow_rel)
        elif cls == "symlink":
            _bazel_emit_symlink_action(ctx, action, index, shadow_rel)
        elif cls == "symlink_tree":
            _bazel_emit_symlink_tree_action(ctx, action, index, shadow_rel)
        elif cls == "write":
            _bazel_emit_write_action(ctx, action, index, shadow_rel)
        elif cls == "empty_write":
            _bazel_emit_empty_write_action(ctx, action, index, shadow_rel)
        else:
            fail(ctx["label"]["id"] + ": internal classification error for action `" + action["mnemonic"] + "`")
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
    docs = "Native Bazel workspace seed. Its resolver runs `bazel query` to enumerate every rule in the workspace and materializes each one as a bazel_target, bazel_test, or bazel_binary that runs through Once when its action graph is fully spawn-based and falls back to `bazel <capability>` otherwise.",
    attrs = [
        attr("bazel", "string", default = "\"bazel\"", docs = "Bazel executable name or workspace-relative executable path. Defaults to `bazel`, which resolves through `bazelisk` when installed.", configurable = False),
        attr("query", "string", docs = "Bazel query expression used to enumerate rules. Defaults to `kind(\"rule\", //...)`.", configurable = False),
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
    docs = "Recognizes a Bazel workspace from MODULE.bazel and exposes its rules as Once targets whose action graph Once executes itself when it is entirely spawn-based.",
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
    docs = "Recognizes a Bazel workspace from WORKSPACE and exposes its rules as Once targets whose action graph Once executes itself when it is entirely spawn-based.",
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
    docs = "Recognizes a Bazel workspace from WORKSPACE.bazel and exposes its rules as Once targets whose action graph Once executes itself when it is entirely spawn-based.",
    markers = ["WORKSPACE.bazel"],
    target_name = "bazel",
    inputs = ["WORKSPACE", "MODULE.bazel.lock", "**/BUILD", "**/BUILD.bazel", "**/*.bzl"],
    exclude = _native_project_generated_dirs() + ["bazel-bin", "bazel-out", "bazel-testlogs", "node_modules"],
    input_exclude = ["bazel-bin", "bazel-out", "bazel-testlogs", ".git", ".once"],
    on_match = "stop",
    requires_tools = ["bazel"],
    owns_descendants = True,
)
