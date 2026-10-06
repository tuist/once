def _dockerfile_follow_links(ctx, context, selected, inputs, symlinks):
    if not symlinks:
        return selected
    root = _dockerfile_workspace_path(ctx, context)
    result = {}
    for path in selected:
        result[path] = True
    frontier = list(selected)
    for _ in range(8):
        added = []
        for path in frontier:
            target = symlinks.get(path)
            if target == None:
                continue
            if target.startswith("/"):
                base = [] if root == "." else root.split("/")
            else:
                base = path.split("/")[:-1]
            for part in target.split("/"):
                if part in ["", "."]:
                    continue
                if part == "..":
                    if not base:
                        return None
                    base = base[:-1]
                else:
                    base = base + [part]
            resolved = "/".join(base)
            if root != "." and resolved != root and not resolved.startswith(root + "/"):
                return None
            for candidate in inputs:
                if (candidate == resolved or candidate.startswith(resolved + "/")) and candidate not in result:
                    result[candidate] = True
                    added.append(candidate)
        if not added:
            return sorted(result.keys())
        frontier = added
    return None

def _dockerfile_declares_onbuild(docker, base):
    local = host_command([docker, "image", "inspect", "--format", "{{json .Config.OnBuild}}", base], check = False).strip()
    if local:
        return local not in ["null", "[]"]
    remote = host_command([docker, "buildx", "imagetools", "inspect", "--format", "{{json .Image}}", base], check = False)
    return '"OnBuild":["' in remote.replace(" ", "").replace("\n", "")

def _dockerfile_copy_inputs(ctx, instruction, context, inputs, symlinks = {}):
    if instruction["opcode"] not in ["COPY", "ADD", "RUN"]:
        return []
    argument = instruction["argument"]
    if instruction["opcode"] == "RUN":
        for flag in _dockerfile_flags(argument):
            if flag.startswith("--mount="):
                options = {}
                for option in flag[len("--mount="):].split(","):
                    parts = option.split("=", 1)
                    options[parts[0]] = parts[1] if len(parts) == 2 else ""
                mount_type = options.get("type", "bind")
                if (mount_type == "bind" or "$" in mount_type) and not options.get("from"):
                    return inputs
        return []
    sources = _dockerfile_copy_sources(instruction)
    if sources == None:
        return inputs
    selected = []
    for source in sources:
        matched = _dockerfile_source_matches(ctx, context, source, inputs)
        if not matched:
            return inputs
        selected.extend(matched)
    followed = _dockerfile_follow_links(ctx, context, _unique(selected), inputs, symlinks)
    return inputs if followed == None else followed

def _dockerfile_copy_sources(instruction):
    argument = instruction["argument"]
    flags = _dockerfile_flags(argument)
    if any([flag.startswith("--from=") for flag in flags]):
        return []
    if flags and ('"' in argument or "'" in argument):
        return None
    body = argument
    for flag in flags:
        body = body[len(flag):].strip()
    inner = body[1:].strip() if body.startswith("[") else ""
    paths = json_decode(body) if body.startswith("[") and (inner.startswith('"') or inner == "]") else _dockerfile_words(body)
    sources = paths[:-1]
    if any(["$" in source or "*" in source or "?" in source or "[" in source for source in sources]):
        return None
    normalized = []
    for source in sources:
        if "://" in source:
            continue
        parts = []
        for part in source.split("/"):
            if part in ["", "."]:
                continue
            if part == "..":
                if parts:
                    parts.pop()
            else:
                parts.append(part)
        normalized.append("/".join(parts))
    return normalized

def _dockerfile_source_matches(ctx, context, source, inputs):
    prefix = _dockerfile_workspace_path(ctx, context)
    prefix = prefix + "/" + source if source not in ["", "."] else prefix
    prefix = prefix[2:] if prefix.startswith("./") else prefix
    return [path for path in inputs if prefix == "." or path == prefix or path.startswith(prefix + "/")]

