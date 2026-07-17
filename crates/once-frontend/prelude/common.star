def attr(name, ty, required = False, default = None, docs = "", configurable = True):
    return {
        "name": name,
        "ty": ty,
        "required": required,
        "default": default,
        "docs": docs,
        "configurable": configurable,
    }

def dep(name, expected_providers, docs = ""):
    return {
        "name": name,
        "expected_providers": expected_providers,
        "docs": docs,
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

def example(slug, name, use_when, path = None):
    return {
        "_once_example": True,
        "slug": slug,
        "name": name,
        "use_when": use_when,
        "path": path or ("examples/" + slug),
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

def target_kind(kind = None, docs = "", attrs = [], deps = [], providers = [], capabilities = [], examples = [], impl = None, tools = [], source_references = []):
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
    }

def rule(kind = None, docs = "", attrs = [], deps = [], providers = [], capabilities = [], examples = [], impl = None, tools = [], source_references = []):
    return target_kind(
        kind = kind,
        docs = docs,
        attrs = attrs,
        deps = deps,
        providers = providers,
        capabilities = capabilities,
        examples = examples,
        impl = impl,
        tools = tools,
        source_references = source_references,
    )

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
    path_like = False
    for character in requested:
        if character == "/" or character == "\\":
            path_like = True
            break
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

def _apple_materialize_native_dep(ctx, dep):
    return dep

def _android_materialize_native_dep(ctx, dep):
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
        else:
            out.append(ch)
    out.append("\"")
    return "".join(out)

def _run_result_json(target_id):
    return "{\"schema\":\"once.run_result.v1\",\"target\":" + _json_string(target_id) + ",\"exit_code\":0}\n"
