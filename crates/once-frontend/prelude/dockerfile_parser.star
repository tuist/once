def _dockerfile_words(text):
    words = []
    word = ""
    quote = ""
    escaped = False
    for character in text.elems():
        if escaped:
            word += character
            escaped = False
        elif character == "\\" and quote != "'":
            escaped = True
        elif quote:
            if character == quote:
                quote = ""
            else:
                word += character
        elif character in ["'", '"']:
            quote = character
        elif character in [" ", "\t", "\n"]:
            if word:
                words.append(word)
                word = ""
        else:
            word += character
    if quote or escaped:
        _dockerfile_fail("dockerfile_malformed", "dockerfile", "Unterminated Dockerfile word: " + text, "Close the quote or remove the trailing backslash in the Dockerfile")
    if word:
        words.append(word)
    return words

def _dockerfile_fail(code, attribute, message, repair):
    _diagnostic_fail(code, attribute, message, repair)

_DOCKERFILE_OPCODES = ["FROM", "ARG", "RUN", "COPY", "ADD", "ENV", "WORKDIR", "USER", "LABEL", "EXPOSE", "VOLUME", "CMD", "ENTRYPOINT", "SHELL", "STOPSIGNAL", "HEALTHCHECK", "MAINTAINER", "ONBUILD"]

def _dockerfile_extended_expansion(value):
    for segment in value.split("${")[1:]:
        name = segment.split("}")[0] if "}" in segment else segment
        if not name or not (name[0].isalpha() or name[0] == "_"):
            return True
        for character in name.elems():
            if not (character.isalnum() or character == "_"):
                return True
    return False

def _dockerfile_unsupported_reason(opcode, argument, escape):
    if opcode not in _DOCKERFILE_OPCODES:
        return "unknown instruction " + opcode
    if opcode == "ONBUILD":
        return "ONBUILD instructions"
    if opcode == "FROM" and not _dockerfile_words(argument):
        return "a FROM instruction without an image"
    if opcode in ["RUN", "COPY", "ADD"] and "<<" in argument:
        return "heredoc syntax"
    if opcode in ["RUN", "COPY", "ADD"]:
        for token in argument.split():
            if token.startswith("--from=") or token.startswith("--mount="):
                if '"' in token or "'" in token:
                    return "quoted --from or --mount values"
                if "$" in token and ("from=" in token):
                    return "variable stage references"
    if opcode == "ARG":
        if len(_dockerfile_words(argument)) != 1:
            return "multiple ARG declarations in one instruction"
        if _dockerfile_extended_expansion(argument):
            return "extended ARG expansion"
        for name in ["TARGETPLATFORM", "TARGETOS", "TARGETARCH", "TARGETVARIANT", "BUILDPLATFORM", "BUILDOS", "BUILDARCH", "BUILDVARIANT"]:
            if "$" + name in argument or "${" + name in argument:
                return "automatic platform arguments in ARG defaults"
    return None

def _dockerfile_instructions(source):
    instructions = []
    pending = ""
    start = 0
    escape = "\\"
    directives = True
    for index, raw in enumerate(source.split("\n")):
        stripped = raw.strip()
        if directives and stripped.startswith("#"):
            body = stripped[1:].strip()
            if body.startswith("escape="):
                escape = body[len("escape="):].strip()
                if escape not in ["\\", "`"]:
                    _dockerfile_fail("dockerfile_malformed", "dockerfile", "Dockerfile line " + str(index + 1) + ": escape must be a backslash or backtick", "Set the escape directive to a backslash or a backtick")
                instructions.append({"opcode": "#", "argument": "escape", "text": stripped, "line": index + 1, "unsupported": "a custom escape character" if escape == "`" else None, "directive": True})
            elif body.startswith("check="):
                instructions.append({"opcode": "#", "argument": "check", "text": stripped, "line": index + 1, "unsupported": "a check directive", "directive": True})
            elif body.startswith("syntax="):
                instructions.append({"opcode": "#", "argument": "syntax", "text": stripped, "line": index + 1, "unsupported": "a custom syntax frontend", "directive": True, "frontend": body[len("syntax="):].strip()})
            continue
        if stripped and not stripped.startswith("#"):
            directives = False
        if not stripped or stripped.startswith("#"):
            continue
        line = raw.rstrip()
        if not pending:
            start = index + 1
            line = line.lstrip()
        if line.endswith(escape):
            pending += line[:-len(escape)]
            continue
        text = pending + line
        pending = ""
        opcode = text.split()[0].upper()
        argument = text[len(opcode):].strip()
        instructions.append({"opcode": opcode, "argument": argument, "text": text, "line": start, "unsupported": _dockerfile_unsupported_reason(opcode, argument, escape)})
    if pending:
        _dockerfile_fail("dockerfile_malformed", "dockerfile", "Dockerfile line " + str(start) + ": unfinished continuation", "Finish or remove the trailing line continuation")
    return instructions