def _dockerfile_missing_sources(ctx, instructions, context, inputs):
    findings = []
    for instruction in _dockerfile_commands(instructions):
        if instruction["opcode"] not in ["COPY", "ADD"] or instruction.get("unsupported"):
            continue
        for source in _dockerfile_copy_sources(instruction) or []:
            if source and not _dockerfile_source_matches(ctx, context, source, inputs):
                findings.append(_dockerfile_finding(ctx, instruction, "missing_copy_source", "Copy source `" + source + "` is not present in the build context `" + context + "`.", "Check the context. A Dockerfile that copies paths relative to a parent directory needs a target in that directory with dockerfile and context = \".\"; a generated source must exist before the build."))
    return findings

def _dockerfile_raw_word(text):
    quote = ""
    escaped = False
    for index, character in enumerate(text.elems()):
        if escaped:
            escaped = False
        elif character == "\\" and quote != "'":
            escaped = True
        elif quote:
            if character == quote:
                quote = ""
        elif character in ["'", '"']:
            quote = character
        elif character in [" ", "\t", "\n"]:
            return (text[:index], text[index:])
    return (text, "")

def _dockerfile_replace_mount_source(text, name, replacement):
    keyword, rest = _dockerfile_raw_word(text)
    rewritten = keyword
    for _flag in _dockerfile_flags(rest):
        spacing = ""
        for character in rest.elems():
            if character not in [" ", "\t", "\n"]:
                break
            spacing += character
        word, tail = _dockerfile_raw_word(rest[len(spacing):])
        if word.startswith("--mount="):
            options = word[len("--mount="):].split(",")
            options = ["from=" + replacement if option == "from=" + name else option for option in options]
            word = "--mount=" + ",".join(options)
        rewritten += spacing + word
        rest = tail
    return rewritten + rest

def _dockerfile_lint(ctx, dockerfile, findings):
    paths = _lint_paths(ctx)
    results = []
    for finding in findings:
        results.append({
            "ruleId": finding["code"],
            "level": finding["severity"],
            "message": {"text": finding["message"] + " " + finding["repairs"][0]},
            "locations": [{"physicalLocation": {
                "artifactLocation": {"uri": _dockerfile_workspace_path(ctx, dockerfile)},
                "region": {"startLine": finding["line"]},
            }}],
            "properties": finding,
        })
    write_path(paths["sarif"], _json_encode({"version": "2.1.0", "runs": [{"tool": {"driver": {"name": "dockerfile_inputs"}}, "results": results}]}))
    write_path(paths["native"], _json_encode(findings))
    return _lint_provider(ctx, "dockerfile_inputs", paths, [_dockerfile_workspace_path(ctx, dockerfile)], {"argv": [], "env": {}, "cwd": "."}, native_results = [paths["native"]], target_kind = "dockerfile_image")

def _dockerfile_context_name(ctx, provider):
    label = provider.get("label_id") or ""
    return (_dockerfile_attr(ctx, "context_names", {}).get(label)) or label.split("/")[-1]

def _dockerfile_pulled_image_declares_onbuild(ctx, image):
    source = image.get("onbuild_source") or ({"reference": image["reference"], "platforms": image.get("platforms") or []} if image.get("reference") else None)
    crane = _resolve_host_executable("crane")
    if not crane or not source:
        return False
    platform = (source.get("platforms") or [""])[0] or _dockerfile_attr(ctx, "platform", "")
    argv = [crane, "config"] + (["--platform", platform] if platform else []) + [source["reference"]]
    return '"OnBuild":["' in host_command(argv, check = False).replace(" ", "").replace("\n", "")

def _dockerfile_onbuild_contexts(ctx):
    roles = ctx.get("deps_by_role") or {}
    return [_dockerfile_context_name(ctx, image) for image in roles.get("images") or [] if image.get("onbuild") or _dockerfile_pulled_image_declares_onbuild(ctx, image)]

def _dockerfile_has_onbuild(instructions):
    return any([instruction["opcode"] == "ONBUILD" for instruction in _dockerfile_commands(instructions)])

def _dockerfile_context_names(ctx):
    roles = ctx.get("deps_by_role") or {}
    return [_dockerfile_context_name(ctx, provider) for provider in (roles.get("images") or []) + (roles.get("programs") or [])]

