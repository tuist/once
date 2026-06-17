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

def rule(kind, docs, attrs = [], deps = [], providers = [], capabilities = [], examples = [], impl = None):
    return {
        "kind": kind,
        "docs": docs,
        "attrs": attrs,
        "deps": deps,
        "providers": providers,
        "capabilities": capabilities,
        "examples": examples,
        "impl": impl,
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

def _package_relative(ctx, path):
    if not path:
        return path
    if path.startswith("/") or path.startswith("."):
        return path
    package = ctx["label"]["package"]
    if package:
        return package + "/" + path
    return path

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

def _shell_quote(value):
    if not value:
        return "''"
    return "'" + value.replace("'", "'\"'\"'") + "'"