def _dockerfile_commands(instructions):
    return [instruction for instruction in instructions if not instruction.get("directive")]

def _dockerfile_first_unsupported(instructions):
    for instruction in instructions:
        if instruction.get("unsupported"):
            return instruction
    return None

def _dockerfile_unsupported_message(instruction):
    return "Dockerfile line " + str(instruction["line"]) + ": " + instruction["unsupported"] + " cannot be translated into instruction actions; use execution_mode = \"buildkit\""

def _dockerfile_flags(argument):
    flags = []
    for word in _dockerfile_words(argument):
        if not word.startswith("--"):
            break
        flags.append(word)
    return flags

def _dockerfile_expand(value, arguments):
    for name in sorted(arguments.keys(), reverse = True):
        value = value.replace("${" + name + "}", arguments[name])
        value = value.replace("$" + name, arguments[name])
    return value

def _dockerfile_arg_value(raw, scope):
    value = raw.strip()
    if len(value) > 1 and value[0] == "'" and value[-1] == "'":
        return value[1:-1]
    if len(value) > 1 and value[0] == '"' and value[-1] == '"':
        value = value[1:-1]
    return _dockerfile_expand(value, scope)

def _dockerfile_global_arguments(ctx, instructions):
    supplied = dict(_dockerfile_attr(ctx, "build_args", {}))
    arguments = {}
    for instruction in _dockerfile_commands(instructions):
        if instruction["opcode"] == "FROM":
            break
        if instruction["opcode"] != "ARG":
            continue
        parts = instruction["argument"].split("=", 1)
        if parts[0] in supplied:
            arguments[parts[0]] = supplied[parts[0]]
        elif len(parts) == 2:
            arguments[parts[0]] = _dockerfile_arg_value(parts[1], arguments)
    return arguments

def _dockerfile_quote_arg(value):
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"').replace("$", "\\$") + '"'

def _dockerfile_resolve(value, scope):
    if "\\" in value:
        return (None, False)
    pieces = value.split("$")
    text = pieces[0]
    for piece in pieces[1:]:
        if piece.startswith("{"):
            end = piece.find("}")
            if end < 0:
                return (None, False)
            name = piece[1:end]
            rest = piece[end + 1:]
        else:
            length = 0
            for character in piece.elems():
                if character.isalnum() or character == "_":
                    length += 1
                else:
                    break
            name = piece[:length]
            rest = piece[length:]
        if not name or name not in scope:
            return (None, False)
        text += scope[name] + rest
    return (text, True)

def _dockerfile_resolve_word(raw, scope):
    value = raw.strip()
    if len(value) > 1 and value[0] == "'" and value[-1] == "'":
        value = value[1:-1]
        if "'" in value or '"' in value:
            return (None, False)
        return (value, True)
    if len(value) > 1 and value[0] == '"' and value[-1] == '"':
        value = value[1:-1]
    if "'" in value or '"' in value:
        return (None, False)
    return _dockerfile_resolve(value, scope)

def _dockerfile_env_pairs(argument):
    words = _dockerfile_words(argument)
    if not words:
        return []
    if "=" in words[0]:
        pairs = []
        for word in words:
            name, separator, value = word.partition("=")
            if not separator:
                return None
            pairs.append((name, value))
        return pairs
    return [(words[0], argument.strip()[len(words[0]):].strip())]