def _dockerfile_named_contexts(ctx):
    roles = ctx.get("deps_by_role") or {}
    named = {}
    for image in roles.get("images") or []:
        name = _dockerfile_context_name(ctx, image)
        layout = image.get("layout")
        if not layout:
            _dockerfile_fail("dockerfile_image_dependency_without_layout", "images", name + " does not provide an image layout; select a container or remote builder for it so it can export one", "Build the dependency with a docker-container or remote Buildx builder, or use an oci_image, oci_pull, or instruction-mode dockerfile_image target")
        named[name] = {"path": layout, "layout": True, "inputs": [layout], "ref": _dockerfile_layout_ref(image)}
    for program in roles.get("programs") or []:
        name = _dockerfile_context_name(ctx, program)
        source = (program.get("executable") or {}).get("path")
        if not source:
            _dockerfile_fail("dockerfile_program_without_executable", "programs", name + " does not provide an executable", "Depend on a target that builds an executable")
        staged = declare_output("contexts/" + name + "/" + _basename(source))
        copy_path(source, staged, inputs = [source], identifier = ctx["label"]["id"] + ":context:" + name, source_files = _action_source_files([source]))
        named[name] = {"path": "/".join(staged.split("/")[:-1]), "layout": False, "inputs": [staged]}
    return named

def _dockerfile_context_flags(contexts):
    flags = []
    for name in sorted(contexts.keys()):
        flags.extend(["--build-context", name + "=" + ("oci-layout://" if contexts[name]["layout"] else "") + contexts[name]["path"] + contexts[name].get("ref", "")])
    return flags

def _dockerfile_layout_ref(image):
    tags = image.get("tags") or []
    if not tags or "@" in tags[0] or image.get("target_kind") in ["oci_pull"]:
        return ""
    last = tags[0].split("/")[-1]
    return ":" + last.split(":")[1] if ":" in last else ":latest"

