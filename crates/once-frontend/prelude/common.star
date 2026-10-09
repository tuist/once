def _action_history(ctx, ecosystem, step, variants = []):
    owner = (ctx.get("label") or {}).get("id") or ""
    if not owner:
        return None
    return action_history_key("once." + ecosystem + ".v1", [owner, step] + variants)

def _action_metadata(package = None, platforms = [], context = []):
    return {"package": package, "platforms": platforms, "context": context}

def _action_platform(scheme, identifier, label = "", usage = "product"):
    return {"scheme": scheme, "id": identifier, "label": label, "usage": usage}

def _action_context(key, value, label = ""):
    return {"key": key, "value": str(value), "label": label}

def _action_package(ecosystem, name, version = "", revision = "", digest = "", origin = ""):
    return {"ecosystem": ecosystem, "name": name, "version": version, "revision": revision, "digest": digest, "origin": origin}

def attr(name, ty, required = False, default = None, docs = "", configurable = True, implemented = True, allowed_values = [], disallowed_values = []):
    return {
        "name": name,
        "ty": ty,
        "required": required,
        "default": default,
        "docs": docs,
        "configurable": configurable,
        "implemented": implemented,
        "allowed_values": allowed_values,
        "disallowed_values": disallowed_values,
    }

def dep(name, expected_providers, docs = "", min_count = 0, max_count = None):
    return {
        "name": name,
        "expected_providers": expected_providers,
        "docs": docs,
        "min_count": min_count,
        "max_count": max_count,
    }

def capability(name, output_groups, requires_outputs = []):
    return {
        "name": name,
        "output_groups": output_groups,
        "requires_outputs": requires_outputs,
    }

def tool(name, executables = []):
    return {
        "name": name,
        "executables": executables or [name],
    }

def example(slug, name, use_when, path = None, platforms = None):
    return {
        "_once_example": True,
        "slug": slug,
        "name": name,
        "use_when": use_when,
        "path": path or ("examples/" + slug),
        "platforms": platforms or [],
    }

def source_reference(system, symbol, url, use_when, content_digest = None):
    return {
        "_once_source_reference": True,
        "system": system,
        "symbol": symbol,
        "url": url,
        "use_when": use_when,
        "content_digest": content_digest,
    }

def native_project(target_kind, markers, name = None, target_name = None, docs = "", inputs = [], exclude = [], input_exclude = [], on_match = "descend", max_depth = 16, requires_tools = [], owns_descendants = False, workspace_markers = []):
    return {
        "_once_native_project": True,
        "target_kind": target_kind,
        "name": name,
        "target_name": target_name,
        "docs": docs,
        "markers": markers,
        "inputs": inputs,
        "exclude": exclude,
        "input_exclude": input_exclude,
        "on_match": on_match,
        "max_depth": max_depth,
        "requires_tools": requires_tools,
        "owns_descendants": owns_descendants,
        "workspace_markers": workspace_markers,
    }

def _native_project_generated_dirs():
    return ["_build", "deps", "target", "third_party", "vendor"]

def target_kind(kind = None, docs = "", attrs = [], deps = [], providers = [], capabilities = [], examples = [], impl = None, resolver = None, tools = [], source_references = []):
    return {
        "_once_target_kind": True,
        "kind": kind,
        "docs": docs,
        "attrs": attrs,
        "deps": deps,
        "providers": providers,
        "capabilities": capabilities,
        "tools": tools,
        "examples": examples,
        "source_references": source_references,
        "impl": impl,
        "resolver": resolver,
    }

def rule(kind = None, docs = "", attrs = [], deps = [], providers = [], capabilities = [], examples = [], impl = None, resolver = None, tools = [], source_references = []):
    return target_kind(
        kind = kind,
        docs = docs,
        attrs = attrs,
        deps = deps,
        providers = providers,
        capabilities = capabilities,
        examples = examples,
        impl = impl,
        resolver = resolver,
        tools = tools,
        source_references = source_references,
    )

def _resolver_snapshot_inputs(files, excluded_paths = []):
    excluded = {}
    for path in excluded_paths:
        if path:
            excluded[path[2:] if path.startswith("./") else path] = True
    return {
        path: content
        for path, content in (files or {}).items()
        if not excluded.get(path)
    }