def _dockerfile_annotate(ctx, instructions):
    commands = _dockerfile_commands(instructions)
    supplied = dict(_dockerfile_attr(ctx, "build_args", {}))
    global_values = _dockerfile_global_arguments(ctx, instructions)
    states = {}
    aliases = {}
    stage = -1
    state = None
    for instruction in commands:
        opcode = instruction["opcode"]
        if opcode == "FROM":
            stage += 1
            words = _dockerfile_words(instruction["argument"])
            base_index = 1 if words and words[0].startswith("--platform=") else 0
            base = _dockerfile_expand(words[base_index], global_values) if len(words) > base_index else ""
            parent = aliases.get(base.lower())
            if parent != None:
                inherited = states[parent]
                state = {"scope": dict(inherited["scope"]), "args": dict(inherited["args"]), "order": list(inherited["order"])}
            else:
                state = {"scope": {}, "args": {}, "order": []}
            states[str(stage)] = state
            aliases[str(stage)] = str(stage)
            if len(words) >= 3 and words[-2].upper() == "AS":
                aliases[words[-1].lower()] = str(stage)
            continue
        if state == None or instruction.get("unsupported"):
            continue
        if opcode == "ARG":
            name, separator, default = instruction["argument"].strip().partition("=")
            if name in supplied:
                value = supplied[name]
            elif separator:
                value, resolved = _dockerfile_resolve_word(default, state["scope"])
                if not resolved:
                    instruction["unsupported"] = "an ARG default the translator cannot evaluate (quotes, escapes, or variables only the image defines)"
                    continue
            elif state["args"].get(name) != None:
                value = state["args"][name]
            elif name in global_values:
                value = global_values[name]
            else:
                value = None
            if "\n" in (value or ""):
                instruction["unsupported"] = "an ARG value that spans lines"
                continue
            if name not in state["args"]:
                state["order"].append(name)
            state["args"][name] = value
            if value == None:
                state["scope"].pop(name, None)
            else:
                state["scope"][name] = value
        elif opcode == "ENV":
            pairs = _dockerfile_env_pairs(instruction["argument"])
            argument = instruction["argument"]
            quoted = "\\" in argument or "'" in argument or '"' in argument
            before = dict(state["scope"])
            for name, value in pairs or []:
                resolved_value, resolved = _dockerfile_resolve(value, before)
                if resolved and not quoted:
                    state["scope"][name] = resolved_value
                else:
                    state["scope"].pop(name, None)
            if pairs == None:
                state["scope"] = {}
        instruction["arg_lines"] = ["ARG " + name + ("=" + _dockerfile_quote_arg(state["args"][name]) if state["args"][name] != None else "") for name in state["order"]]
    return instructions

def _dockerfile_selected_stages(ctx, instructions):
    aliases = {}
    dependencies = {}
    stage = -1
    arguments = _dockerfile_global_arguments(ctx, instructions)
    for instruction in _dockerfile_commands(instructions):
        opcode = instruction["opcode"]
        references = []
        if opcode == "FROM":
            stage += 1
            dependencies[stage] = []
            words = _dockerfile_words(instruction["argument"])
            base = _dockerfile_expand(words[1] if words[0].startswith("--platform=") else words[0], arguments)
            references.append(base)
        elif opcode in ["COPY", "ADD", "RUN"]:
            for word in _dockerfile_flags(instruction["argument"]):
                if word.startswith("--from="):
                    references.append(word[len("--from="):])
                elif word.startswith("--mount="):
                    for option in word[len("--mount="):].split(","):
                        if option.startswith("from=") and not option[len("from="):].isdigit():
                            references.append(option[len("from="):])
        for reference in references:
            dependency = aliases.get(reference.lower())
            if dependency != None and dependency < stage:
                dependencies[stage].append(dependency)
        if stage >= 0:
            aliases[str(stage)] = stage
        if opcode == "FROM" and len(words) >= 3 and words[-2].upper() == "AS":
            aliases[words[-1].lower()] = stage
        instruction["stage"] = stage
    target = _dockerfile_attr(ctx, "target", "")
    selected = aliases.get(target.lower()) if target else stage
    if selected == None or selected < 0:
        _dockerfile_fail("dockerfile_target_stage_not_found", "target", "Dockerfile target stage not found: " + target + "; available stages: " + ", ".join(sorted(aliases.keys())), "Set target to one of the available stage names or remove it")
    required = [selected]
    for index in reversed(range(stage + 1)):
        if index in required:
            required.extend(dependencies[index])
    return (_unique(required), arguments)

def _dockerfile_finding(ctx, instruction, code, message, repair, severity = "warning"):
    return {
        "code": code,
        "severity": severity,
        "target": ctx["label"]["id"],
        "attribute": "dockerfile",
        "line": instruction["line"],
        "instruction": instruction["opcode"],
        "message": message,
        "repairs": [repair],
        "confidence": "advisory",
    }