def _dockerfile_instruction_impl(ctx, auto = False):
    dockerfile, context, _, instructions = _dockerfile_load(ctx)
    commands = _dockerfile_commands(instructions)
    findings = _dockerfile_findings(ctx, instructions)
    inputs, discovery, symlinks = _dockerfile_inputs(ctx, dockerfile, context)
    unsupported = _dockerfile_first_unsupported(instructions)
    if ctx["capability"] == "metadata":
        return {"container_image": True, "dockerfile_image": True, "label_id": ctx["label"]["id"], "target_kind": "dockerfile_image", "execution_mode": "instructions", "affected_inputs": inputs, "instructions": commands, "diagnostics": findings, "unsupported": _dockerfile_unsupported_message(unsupported) if unsupported else None, "input_discovery": discovery}
    if unsupported:
        _dockerfile_fail("dockerfile_unsupported_syntax", "execution_mode", ctx["label"]["id"] + ": " + _dockerfile_unsupported_message(unsupported), "Set execution_mode = \"buildkit\" (or \"auto\") on this target, or remove the unsupported construct")
    required_stages, global_values = _dockerfile_selected_stages(ctx, instructions)
    docker, identity, driver, builder = _dockerfile_toolchain(ctx, auto_builder = True)
    if driver == "docker":
        _dockerfile_fail("dockerfile_builder_unsupported", "builder", "Instruction execution requires a docker-container or remote builder; select builder or execution_mode = \"buildkit\"", "Set builder to a docker-container or remote Buildx builder, or set execution_mode = \"buildkit\"")
    if _dockerfile_attr(ctx, "cache_to", []) or _dockerfile_attr(ctx, "export_cache", False) or (ctx.get("deps_by_role") or {}).get("caches", []):
        _dockerfile_fail("dockerfile_option_needs_whole_file", "execution_mode", "Instruction snapshots are cached by Once; cache_to, export_cache, and caches dependencies require execution_mode = \"buildkit\"", "Set execution_mode = \"buildkit\" (or \"auto\"), or remove cache_to, export_cache, and caches dependencies")
    onbuild_contexts = _dockerfile_onbuild_contexts(ctx)
    for entry in _dockerfile_image_references(ctx, instructions):
        if entry["role"] != "base" or entry["stage"] not in required_stages:
            continue
        triggers = entry["base"] in onbuild_contexts or (entry.get("external") and _dockerfile_declares_onbuild(docker, entry["base"]))
        if triggers:
            if auto:
                return _dockerfile_buildkit_impl(ctx, "line " + str(entry["instruction"]["line"]) + " uses a base image with ONBUILD triggers", True)
            _dockerfile_fail("dockerfile_onbuild_base", "execution_mode", ctx["label"]["id"] + ": Dockerfile line " + str(entry["instruction"]["line"]) + ": base image " + entry["base"] + " declares ONBUILD triggers, which need the whole build context", "Set execution_mode = \"buildkit\" (or \"auto\") on this target")
    cacheable = _dockerfile_attr(ctx, "cacheable", False)
    platform = _dockerfile_attr(ctx, "platform", "")
    pull = _dockerfile_attr(ctx, "pull", True)
    if cacheable and (pull or not platform):
        _dockerfile_fail("dockerfile_cacheable_requirements", "cacheable", "Cacheable instruction execution requires pull = false and an explicit platform", "Set pull = false and platform = \"linux/<architecture>\", or set cacheable = false")
    if cacheable:
        _dockerfile_require_pinned_bases(ctx, instructions, required_stages)
    _dockerfile_platform(platform)
    epoch = _dockerfile_attr(ctx, "source_date_epoch", 0)
    if epoch < 0:
        _dockerfile_fail("dockerfile_invalid_source_date_epoch", "source_date_epoch", "source_date_epoch must be non-negative", "Set source_date_epoch to 0 or a positive Unix timestamp")
    named = _dockerfile_named_contexts(ctx)
    environment = _dockerfile_environment()
    global_args = []
    stages = {}
    aliases = {}
    stage_platforms = {}
    stage_cmd = None
    stage = -1
    previous = None
    snapshots = []
    snapshot_depths = {}
    actions_by_depth = {}
    ignore = _dockerfile_existing_ignore(ctx, dockerfile, context)
    ignore_rules = host_file_read(workspace_root() + "/" + _dockerfile_workspace_path(ctx, ignore)) if ignore else ""
    selected_target = _dockerfile_attr(ctx, "target", "")
    context_path = _dockerfile_workspace_path(ctx, context)
    empty_context = ctx["scratch_dir"] + "/empty-context"
    named_refs = {}
    for name, entry in named.items():
        if entry["layout"]:
            stages[name] = entry["path"]
            named_refs[name] = entry.get("ref", "")
    shared = {
        "dockerfile": _dockerfile_workspace_path(ctx, dockerfile),
        "docker": docker,
        "builder": builder,
        "platform": platform,
        "pull": pull,
        "cacheable": cacheable,
        "identity": identity,
        "environment": environment,
        "global_args": global_args,
        "ignore_rules": ignore_rules,
        "empty_context": empty_context,
        "context_path": context_path,
        "actions_by_depth": actions_by_depth,
        "snapshot_depths": snapshot_depths,
        "snapshots": snapshots,
    }
    pending = []
    for ordinal, instruction in enumerate(commands):
        opcode = instruction["opcode"]
        if opcode == "ARG" and stage < 0:
            global_args.append(instruction["text"])
            continue
        if instruction["stage"] not in required_stages:
            if opcode == "FROM":
                if pending:
                    previous = _dockerfile_declare_group(ctx, shared, pending, previous, stage_platforms.get(str(stage), ""), stage_cmd, {}, [])
                    stages[str(stage)] = previous
                    stage_cmd = _dockerfile_group_cmd(pending, stage_cmd)
                    pending = []
                stage = instruction["stage"]
            continue
        if opcode == "ARG":
            continue
        contexts = {}
        text = instruction["text"]
        if opcode == "FROM":
            if pending:
                previous = _dockerfile_declare_group(ctx, shared, pending, previous, stage_platforms.get(str(stage), ""), stage_cmd, {}, [])
                stages[str(stage)] = previous
                pending = []
            stage = instruction["stage"]
            words = _dockerfile_words(instruction["argument"])
            base_index = 1 if words[0].startswith("--platform=") else 0
            base = _dockerfile_expand(words[base_index], global_values)
            alias = words[-1] if len(words) >= 3 and words[-2].upper() == "AS" else str(stage)
            parent = aliases.get(base.lower(), base)
            previous = stages.get(parent)
            stage_cmd = None
            stage_platform = words[0] if base_index else stage_platforms.get(parent, "")
            stage_platforms[str(stage)] = stage_platform
            aliases[str(stage)] = str(stage)
            if alias != str(stage):
                aliases[alias.lower()] = str(stage)
            if previous:
                contexts["once_previous"] = {"path": previous, "layout": True, "ref": named_refs.get(parent, "")}
                text = "FROM " + (stage_platform + " " if stage_platform else "") + "once_previous"
            else:
                text = "FROM " + " ".join(words[:base_index + 1])
            previous = _dockerfile_declare_from(ctx, shared, instruction, str(ordinal + 1), text, contexts, bool(previous))
            stages[str(stage)] = previous
            continue
        if previous == None:
            _dockerfile_fail("dockerfile_missing_from", "dockerfile", "Dockerfile line " + str(instruction["line"]) + ": instruction requires a preceding FROM", "Add a FROM instruction before line " + str(instruction["line"]))
        for word in _dockerfile_flags(instruction["argument"]) if opcode in ["COPY", "ADD", "RUN"] else []:
            if word.startswith("--from="):
                name = word[len("--from="):]
                key = aliases.get(name.lower(), name)
                if key in named:
                    contexts["once_source"] = named[key]
                    text = text.replace("--from=" + name, "--from=once_source", 1)
                elif stages.get(key):
                    contexts["once_source"] = {"path": stages[key], "layout": True}
                    text = text.replace("--from=" + name, "--from=once_source", 1)
            elif opcode == "RUN" and word.startswith("--mount="):
                for option in word[len("--mount="):].split(","):
                    if option.startswith("from="):
                        name = option[len("from="):]
                        key = aliases.get(name.lower(), name)
                        dependency = None if name.isdigit() else (named[key] if key in named else ({"path": stages[key], "layout": True} if stages.get(key) else None))
                        if dependency:
                            context_name = "once_stage_" + key
                            contexts[context_name] = dependency
                            text = _dockerfile_replace_mount_source(text, name, context_name)
        entry = {"body": text, "opcode": opcode, "line": instruction["line"], "number": str(ordinal + 1), "arg_lines": instruction.get("arg_lines") or []}
        if pending and pending[-1]["arg_lines"] != entry["arg_lines"]:
            previous = _dockerfile_declare_group(ctx, shared, pending, previous, stage_platforms[str(stage)], stage_cmd, {}, [])
            stages[str(stage)] = previous
            stage_cmd = _dockerfile_group_cmd(pending, stage_cmd)
            pending = []
        pending.append(entry)
        if opcode in ["RUN", "COPY", "ADD"]:
            previous = _dockerfile_declare_group(ctx, shared, pending, previous, stage_platforms[str(stage)], stage_cmd, contexts, _dockerfile_copy_inputs(ctx, instruction, context, inputs, symlinks))
            stages[str(stage)] = previous
            stage_cmd = _dockerfile_group_cmd(pending, stage_cmd)
            pending = []
    if pending:
        previous = _dockerfile_declare_group(ctx, shared, pending, previous, stage_platforms.get(str(stage), ""), stage_cmd, {}, [])
        stages[str(stage)] = previous
        pending = []
    for depth in sorted(actions_by_depth.keys()):
        for declaration in actions_by_depth[depth]:
            run_action(**declaration)
    if selected_target:
        previous = stages.get(aliases.get(selected_target.lower(), selected_target))
    if previous == None:
        _dockerfile_fail("dockerfile_target_stage_not_found", "target", "Dockerfile target stage not found: " + selected_target + "; available stages: " + ", ".join(sorted(aliases.keys())), "Set target to one of the available stage names or remove it")
    selected_stage = aliases.get(selected_target.lower(), selected_target) if selected_target else str(stage)
    provider = _dockerfile_export_snapshot(ctx, docker, identity, environment, previous, snapshots, findings, platform, builder, stage_platforms.get(selected_stage, ""), global_args)
    provider["affected_inputs"] = inputs
    provider["input_discovery"] = discovery
    provider["onbuild"] = _dockerfile_has_onbuild(instructions)
    return provider