def _ends_with(value, suffix):
    if len(value) < len(suffix):
        return False
    return value[len(value) - len(suffix):] == suffix

def _filter_by_extensions(paths, extensions):
    out = []
    for path in paths:
        for ext in extensions:
            if _ends_with(path, ext):
                out.append(path)
                break
    return out

def _file_globs(patterns):
    expanded = []
    for pattern in patterns:
        expanded.append(pattern)
        if _ends_with(pattern, "/**"):
            expanded.append(pattern + "/*")
    return glob(expanded)

def _package_relative(ctx, path):
    if not path:
        return path
    if path.startswith("/") or path.startswith("."):
        return path
    package = ctx["label"]["package"]
    if package:
        return package + "/" + path
    return path

def _resolve_host_executable(requested):
    path_like = "/" in requested or "\\" in requested
    resolved = "" if path_like else host_which_optional(requested)
    if resolved or not requested:
        return resolved
    absolute = requested.startswith("/") or (
        len(requested) > 2 and
        requested[1] == ":" and
        (requested[2] == "/" or requested[2] == "\\")
    )
    candidate = requested if absolute else workspace_root() + "/" + requested
    return candidate if host_file_exists(candidate) else ""

def _apple_materialize_native_dep(ctx, dep, state = None):
    return dep

def _android_materialize_native_dep(ctx, dep, state = None):
    return dep

def _parent_dir(path):
    idx = -1
    for i in range(len(path)):
        if path[i] == "/":
            idx = i
    if idx < 0:
        return ""
    return path[:idx]

def _unique(values):
    seen = {}
    out = []
    for value in values:
        if value not in seen:
            seen[value] = True
            out.append(value)
    return out

def _action_source_files(paths):
    return {"_once_workspace_source_files": paths}

def _collect_transitive(deps, key, own_values):
    out = []
    for value in own_values:
        out.append(value)
    for dep in deps:
        for value in dep.get(key) or []:
            out.append(value)
    return _unique(out)

def _unique_args(values, option_arity = {}, forwarder = "", forwarded_option_arity = {}):
    seen = {}
    out = []
    consumed_until = 0
    for index in range(len(values)):
        if index < consumed_until:
            continue
        value = values[index]
        group = [value]
        if forwarder and value == forwarder and index + 1 < len(values):
            forwarded_option = values[index + 1]
            group.append(forwarded_option)
            cursor = index + 2
            for _unused in range(forwarded_option_arity.get(forwarded_option) or 0):
                if cursor < len(values) and values[cursor] == forwarder and cursor + 1 < len(values):
                    group.extend([values[cursor], values[cursor + 1]])
                    cursor += 2
                elif cursor < len(values):
                    group.append(values[cursor])
                    cursor += 1
            consumed_until = cursor
        else:
            arity = option_arity.get(value) or 0
            consumed_until = index + 1 + arity
            if consumed_until > len(values):
                consumed_until = len(values)
            group.extend(values[index + 1:consumed_until])
        key = repr(group)
        if key not in seen:
            seen[key] = True
            out.extend(group)
    return out

def _collect_transitive_args(deps, key, own_values):
    out = list(own_values)
    for dep in deps:
        out.extend(dep.get(key) or [])
    return out

def _once_executable_fields(path, runtime_files = [], os = "", architecture = "", variant = "", linkage = "unknown"):
    return {
        "once_executable": True,
        "executable": {
            "path": path,
            "runtime_files": _unique(runtime_files),
            "os": os,
            "architecture": architecture,
            "variant": variant,
            "linkage": linkage,
        },
    }

def _once_normalize_architecture(value):
    return {
        "aarch64": "arm64",
        "arm64": "arm64",
        "x86_64": "amd64",
        "amd64": "amd64",
        "x86": "386",
        "i386": "386",
        "i686": "386",
    }.get(value) or value

def _once_normalize_os(value):
    return {
        "macos": "darwin",
        "win32": "windows",
    }.get(value) or value