def _dockerfile_image_references(ctx, instructions):
    references = []
    known = _dockerfile_context_names(ctx)
    stages = []
    stage_index = -1
    arguments = _dockerfile_global_arguments(ctx, instructions)
    for instruction in _dockerfile_commands(instructions):
        opcode = instruction["opcode"]
        argument = instruction["argument"]
        if opcode == "FROM" and _dockerfile_words(argument):
            words = _dockerfile_words(argument)
            base_index = 1 if words[0].startswith("--platform=") and len(words) > 1 else 0
            base = _dockerfile_expand(words[base_index], arguments)
            stage_index += 1
            internal = base == "scratch" or base.lower() in stages or base in known
            pinned = internal or "@sha256:" in base
            references.append({"instruction": instruction, "base": base, "pinned": pinned, "external": not internal, "stage": stage_index, "role": "base"})
            stages.append(str(stage_index))
            if len(words) >= 3 and words[-2].upper() == "AS":
                stages.append(words[-1].lower())
        elif opcode in ["COPY", "ADD", "RUN"] and stage_index >= 0:
            for word in _dockerfile_flags(argument):
                names = []
                if word.startswith("--from="):
                    names.append(word[len("--from="):])
                elif word.startswith("--mount="):
                    for option in word[len("--mount="):].split(","):
                        if option.startswith("from="):
                            names.append(option[len("from="):])
                for name in names:
                    from_flag = word.startswith("--from=")
                    named_stage = name.lower() in stages and (from_flag or not name.isdigit())
                    pinned = named_stage or "@sha256:" in name or name in known or "$" in name
                    references.append({"instruction": instruction, "base": name, "pinned": pinned, "stage": stage_index, "role": "source"})
    return references

def _dockerfile_base_images(ctx, instructions):
    return [entry for entry in _dockerfile_image_references(ctx, instructions) if entry["role"] == "base"]

def _dockerfile_require_pinned_bases(ctx, instructions, required_stages):
    for entry in _dockerfile_image_references(ctx, instructions):
        if not entry["pinned"] and entry["stage"] in required_stages:
            role = "base images" if entry["role"] == "base" else "images copied from"
            _dockerfile_fail("dockerfile_base_image_not_pinned", "cacheable", ctx["label"]["id"] + ": Dockerfile line " + str(entry["instruction"]["line"]) + ": cacheable = true requires " + role + " pinned by digest, found " + entry["base"] + "; use name@sha256:<digest>", "Pin the image on line " + str(entry["instruction"]["line"]) + " as name@sha256:<digest>, or set cacheable = false")

def _dockerfile_findings(ctx, instructions):
    findings = []
    for instruction in instructions:
        if instruction.get("frontend") and "@sha256:" not in instruction["frontend"]:
            findings.append(_dockerfile_finding(ctx, instruction, "mutable_syntax_frontend", "Syntax frontend is not pinned by digest: " + instruction["frontend"], "Pin the syntax directive to a digest, for example docker/dockerfile:1@sha256:<digest>. BuildKit pulls the frontend image from a registry."))
    for entry in _dockerfile_image_references(ctx, instructions):
        if not entry["pinned"]:
            code = "mutable_base_image" if entry["role"] == "base" else "mutable_copy_source_image"
            noun = "Base image" if entry["role"] == "base" else "Image copied from"
            findings.append(_dockerfile_finding(ctx, entry["instruction"], code, noun + " is not pinned by digest: " + entry["base"], "Pin the image to a digest for the selected platform; version tags can also change."))
    for instruction in _dockerfile_commands(instructions):
        if instruction["opcode"] != "RUN":
            continue
        argument = instruction["argument"]
        if ("curl " in argument or "wget " in argument) and "|" in argument:
            findings.append(_dockerfile_finding(ctx, instruction, "unverified_network_pipe", "Downloaded content is piped into another command without a visible checksum check.", "Inspect the pipeline and verify downloaded bytes against a pinned checksum before consuming them."))
        if "apt-get update" in argument or "apt-get upgrade" in argument or "apk add" in argument:
            findings.append(_dockerfile_finding(ctx, instruction, "mutable_package_repository", "Package installation may depend on changing repository contents.", "Check whether repositories use immutable snapshots and packages are pinned. A snapshot configured in the base image can make this a false positive."))
        if "--mount=type=secret" in argument or "--mount=type=ssh" in argument:
            findings.append(_dockerfile_finding(ctx, instruction, "untracked_mount_contents", "Secret or authentication mount contents are not included in the action cache key.", "Confirm the mount only authenticates access to immutable inputs; otherwise disable caching or declare a non-secret version input."))
    return findings