def _dockerfile_group_cmd(entries, inherited):
    cmd = inherited
    for entry in entries:
        if entry["opcode"] == "CMD":
            cmd = entry["body"]
    return cmd

def _dockerfile_group_text(stage_platform, entries, inherited_cmd):
    parts = ["FROM " + (stage_platform + " " if stage_platform else "") + "once_previous"] + entries[-1]["arg_lines"]
    seen_cmd = False
    for entry in entries:
        parts.append(entry["body"])
        if entry["opcode"] == "CMD":
            seen_cmd = True
        elif entry["opcode"] == "ENTRYPOINT" and inherited_cmd and not seen_cmd:
            parts.append(inherited_cmd)
    return "\n".join(parts)

def _dockerfile_declare_group(ctx, shared, entries, previous, stage_platform, inherited_cmd, extra_contexts, copy_inputs):
    contexts = {"once_previous": {"path": previous, "layout": True}}
    contexts.update(extra_contexts)
    last = entries[-1]
    return _dockerfile_declare_step(ctx, shared, last, last["number"], _dockerfile_group_text(stage_platform, entries, inherited_cmd), contexts, False, copy_inputs, [entry["line"] for entry in entries])

def _dockerfile_declare_from(ctx, shared, instruction, number, text, contexts, from_layout):
    entry = {"opcode": "FROM", "line": instruction["line"], "number": number}
    return _dockerfile_declare_step(ctx, shared, entry, number, text, contexts, not from_layout, [], [instruction["line"]])