def _configuration_tokens(ctx, extra = []):
    configured = (ctx.get("configuration") or {}).get("tokens") or []
    return _unique(extra + configured + ["default"])

def _is_select_value(value):
    return type(value) == type({}) and len(value) == 1 and type(value.get("select")) == type({})

def _resolve_configured_value(value, tokens, label_id, attr_name):
    if _is_select_value(value):
        branches = value["select"]
        for token in tokens:
            if token in branches:
                return _resolve_configured_value(branches[token], tokens, label_id, attr_name)
        fail(label_id + ": attribute `" + attr_name + "` uses select() without a branch matching the target configuration or a `default` branch")
    if type(value) == type([]):
        return [_resolve_configured_value(item, tokens, label_id, attr_name) for item in value]
    if type(value) == type({}):
        return {key: _resolve_configured_value(item, tokens, label_id, attr_name) for key, item in value.items()}
    return value

def _configured_attr(ctx, name, default, extra_tokens = []):
    value = ctx["attr"].get(name)
    if value == None:
        return default
    return _resolve_configured_value(
        value,
        _configuration_tokens(ctx, extra_tokens),
        ctx["label"]["id"],
        name,
    )

def _configured_attrs(ctx, extra_tokens = []):
    tokens = _configuration_tokens(ctx, extra_tokens)
    return {
        name: _resolve_configured_value(value, tokens, ctx["label"]["id"], name)
        for name, value in ctx["attr"].items()
    }

def _test_unit_suffix(ctx, unit):
    prefix = ctx["label"]["id"] + "::"
    if not unit.startswith(prefix):
        fail("test unit `" + unit + "` does not belong to target `" + ctx["label"]["id"] + "`")
    return unit[len(prefix):]

def _test_output_dir(ctx):
    batch_id = (ctx.get("test") or {}).get("batch_id")
    if batch_id:
        return ctx["build_dir"] + "/test/batches/" + batch_id
    return ctx["build_dir"] + "/test"

def _basename(path):
    normalized = path.replace("\\", "/")
    parts = normalized.split("/")
    return parts[len(parts) - 1]

def _native_library_key(library):
    return (library.get("abi") or "") + "\x00" + (library.get("path") or "")

def _unique_native_libraries(libraries):
    seen = {}
    out = []
    for library in libraries:
        abi = library.get("abi") or ""
        path = library.get("path") or ""
        if not abi or not path:
            continue
        key = _native_library_key(library)
        if key not in seen:
            seen[key] = True
            out.append({"abi": abi, "path": path})
    return out

def _shell_quote(value):
    if not value:
        return "''"
    return "'" + value.replace("'", "'\"'\"'") + "'"

def _powershell_quote(value):
    return "'" + value.replace("'", "''") + "'"

def _json_string(value):
    out = ["\""]
    for ch in value.elems():
        if ch == "\"":
            out.append("\\\"")
        elif ch == "\\":
            out.append("\\\\")
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\r":
            out.append("\\r")
        elif ch == "\t":
            out.append("\\t")
        elif ord(ch) < 32:
            out.append("\\u00" + "0123456789abcdef"[ord(ch) // 16] + "0123456789abcdef"[ord(ch) % 16])
        else:
            out.append(ch)
    out.append("\"")
    return "".join(out)

def _diagnostic_fail(code, attribute, message, repair):
    fail("once.diagnostic.v1 " + _json_encode({"code": code, "message": message, "attribute": attribute, "repairs": [repair]}))

def _json_encode(value):
    if value == None:
        return "null"
    if type(value) == type(True):
        return "true" if value else "false"
    if type(value) == type(0):
        return str(value)
    if type(value) == type(""):
        return _json_string(value)
    if type(value) == type([]):
        return "[" + ",".join([_json_encode(item) for item in value]) + "]"
    if type(value) == type({}):
        return "{" + ",".join([
            _json_string(key) + ":" + _json_encode(value[key])
            for key in sorted(value.keys())
        ]) + "}"
    fail("cannot encode " + type(value) + " as JSON")

def _run_result_json(target_id):
    return "{\"schema\":\"once.run_result.v1\",\"target\":" + _json_string(target_id) + ",\"exit_code\":0}\n"