def _dockerfile_declare_step(ctx, shared, entry, number, text, contexts, pull_base, copy_inputs, lines):
    step_inputs = []
    for context in contexts.values():
        step_inputs.extend(context.get("inputs") or [context["path"]])
    step_inputs.extend(copy_inputs)
    recipe = declare_output("instructions/" + number + "/Dockerfile")
    rules = recipe + ".dockerignore"
    snapshot = declare_output("instructions/" + number + "/image")
    metadata = declare_output("instructions/" + number + "/metadata.json")
    write_path(recipe, "\n".join(shared["global_args"]) + "\n" + text + "\n")
    write_path(rules, shared["ignore_rules"] + "\n**/.once\n")
    argv = _dockerfile_instruction_argv(ctx, shared["docker"], recipe, metadata, shared["platform"], shared["builder"])
    argv.extend(["--output", "type=oci,tar=false,dest=" + snapshot + (",rewrite-timestamp=true" if _dockerfile_attr(ctx, "reproducible_layers", False) else "")])
    if pull_base and shared["pull"]:
        argv.append("--pull")
    argv.extend(_dockerfile_context_flags(contexts))
    argv.append(shared["context_path"] if copy_inputs else shared["empty_context"])
    depth = max([shared["snapshot_depths"].get(context["path"], 0) for context in contexts.values()] + [0]) + 1
    if depth not in shared["actions_by_depth"]:
        shared["actions_by_depth"][depth] = []
    identifier = ctx["label"]["id"] + ":" + number + ":" + entry["opcode"].lower()
    shared["actions_by_depth"][depth].append({
        "argv": argv,
        "inputs": _unique(step_inputs + [recipe, rules]),
        "outputs": [snapshot, metadata],
        "clean_paths": [snapshot, metadata],
        "create_dirs": [] if copy_inputs else [shared["empty_context"]],
        "env": shared["environment"],
        "sandbox": "copied-inputs",
        "cacheable": shared["cacheable"],
        "depends_on_prior_actions": False,
        "toolchain_identity": shared["identity"],
        "identifier": identifier,
        "display_name": "Container " + entry["opcode"] + " · line " + str(entry["line"]),
        "source_files": _action_source_files([shared["dockerfile"]] + copy_inputs),
    })
    shared["snapshot_depths"][snapshot] = depth
    shared["snapshots"].append({"line": entry["line"], "lines": lines, "instruction": entry["opcode"], "snapshot": snapshot, "identifier": identifier})
    return snapshot

def _dockerfile_tags(ctx):
    tags = []
    primary = _dockerfile_attr(ctx, "tag", _dockerfile_default_tag(ctx))
    if primary:
        tags.append(primary)
    for extra in _dockerfile_attr(ctx, "tags", []):
        if extra and extra not in tags:
            tags.append(extra)
    return tags

def _dockerfile_instruction_argv(ctx, docker, recipe, metadata, platform, builder):
    argv = [docker, "buildx", "build", "--file", recipe, "--metadata-file", metadata, "--progress", "plain", "--provenance=false", "--sbom=false", "--network", _dockerfile_attr(ctx, "network", "default")]
    if builder:
        argv.extend(["--builder", builder])
    if platform:
        argv.extend(["--platform", platform])
    if _dockerfile_attr(ctx, "network", "default") == "host":
        argv.extend(["--allow", "network.host"])
    if _dockerfile_attr(ctx, "no_cache", False):
        argv.append("--no-cache")
    arguments = _dockerfile_build_args(ctx, _dockerfile_attr(ctx, "source_date_epoch", 0))
    for name in sorted(arguments.keys()):
        argv.extend(["--build-arg", name + "=" + arguments[name]])
    for reference in _dockerfile_attr(ctx, "cache_from", []):
        argv.extend(["--cache-from", "type=registry,ref=" + _dockerfile_cache_reference(reference)])
    return argv

def _dockerfile_export_snapshot(ctx, docker, identity, environment, snapshot, snapshots, findings, platform, builder, stage_platform, global_args):
    output_format = _dockerfile_attr(ctx, "format", "docker")
    archive = declare_output(ctx["label"]["name"] + (".oci.tar" if output_format == "oci" else ".docker.tar"))
    metadata = declare_output("build-metadata.json")
    recipe = declare_output("export/Dockerfile")
    plan = declare_output("instruction-plan.json")
    write_path(recipe, "\n".join(global_args) + "\nFROM " + (stage_platform + " " if stage_platform else "") + "once_previous\n")
    argv = _dockerfile_instruction_argv(ctx, docker, recipe, metadata, platform, builder)
    exported = declare_output("export/layout")
    argv.extend(["--output", "type=" + output_format + ",dest=" + archive + ",rewrite-timestamp=true"])
    argv.extend(["--output", "type=oci,tar=false,dest=" + exported + ",rewrite-timestamp=true"])
    tags = _dockerfile_tags(ctx)
    if not tags and output_format == "docker":
        _dockerfile_fail("dockerfile_empty_tag", "tag", "tag must not be empty for Docker archive output", "Set tag to an image reference such as name:latest")
    for tag in tags:
        argv.extend(["--tag", tag])
    for name, value in _dockerfile_attr(ctx, "labels", {}).items():
        argv.extend(["--label", name + "=" + value])
    for name, value in _dockerfile_attr(ctx, "annotations", {}).items():
        argv.extend(["--annotation", name + "=" + value])
    argv.extend(_dockerfile_context_flags({"once_previous": {"path": snapshot, "layout": True}}))
    argv.append(".")
    write_path(plan, _json_encode({"instructions": snapshots, "diagnostics": findings}))
    run_action(
        display_name = "Export container filesystem · " + ctx["label"]["name"],
        source_files = _action_source_files([recipe, snapshot]),
        argv = argv,
        inputs = [recipe, snapshot],
        outputs = [archive, metadata, exported],
        clean_paths = [archive, metadata, exported],
        env = environment,
        sandbox = "copied-inputs",
        cacheable = _dockerfile_attr(ctx, "cacheable", False),
        depends_on_prior_actions = False,
        toolchain_identity = identity,
        identifier = ctx["label"]["id"] + ":dockerfile-export",
    )
    return {"container_image": True, "dockerfile_image": True, "label_id": ctx["label"]["id"], "target_kind": "dockerfile_image", "archive": archive, "metadata": metadata, "layout": exported, "plan": plan, "execution_mode": "instructions", "diagnostics": findings, "format": output_format, "tag": tags[0] if tags else "", "tags": tags, "platform": _dockerfile_platform(platform), "default_output": archive}
