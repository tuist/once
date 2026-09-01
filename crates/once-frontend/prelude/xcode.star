# Xcode native integration reader.
#
# The `xcode_workspace` target kind is a graph resolver: it reads an Xcode
# project (`project.pbxproj`), flattens its layered build settings (project,
# target, and `.xcconfig` includes), resolves file references (both classic
# `PBXBuildFile` lists and Xcode 16+ file-system synchronized groups), and
# lowers every `PBXNativeTarget` into the existing Apple target kinds:
#
#   application        -> apple_application
#   framework / static -> apple_library (with framework output)
#   static library     -> apple_library
#   unit-test bundle   -> apple_test_bundle (with the host application wired)
#
# The pbxproj is an OpenStep (NeXTSTEP) plist. Instead of parsing that format,
# the resolver asks the macOS built-in `plutil` to convert it to JSON in one
# step, then walks the object graph the same way Cargo walks `cargo metadata`.
# Schemes (`.xcscheme`) are read to discover which targets are testable so the
# host application is wired for test bundles that declare a test host.

_XCODE_TOOL = tool("xcode", executables = ["plutil", "xcrun"])

# ---------------------------------------------------------------------------
# Paths and pbxproj reading
# ---------------------------------------------------------------------------

def _xcode_workspace_root():
    root = workspace_root()
    return root + "/" if root else ""

def _xcode_abs(path):
    if not path:
        return path
    if path.startswith("/"):
        return path
    return _xcode_workspace_root() + path

def _xcode_project_path(ctx):
    configured = ctx["attr"].get("project")
    if configured:
        package = ctx["label"]["package"]
        return _xcode_join(package, configured) if package else configured
    candidates = [path for path in glob(["*.xcodeproj/project.pbxproj"]) if _ends_with(path, "/project.pbxproj")]
    if len(candidates) == 0:
        fail(ctx["label"]["id"] + ": no `project` attribute was set and no `*.xcodeproj/project.pbxproj` was found in the package")
    if len(candidates) > 1:
        fail(ctx["label"]["id"] + ": multiple Xcode projects found; set the `project` attribute to one of: " + ", ".join(candidates))
    # `glob` always returns workspace-relative paths, including when the
    # native seed belongs to a nested package. The resolver reads project
    # files through the workspace root, so keep that coordinate system.
    return _parent_dir(candidates[0])

def _xcode_is_workspace(path):
    return _ends_with(path, ".xcworkspace") or _ends_with(path, "/contents.xcworkspacedata")

def _xcode_workspace_data_path(workspace_path):
    if _ends_with(workspace_path, "/contents.xcworkspacedata"):
        return workspace_path
    return workspace_path + "/contents.xcworkspacedata"

def _xcode_workspace_dir(workspace_path):
    # The workspace directory is the parent of the `.xcworkspace` bundle. Project
    # references inside `contents.xcworkspacedata` are resolved relative to it.
    if _ends_with(workspace_path, "/contents.xcworkspacedata"):
        return _parent_dir(_parent_dir(workspace_path))
    return _parent_dir(workspace_path)

def _xcode_workspace_location(line):
    # A `location = "group:relative/path"` attribute. Return the reference kind
    # ("group", "container", ...) and the path, or None for other attributes.
    marker = "location = \""
    start = line.find(marker)
    if start < 0:
        return None
    rest = line[start + len(marker):]
    end = rest.find("\"")
    if end < 0:
        return None
    value = rest[:end]
    colon = value.find(":")
    if colon < 0:
        return ("group", value)
    return (value[:colon], value[colon + 1:])

def _xcode_parse_workspace_data(raw, workspace_dir):
    # Resolve the nested `<Group>`/`<FileRef>` tree in `contents.xcworkspacedata`
    # into `.xcodeproj` paths relative to the workspace directory. Each group
    # contributes one path segment; a file reference joins its own location onto
    # the enclosing groups' segments.
    stack = []
    pending = None
    projects = []
    for line in raw.split("\n"):
        stripped = line.strip()
        # A group or file reference opens a scope whose `location` attribute may
        # sit on the same line as the tag or on a following line.
        if stripped.startswith("<Group"):
            pending = "group"
        elif stripped.startswith("<FileRef"):
            pending = "fileref"
        if pending != None and stripped.find("location = \"") >= 0:
            parsed = _xcode_workspace_location(stripped)
            if parsed != None:
                kind, value = parsed
                if kind == "container":
                    base = value
                else:
                    base = value
                    for segment in reversed(stack):
                        base = _xcode_join(segment, base)
                if pending == "group":
                    stack.append(value)
                elif _ends_with(base, ".xcodeproj"):
                    projects.append(_xcode_join(workspace_dir, base) if workspace_dir else base)
            pending = None
        if stripped.find("</Group>") >= 0 and stack:
            stack.pop()
    return projects

def _xcode_workspace_projects(ctx, workspace_path):
    # Enumerate every `.xcodeproj` a workspace references. The workspace index
    # itself can be generated (some projects produce `contents.xcworkspacedata`
    # during setup), so a missing index yields no projects rather than failing.
    workspace_dir = _xcode_workspace_dir(workspace_path)
    data_path = _xcode_workspace_data_path(workspace_path)
    if not host_file_exists(_xcode_abs(data_path)):
        return []
    raw = host_file_read(_xcode_abs(data_path))
    return _xcode_parse_workspace_data(raw, workspace_dir)

def _xcode_project_dir(project_path):
    # The project directory (`SOURCE_ROOT`) is the parent of the `.xcodeproj`
    # bundle, relative to the package. `project_path` is either the bundle path
    # (`App.xcodeproj`) or the manifest path (`App.xcodeproj/project.pbxproj`);
    # normalize both to the bundle's parent directory.
    if _ends_with(project_path, "/project.pbxproj"):
        return _parent_dir(_parent_dir(project_path))
    return _parent_dir(project_path)

def _xcode_pbxproj_path(project_path):
    # `project_path` is either the `.xcodeproj` bundle or its `project.pbxproj`.
    if _ends_with(project_path, "/project.pbxproj"):
        return project_path
    return project_path + "/project.pbxproj"

def _xcode_read_pbxproj(ctx, project_path):
    raw = host_command(["plutil", "-convert", "json", "-o", "-", _xcode_abs(_xcode_pbxproj_path(project_path))])
    return json_decode(raw)

# ---------------------------------------------------------------------------
# Build settings: layering, .xcconfig flattening, and variable expansion
# ---------------------------------------------------------------------------

# Settings that Xcode stores as whitespace- or comma-separated lists even when
# they look scalar in the editor. plutil surfaces these as JSON arrays when the
# pbxproj writes them as `(a, b, c)`, and as strings otherwise.
_XCODE_LIST_SETTINGS = {
    "GCC_PREPROCESSOR_DEFINITIONS": True,
    "SWIFT_ACTIVE_COMPILATION_CONDITIONS": True,
    "OTHER_SWIFT_FLAGS": True,
    "OTHER_LDFLAGS": True,
    "OTHER_CFLAGS": True,
    "OTHER_CPLUSPLUSFLAGS": True,
    "FRAMEWORK_SEARCH_PATHS": True,
    "HEADER_SEARCH_PATHS": True,
    "LIBRARY_SEARCH_PATHS": True,
    "LD_RUNPATH_SEARCH_PATHS": True,
    "SYSTEM_FRAMEWORK_SEARCH_PATHS": True,
    "USER_HEADER_SEARCH_PATHS": True,
    "ALWAYS_SEARCH_USER_PATHS": True,
}

_XCODE_LINKER_OPTION_ARITY = {
    "-alias": 2,
    "-compatibility_version": 1,
    "-current_version": 1,
    "-exported_symbols_list": 1,
    "-filelist": 1,
    "-force_load": 1,
    "-install_name": 1,
    "-order_file": 1,
    "-rpath": 1,
    "-sectcreate": 3,
    "-segprot": 3,
    "-u": 1,
    "-undefined": 1,
    "-unexported_symbols_list": 1,
    "-weak_framework": 1,
}

def _xcode_setting_to_list(value):
    if value == None:
        return []
    if type(value) == type([]):
        out = []
        for item in value:
            out.extend(_xcode_split_setting(str(item)))
        return out
    return _xcode_split_setting(str(value))

def _xcode_split_setting(value):
    stripped = value.strip()
    if not stripped:
        return []
    parts = []
    current = []
    quote = ""
    escaped = False
    for ch in stripped.elems():
        if escaped:
            current.append(ch)
            escaped = False
        elif ch == "\\":
            escaped = True
        elif quote:
            if ch == quote:
                quote = ""
            else:
                current.append(ch)
        elif ch == '"' or ch == "'":
            quote = ch
        elif ch == " " or ch == "\t":
            if current:
                parts.append("".join(current))
                current = []
        else:
            current.append(ch)
    if escaped:
        current.append("\\")
    if current:
        parts.append("".join(current))
    return parts

def _xcode_read_xcconfig(ctx, path, visited = None):
    if not path or not host_file_exists(_xcode_abs(path)):
        return {}
    visited = visited or []
    if path in visited:
        return {}
    visited = visited + [path]
    base_dir = _parent_dir(path)
    content = host_file_read(_xcode_abs(path))
    flattened = {}
    for raw_line in content.split("\n"):
        line = raw_line.strip()
        if not line or line.startswith("//"):
            continue
        include = _xcode_parse_include(line)
        if include != None:
            resolved = include if include.startswith("/") else _xcode_join(base_dir, include)
            flattened = _xcode_merge_settings(flattened, _xcode_read_xcconfig(ctx, resolved, visited))
            continue
        pair = _xcode_parse_setting(line)
        if pair != None:
            flattened = _xcode_merge_settings(flattened, {pair[0]: pair[1]})
    return flattened

def _xcode_parse_include(line):
    trimmed = line
    # Strip trailing comments.
    comment = trimmed.find("//")
    if comment >= 0:
        trimmed = trimmed[:comment].strip()
    if not trimmed.lower().startswith("#include"):
        return None
    rest = trimmed[len("#include"):]
    quote = rest.find('"')
    if quote < 0:
        return None
    end = rest.find('"', quote + 1)
    if end < 0:
        return None
    return rest[quote + 1:end]

def _xcode_parse_setting(line):
    equals = -1
    bracket_depth = 0
    for index in range(len(line)):
        ch = line[index]
        if ch == "[":
            bracket_depth += 1
        elif ch == "]" and bracket_depth > 0:
            bracket_depth -= 1
        elif ch == "=" and bracket_depth == 0:
            equals = index
            break
    if equals <= 0:
        return None
    key = line[:equals].strip()
    value = line[equals + 1:].strip()
    # Strip a trailing semicolon and trailing comment.
    if value.endswith(";"):
        value = value[:-1].strip()
    comment = value.find("//")
    if comment >= 0:
        value = value[:comment].strip()
    if value.startswith('"') and value.endswith('"') and len(value) >= 2:
        value = value[1:-1]
    if not key:
        return None
    return (key, value)

def _xcode_conditional_setting_key(key):
    opening = key.find("[")
    if opening < 0:
        return {"name": key, "conditions": []}
    name = key[:opening].strip()
    conditions = []
    remaining = key[opening:]
    for _ in range(len(remaining) + 1):
        if not remaining:
            break
        if not remaining.startswith("["):
            return None
        closing = remaining.find("]")
        if closing < 0:
            return None
        expression = remaining[1:closing]
        equals = expression.find("=")
        if equals <= 0:
            return None
        parameter = expression[:equals].strip()
        pattern = expression[equals + 1:].strip()
        if not parameter:
            return None
        conditions.append((parameter, pattern))
        remaining = remaining[closing + 1:].strip()
    return {"name": name, "conditions": conditions} if name else None

def _xcode_select_conditional_settings(settings, parameters):
    selected = {}
    conditional = []
    for key, value in settings.items():
        parsed = _xcode_conditional_setting_key(key)
        if parsed == None or not parsed["conditions"]:
            selected[key] = value
            continue
        matches = True
        for parameter, pattern in parsed["conditions"]:
            candidate = parameters.get(parameter)
            if candidate == None:
                if pattern != "*":
                    matches = False
                    break
            elif not _xcode_glob_match(pattern, str(candidate)):
                matches = False
                break
        if matches:
            conditional.append((parsed["name"], value))
    for name, value in conditional:
        selected[name] = value
    return selected

def _xcode_base_config_xcconfig(ctx, objects, config, path_maps):
    # Classic form: a direct file reference to the `.xcconfig`.
    ref = config.get("baseConfigurationReference")
    if ref:
        file_ref = objects.get(ref) or {}
        path = path_maps["files"].get(ref) or _xcode_file_ref_path(ctx, file_ref)
        if path:
            return _xcode_read_xcconfig(ctx, path)
    # Newer form (Xcode 16): an anchor group plus a path relative to it.
    anchor = config.get("baseConfigurationReferenceAnchor")
    relative = config.get("baseConfigurationReferenceRelativePath")
    if anchor and relative:
        anchor_dir = path_maps["groups"].get(anchor)
        if anchor_dir != None:
            return _xcode_read_xcconfig(ctx, _xcode_join(anchor_dir, relative))
    return {}

def _xcode_merge_settings(lower, higher):
    # Overlay higher-priority settings onto lower. List settings keep
    # `$(inherited)` sem a resolved against the lower value.
    merged = {}
    for key, value in lower.items():
        merged[key] = value
    for key, value in higher.items():
        if key in _XCODE_LIST_SETTINGS:
            merged[key] = _xcode_resolve_inherited_list(value, merged.get(key))
        elif value == "$(inherited)":
            merged[key] = merged.get(key, "")
        elif type(value) == type("") and "$(inherited)" in value:
            inherited = merged.get(key, "")
            merged[key] = value.replace("$(inherited)", inherited)
        else:
            merged[key] = value
    return merged

def _xcode_resolve_inherited_list(value, inherited):
    parts = _xcode_setting_to_list(value)
    out = []
    for part in parts:
        if part == "$(inherited)":
            if inherited == None:
                out.append(part)
            else:
                out.extend(_xcode_setting_to_list(inherited))
        else:
            out.append(part)
    return out

# Variable expansion for the subset of build settings that affect compilation.
# Paths expand to package-relative fragments so the lowered Apple targets can
# consume them directly. Unknown variables are left intact; the caller decides
# whether to drop unresolved fragments.
def _xcode_resolve_vars(value, subs):
    if value == None:
        return value
    if type(value) == type([]):
        return [_xcode_resolve_vars(item, subs) for item in value]
    text = str(value)
    # Xcode accepts both `$(NAME)` and `${NAME}` spellings. The latter is
    # especially common in generated configuration files, including build
    # dependency integrations, so normalize both through the same recursive
    # expansion path.
    expanded = text
    for _ in range(16):
        resolved = _xcode_expand_once(expanded, subs, 0, "${", "}")
        resolved = _xcode_expand_once(resolved, subs, 0, "$(", ")")
        resolved = _xcode_expand_bare(resolved, subs)
        if resolved == expanded:
            break
        expanded = resolved
    return expanded

def _xcode_expand_bare(text, subs):
    out = []
    index = 0
    for _ in range(len(text) + 1):
        if index >= len(text):
            break
        if text[index] != "$" or index + 1 >= len(text) or text[index + 1] in ["(", "{"]:
            out.append(text[index])
            index += 1
            continue
        end = index + 1
        for _ in range(len(text) - index):
            if end >= len(text):
                break
            ch = text[end]
            if not ((ch >= "a" and ch <= "z") or (ch >= "A" and ch <= "Z") or (ch >= "0" and ch <= "9") or ch == "_"):
                break
            end += 1
        name = text[index + 1:end]
        if name in subs:
            out.append(str(subs[name]))
        else:
            out.append(text[index:end])
        index = end
    return "".join(out)

def _xcode_identifier_modifier(value, modifier):
    replacement = "-" if modifier == "rfc1034identifier" else "_"
    allow_period = modifier == "rfc1034identifier"
    out = []
    for ch in str(value).elems():
        allowed = (ch >= "a" and ch <= "z") or (ch >= "A" and ch <= "Z") or (ch >= "0" and ch <= "9") or (allow_period and (ch == "." or ch == "-")) or (not allow_period and ch == "_")
        out.append(ch if allowed else replacement)
    result = "".join(out)
    if modifier == "c99extidentifier" and result and result[0] >= "0" and result[0] <= "9":
        result = "_" + result
    return result

def _xcode_expand_once(text, subs, depth, opening, closing):
    if depth > 16:
        return text
    start = text.find(opening)
    if start < 0:
        return text
    end = text.find(closing, start + len(opening))
    if end < 0:
        return text
    expression = text[start + len(opening):end]
    parts = expression.split(":")
    name = parts[0]
    head = text[:start]
    tail = text[end + 1:]
    if name in subs:
        replacement = _xcode_expand_once(str(subs[name]), subs, depth + 1, opening, closing)
        for modifier in parts[1:]:
            if modifier == "rfc1034identifier" or modifier == "c99extidentifier":
                replacement = _xcode_identifier_modifier(replacement, modifier)
    else:
        replacement = opening + expression + closing
    return _xcode_expand_once(head + replacement + tail, subs, depth + 1, opening, closing)

def _xcode_setting_subs(ctx, target_name, product_name, sdkroot, settings = None, project_dir = "", configuration = ""):
    package = (ctx.get("label") or {}).get("package") or ""
    target_build_dir = ".once/out/" + ((package + "/") if package else "") + _xcode_sanitized_target_name(target_name)
    source_root = project_dir or "."
    subs = {
        "SRCROOT": source_root,
        "PROJECT_DIR": source_root,
        "TARGET_NAME": target_name,
        "PRODUCT_NAME": product_name,
        "PRODUCT_MODULE_NAME": product_name.replace("-", "_"),
        "SDKROOT": sdkroot or "",
        "CONFIGURATION": configuration or ctx["attr"].get("configuration") or "Debug",
        "PROJECT_TEMP_DIR": target_build_dir + "/Intermediates",
        "TARGET_TEMP_DIR": target_build_dir + "/Intermediates",
        "CONFIGURATION_TEMP_DIR": target_build_dir + "/Intermediates",
    }
    # A target configuration can introduce arbitrary build-setting names. Make
    # those values available to subsequent expansions, while retaining the
    # adapter-owned values above for paths and target identity.
    for key, value in (settings or {}).items():
        if key not in subs:
            subs[key] = _xcode_scalar(value)
    return subs

def _xcode_variable_names(text):
    names = []
    for remainder in text.split("$(")[1:]:
        name = remainder.split(")")[0]
        if name and name not in names:
            names.append(name)
    for remainder in text.split("${")[1:]:
        name = remainder.split("}")[0]
        if name and name not in names:
            names.append(name)
    return names

def _xcode_effective_settings(ctx, objects, config_list, default_name, project_settings, target_name, path_maps):
    # Resolve the active configuration from a build configuration list, then
    # layer project settings < target xcconfig < target settings.
    configs = (config_list or {}).get("buildConfigurations") or []
    config_id = _xcode_choose_config(objects, configs, default_name, ctx["attr"].get("configuration") or "Debug")
    config = objects.get(config_id) or {}
    xcconfig_settings = _xcode_base_config_xcconfig(ctx, objects, config, path_maps)
    target_settings = config.get("buildSettings") or {}
    layered = _xcode_merge_settings(project_settings, xcconfig_settings)
    layered = _xcode_merge_settings(layered, target_settings)
    return layered

def _xcode_choose_config(objects, configs, default_name, wanted):
    ids = [c for c in configs]
    for config_id in ids:
        config = objects.get(config_id) or {}
        if config.get("name") == wanted:
            return config_id
    for config_id in ids:
        config = objects.get(config_id) or {}
        if config.get("name") == default_name:
            return config_id
    return ids[0] if ids else None

def _xcode_project_settings(ctx, objects, root, configuration, path_maps):
    config_list = objects.get((objects.get(root) or {}).get("buildConfigurationList")) or {}
    default_name = config_list.get("defaultConfigurationName") or "Release"
    return _xcode_effective_settings_for_list(ctx, objects, config_list, default_name, configuration, path_maps)

def _xcode_effective_settings_for_list(ctx, objects, config_list, default_name, configuration, path_maps):
    configs = (config_list or {}).get("buildConfigurations") or []
    config_id = _xcode_choose_config(objects, configs, default_name, configuration)
    config = objects.get(config_id) or {}
    xcconfig_settings = _xcode_base_config_xcconfig(ctx, objects, config, path_maps)
    return _xcode_merge_settings(xcconfig_settings, config.get("buildSettings") or {})

# ---------------------------------------------------------------------------
# File references and build phases
# ---------------------------------------------------------------------------

def _xcode_file_ref_path(ctx, file_ref):
    # Fallback path for a PBXFileReference seen without its enclosing group
    # (e.g. SDK framework references reached through a frameworks build phase).
    # Files reached through the project's group tree are resolved with their
    # full package-relative path by `_xcode_group_file_paths` instead.
    path = file_ref.get("path") or file_ref.get("name") or ""
    return path

def _xcode_join(prefix, segment):
    # Join two path fragments into one package-relative path.
    if not segment:
        return _xcode_normalize_path(prefix)
    if not prefix:
        return _xcode_normalize_path(segment)
    if segment.startswith("/"):
        return _xcode_normalize_path(segment)
    if prefix.endswith("/"):
        return _xcode_normalize_path(prefix + segment)
    return _xcode_normalize_path(prefix + "/" + segment)

def _xcode_normalize_path(path):
    # PBX groups routinely use paths such as `Framework/../Sources/File.swift`.
    # The build graph accepts only workspace-contained normalized paths, so
    # collapse dot segments while retaining an attempted relative escape for
    # the graph validator to reject with its ordinary diagnostic.
    if not path:
        return path
    absolute = path.startswith("/")
    parts = []
    for part in path.split("/"):
        if not part or part == ".":
            continue
        if part == "..":
            if parts and parts[-1] != "..":
                parts.pop()
            elif not absolute:
                parts.append(part)
        else:
            parts.append(part)
    normalized = "/".join(parts)
    if absolute:
        return "/" + normalized
    return normalized

def _xcode_node_dir(prefix, project_dir, node):
    # Resolve a group or file node's own path, honoring its `sourceTree`.
    # A file node returns its full package-relative path; a group node returns
    # the directory its children are resolved against. `None` marks a node that
    # does not live on the source tree (built products, SDK, developer dir).
    source_tree = node.get("sourceTree") or "<group>"
    path = node.get("path") or ""
    if source_tree == "<absolute>":
        return path
    if source_tree == "SOURCE_ROOT" or source_tree == "SRCROOT":
        return _xcode_join(project_dir, path)
    if source_tree in ["BUILT_PRODUCTS_DIR", "SDKROOT", "DEVELOPER_DIR"]:
        return None
    # "<group>" (the common case) and unknown trees resolve relative to the
    # enclosing group.
    return _xcode_join(prefix, path)

def _xcode_group_file_paths(objects, root_project, project_dir):
    # Walk the project's PBXGroup tree from `mainGroup`, accumulating the full
    # package-relative path of every file reference and versioned model group.
    # Xcode stores these nodes with a leaf `path` that is relative to the
    # enclosing group, so the full path only exists as the concatenation of the
    # group chain. Group directories are recorded too, so a reference given as
    # an anchor group plus a relative path (the newer `.xcconfig` reference
    # form) can be resolved.
    files = {}
    groups = {}
    main_group = root_project.get("mainGroup")
    if main_group:
        _xcode_walk_group(objects, main_group, project_dir, project_dir, files, groups, {})
    return {"files": files, "groups": groups, "additive": _xcode_additive_membership(objects, groups)}

def _xcode_additive_membership(objects, group_dirs):
    # Precompute, once for the whole project, which synchronized groups add
    # files to which target through an exception set. Scanning every object per
    # target would be quadratic on large projects.
    additive = {}
    for group_id, group in objects.items():
        if group.get("isa") != "PBXFileSystemSynchronizedRootGroup":
            continue
        base = group_dirs.get(group_id)
        if base == None:
            continue
        for exception_id in group.get("exceptions") or []:
            exception = objects.get(exception_id) or {}
            if exception.get("isa") != "PBXFileSystemSynchronizedBuildFileExceptionSet":
                continue
            target_name = (objects.get(exception.get("target")) or {}).get("name") or ""
            files = exception.get("membershipExceptions") or []
            if not target_name or not files:
                continue
            entry = {"group_id": group_id, "base": base, "relatives": files}
            if target_name in additive:
                additive[target_name].append(entry)
            else:
                additive[target_name] = [entry]
    return additive

def _xcode_walk_group(objects, group_id, prefix, project_dir, files, groups, seen):
    if group_id == None or group_id in seen:
        return
    seen[group_id] = True
    group = objects.get(group_id) or {}
    base = _xcode_node_dir(prefix, project_dir, group)
    if base == None:
        return
    groups[group_id] = base
    if group.get("isa") == "XCVersionGroup":
        files[group_id] = base
    for child_id in group.get("children") or []:
        child = objects.get(child_id) or {}
        isa = child.get("isa") or ""
        if isa in ["PBXGroup", "PBXVariantGroup", "XCVersionGroup", "PBXFileSystemSynchronizedRootGroup"]:
            _xcode_walk_group(objects, child_id, base, project_dir, files, groups, seen)
        elif isa == "PBXFileReference":
            resolved = _xcode_node_dir(base, project_dir, child)
            if resolved:
                files[child_id] = resolved

def _xcode_glob_match(pattern, text):
    # fnmatch-style match where `*` matches any run of characters (including
    # `/`) and `?` matches a single character. Backs the
    # `EXCLUDED_SOURCE_FILE_NAMES` / `INCLUDED_SOURCE_FILE_NAMES` build settings.
    p = 0
    t = 0
    star = -1
    star_t = 0
    plen = len(pattern)
    tlen = len(text)
    for _ in range(plen + tlen + tlen + 2):
        if p >= plen and t >= tlen:
            return True
        if p < plen and t < tlen and (pattern[p] == text[t] or pattern[p] == "?"):
            p += 1
            t += 1
        elif p < plen and pattern[p] == "*":
            star = p
            star_t = t
            p += 1
        elif star >= 0:
            p = star + 1
            star_t += 1
            t = star_t
        else:
            return False
    return p >= plen and t >= tlen

def _xcode_matches_any(patterns, path, base):
    for pattern in patterns:
        if _xcode_glob_match(pattern, base) or _xcode_glob_match(pattern, path):
            return True
    return False

def _xcode_filter_excluded_files(files, settings):
    # Drop build files matched by `EXCLUDED_SOURCE_FILE_NAMES` unless
    # `INCLUDED_SOURCE_FILE_NAMES` matches them back in. Patterns match against
    # both the file's basename and its package-relative path, mirroring how
    # Xcode filters per-platform source variants out of a target.
    excluded = _xcode_setting_to_list(settings.get("EXCLUDED_SOURCE_FILE_NAMES"))
    if not excluded:
        return files
    included = _xcode_setting_to_list(settings.get("INCLUDED_SOURCE_FILE_NAMES"))
    out = []
    for path in files:
        base = _basename(path)
        if _xcode_matches_any(excluded, path, base) and not _xcode_matches_any(included, path, base):
            continue
        out.append(path)
    return out

def _xcode_filter_excluded_sources(sources, settings):
    return _xcode_filter_excluded_files(sources, settings)

_XCODE_SOURCE_EXTS = [".swift", ".m", ".mm", ".c", ".cc", ".cpp", ".cxx", ".c++", ".S"]
_XCODE_HEADER_EXTS = [".h", ".hh", ".hpp", ".ipp", ".hxx"]
_XCODE_RESOURCE_EXTS = [".storyboard", ".xib", ".strings", ".plist", ".json", ".xcdatamodeld", ".xcdatamodel", ".xcmappingmodel", ".entitlements"]

def _xcode_is_source(path):
    for ext in _XCODE_SOURCE_EXTS:
        if _ends_with(path, ext):
            return True
    return False

def _xcode_is_header(path):
    for ext in _XCODE_HEADER_EXTS:
        if _ends_with(path, ext):
            return True
    return False

def _xcode_is_asset_catalog(path):
    return _ends_with(path, ".xcassets") or _ends_with(path, ".icon")

def _xcode_is_intent_definition(path):
    return _ends_with(path, ".intentdefinition")

def _xcode_is_resource(path):
    for ext in _XCODE_RESOURCE_EXTS:
        if _ends_with(path, ext):
            return True
    return False

def _xcode_is_excluded_source_path(path):
    # Skip Xcode-derived and preview paths that live next to sources.
    lower = path.lower()
    for needle in ["/.build/", "/build/", "preview content", ".swiftpm/"]:
        if needle in lower:
            return True
    return False

def _xcode_is_dependency_tree_path(path):
    # A path inside a dependency, package-manager, or generated tree, rather than
    # a first-party manifest. Discovering local Swift packages ignores these so a
    # project's own build artifacts or vendored checkouts are not mistaken for
    # workspace-local packages and fed to `swift package dump-package`.
    lower = "/" + path.lower()
    for needle in ["/.once/", "/checkouts/", "/pods/", "/carthage/", "/node_modules/", "/vendor/", "/.git/", "/deriveddata/"]:
        if needle in lower:
            return True
    return False

def _xcode_asset_catalog_dir(path):
    # If a path is a `.xcassets` bundle or lives inside one, return the catalog
    # directory (up to and including `.xcassets`); otherwise the empty string.
    for marker in [".xcassets", ".icon"]:
        index = path.find(marker + "/")
        if index >= 0:
            return path[:index + len(marker)]
        if _ends_with(path, marker):
            return path
    return ""

def _xcode_synced_exceptions(objects, group, target_name, base):
    # Collect the package-relative paths a synchronized group excludes from the
    # target named `target_name`. A membership exception path is relative to the
    # group directory (a leading slash still means group-relative), so it is
    # joined onto the group base.
    excluded = {}
    for exception_id in group.get("exceptions") or []:
        exception = objects.get(exception_id) or {}
        if exception.get("isa") != "PBXFileSystemSynchronizedBuildFileExceptionSet":
            continue
        exception_target = objects.get(exception.get("target")) or {}
        if (exception_target.get("name") or "") != target_name:
            continue
        for relative in exception.get("membershipExceptions") or []:
            trimmed = relative[1:] if relative.startswith("/") else relative
            excluded[_xcode_join(base, trimmed)] = True
    return excluded

def _xcode_path_excluded(path, excluded):
    if path in excluded:
        return True
    # An exception may name a directory; exclude everything beneath it.
    for prefix in excluded:
        if path.startswith(prefix + "/"):
            return True
    return False

def _xcode_classify_synced_path(path, buckets):
    if _xcode_is_excluded_source_path(path):
        return
    # A synchronized glob returns files inside a `.xcassets`, not the catalog
    # directory itself, so recover the catalog from the path and skip contents.
    catalog = _xcode_asset_catalog_dir(path)
    if catalog:
        buckets["asset_catalogs"].append(catalog)
    elif _xcode_is_intent_definition(path):
        buckets["intent_definitions"].append(path)
    elif _xcode_is_source(path):
        buckets["sources"].append(path)
    elif _xcode_is_header(path):
        buckets["headers"].append(path)
    elif _xcode_is_asset_catalog(path):
        buckets["asset_catalogs"].append(path)
    elif _xcode_is_resource(path):
        buckets["resources"].append(path)

def _xcode_synced_group_files(ctx, objects, target, project_dir, path_maps):
    # Xcode 16+ file-system synchronized root groups: every file under the group
    # directory is a member of the owning target, minus the files a membership
    # exception set removes from that target. An exception set that names a
    # target which does not own the group instead adds those files to that
    # target, which is how a shared source directory is compiled into several
    # targets. Enumerate and classify both. The group directory comes from the
    # tree walk so a group nested under other groups resolves to its full path.
    group_dirs = path_maps["groups"]
    buckets = {"sources": [], "headers": [], "resources": [], "asset_catalogs": [], "intent_definitions": []}
    target_name = target.get("name") or ""
    owned = {}
    for group_id in target.get("fileSystemSynchronizedGroups") or []:
        owned[group_id] = True
        group = objects.get(group_id) or {}
        base = group_dirs.get(group_id)
        if base == None:
            continue
        excluded = _xcode_synced_exceptions(objects, group, target_name, base)
        for path in glob([base + "/**"]):
            if _xcode_path_excluded(path, excluded):
                continue
            _xcode_classify_synced_path(path, buckets)

    # Additive membership from synchronized groups owned by other targets,
    # looked up from the precomputed map keyed by target.
    for entry in path_maps["additive"].get(target_name) or []:
        if entry["group_id"] in owned:
            continue
        for relative in entry["relatives"]:
            trimmed = relative[1:] if relative.startswith("/") else relative
            candidate = _xcode_join(entry["base"], trimmed)
            if host_path_exists(_xcode_abs(candidate)):
                _xcode_classify_synced_path(candidate, buckets)

    return {
        "sources": _unique(buckets["sources"]),
        "headers": _unique(buckets["headers"]),
        "resources": _unique(buckets["resources"]),
        "asset_catalogs": _unique(buckets["asset_catalogs"]),
        "intent_definitions": _unique(buckets["intent_definitions"]),
    }

def _xcode_classic_phase_files(ctx, objects, target, file_paths):
    # Classic projects list PBXBuildFile entries per phase. Resolve each back
    # to its PBXFileReference's full package-relative path (through the group
    # tree), falling back to the leaf path when the reference is not rooted in
    # a group (for example an SDK framework).
    sources = []
    headers = []
    exported_headers = []
    resources = []
    structured_resources = []
    asset_catalogs = []
    intent_definitions = []
    frameworks = []
    source_flags = {}
    for phase_id in target.get("buildPhases") or []:
        phase = objects.get(phase_id) or {}
        isa = phase.get("isa") or ""
        for build_file_id in phase.get("files") or []:
            build_file = objects.get(build_file_id) or {}
            file_ref_id = build_file.get("fileRef")
            file_ref = objects.get(file_ref_id) or {}
            if isa == "PBXSourcesBuildPhase" and file_ref.get("isa") == "PBXVariantGroup":
                candidates = []
                for child_id in file_ref.get("children") or []:
                    child = objects.get(child_id) or {}
                    child_path = file_paths.get(child_id) or _xcode_file_ref_path(ctx, child)
                    if child_path and _xcode_is_intent_definition(child_path):
                        candidates.append({"name": child.get("name") or "", "path": child_path})
                selected = ""
                for candidate in candidates:
                    if candidate["name"] == "Base":
                        selected = candidate["path"]
                        break
                if not selected and candidates:
                    selected = candidates[0]["path"]
                if selected:
                    intent_definitions.append(selected)
                continue
            if isa == "PBXResourcesBuildPhase" and file_ref.get("isa") == "PBXVariantGroup":
                for child_id in file_ref.get("children") or []:
                    child = objects.get(child_id) or {}
                    child_path = file_paths.get(child_id) or _xcode_file_ref_path(ctx, child)
                    if not child_path:
                        continue
                    if _xcode_is_asset_catalog(child_path):
                        asset_catalogs.append(child_path)
                    else:
                        resources.append(child_path)
                continue
            path = file_paths.get(file_ref_id) or _xcode_file_ref_path(ctx, file_ref)
            if not path:
                continue
            if isa == "PBXSourcesBuildPhase":
                if file_ref.get("isa") == "XCVersionGroup" and path.endswith(".xcdatamodeld"):
                    resources.append(path)
                elif _xcode_is_intent_definition(path):
                    intent_definitions.append(path)
                elif _xcode_is_source(path):
                    sources.append(path)
                    compiler_flags = _xcode_setting_to_list((build_file.get("settings") or {}).get("COMPILER_FLAGS"))
                    if compiler_flags:
                        source_flags[path] = compiler_flags
                elif _xcode_is_header(path):
                    headers.append(path)
            elif isa == "PBXResourcesBuildPhase":
                if file_ref.get("sourceTree") == "BUILT_PRODUCTS_DIR":
                    continue
                if _xcode_is_asset_catalog(path):
                    asset_catalogs.append(path)
                else:
                    resources.append(path)
                    if (file_ref.get("lastKnownFileType") or "").startswith("folder"):
                        structured_resources.append(path)
            elif isa == "PBXHeadersBuildPhase":
                headers.append(path)
                attributes = _xcode_setting_to_list((build_file.get("settings") or {}).get("ATTRIBUTES"))
                if "Public" in attributes:
                    exported_headers.append(path)
            elif isa == "PBXFrameworksBuildPhase":
                # Only SDK / system frameworks (sourceTree = SDKROOT or
                # DEVELOPER_DIR) become sdk_frameworks. Framework products
                # from other targets (sourceTree = BUILT_PRODUCTS_DIR) are
                # wired through deps, not as SDK frameworks.
                source_tree = file_ref.get("sourceTree") or ""
                if source_tree not in ["SDKROOT", "DEVELOPER_DIR"]:
                    continue
                name = file_ref.get("name") or _basename(path)
                if _ends_with(name, ".framework"):
                    frameworks.append(name[:len(name) - len(".framework")])
    return {
        "sources": _unique(sources),
        "headers": _unique(headers),
        "exported_headers": _unique(exported_headers),
        "resources": _unique(resources),
        "structured_resources": _unique(structured_resources),
        "asset_catalogs": _unique(asset_catalogs),
        "intent_definitions": _unique(intent_definitions),
        "frameworks": _unique(frameworks),
        "source_flags": source_flags,
    }

def _xcode_target_files(ctx, objects, target, file_paths, project_dir, path_maps):
    classic = _xcode_classic_phase_files(ctx, objects, target, file_paths)
    synced = _xcode_synced_group_files(ctx, objects, target, project_dir, path_maps)
    project_header_dirs = []
    for path in file_paths.values():
        if _xcode_is_header(path) and _parent_dir(path):
            project_header_dirs.append(_parent_dir(path))
    return {
        "sources": _unique(classic["sources"] + synced["sources"]),
        "headers": _unique(classic["headers"] + synced["headers"]),
        "exported_headers": _unique(classic["exported_headers"] + synced["headers"]),
        "resources": _unique(classic["resources"] + synced["resources"]),
        "structured_resources": classic["structured_resources"],
        "asset_catalogs": _unique(classic["asset_catalogs"] + synced["asset_catalogs"]),
        "intent_definitions": _unique(classic["intent_definitions"] + synced["intent_definitions"]),
        "frameworks": classic["frameworks"],
        "source_flags": classic["source_flags"],
        "project_header_dirs": _unique(project_header_dirs),
    }

def _xcode_workspace_relative(path):
    root = _xcode_workspace_root()
    if root and path.startswith(root):
        return path[len(root):]
    return path

def _xcode_workspace_input_path(path):
    relative = _xcode_workspace_relative(path)
    if not relative or relative.startswith("/"):
        return ""
    return relative

def _xcode_target_input_path(ctx, path):
    path = _xcode_workspace_input_path(path)
    package = ctx["label"]["package"]
    prefix = package + "/" if package else ""
    if prefix and path.startswith(prefix):
        return path[len(prefix):]
    return path

def _xcode_lowered_attrs(ctx, attrs):
    for key in ["bridging_header", "prefix_header", "modulemap", "info_plist", "entitlements"]:
        if attrs.get(key):
            attrs[key] = _xcode_target_input_path(ctx, attrs[key])
    for key in [
        "exported_header_dirs",
        "exported_headers",
        "private_header_dirs",
        "modulemap_headers",
        "auxiliary_modulemaps",
        "asset_catalogs",
        "resources",
        "structured_resources",
    ]:
        if attrs.get(key):
            attrs[key] = [_xcode_target_input_path(ctx, path) for path in attrs[key]]
    if attrs.get("per_source_clang_flags"):
        attrs["per_source_clang_flags"] = {
            _xcode_target_input_path(ctx, path): flags
            for path, flags in attrs["per_source_clang_flags"].items()
        }
    return attrs

def _xcode_script_path(value, subs):
    path = _xcode_resolve_vars(value or "", subs)
    if not path or path.startswith("$("):
        return ""
    return _xcode_workspace_relative(path)

def _xcode_shell_phase_paths(phase, paths_key, file_lists_key, subs):
    paths = []
    for value in phase.get(paths_key) or []:
        path = _xcode_script_path(value, subs)
        if path:
            paths.append(path)
    for value in phase.get(file_lists_key) or []:
        file_list = _xcode_script_path(value, subs)
        if not file_list or not host_file_exists(_xcode_abs(file_list)):
            continue
        for line in host_file_read(_xcode_abs(file_list)).split("\n"):
            entry = line.strip()
            if not entry or entry.startswith("#"):
                continue
            path = _xcode_script_path(entry, subs)
            if path:
                paths.append(path)
    return _unique(paths)

def _xcode_xml_attribute(text, name):
    marker = name + '="'
    start = text.find(marker)
    if start < 0:
        return ""
    start += len(marker)
    end = text.find('"', start)
    return text[start:end] if end >= 0 else ""

def _xcode_plist_string(text, key):
    marker = "<key>" + key + "</key>"
    start = text.find(marker)
    if start < 0:
        return ""
    remainder = text[start + len(marker):]
    opening = remainder.find("<string>")
    if opening < 0:
        return ""
    opening += len("<string>")
    closing = remainder.find("</string>", opening)
    return remainder[opening:closing] if closing >= 0 else ""

def _xcode_current_datamodel_contents(model):
    contents = glob([model + "/**/contents"])
    if len(contents) <= 1:
        return contents
    marker = model + "/.xccurrentversion"
    if not host_file_exists(_xcode_abs(marker)):
        return []
    current = _xcode_plist_string(host_file_read(_xcode_abs(marker)), "_XCCurrentVersionName")
    selected = model + "/" + current + "/contents"
    return [selected] if current and selected in contents else []

def _xcode_datamodel_generated_outputs(contents, model_name, out_dir):
    model_header = contents.split("<entity")[0]
    language = _xcode_xml_attribute(model_header, "sourceLanguage")
    extension = ".swift" if language == "Swift" else ".m"
    names = []
    generated = []
    for entity in contents.split("<entity")[1:]:
        generation = _xcode_xml_attribute(entity, "codeGenerationType")
        name = _xcode_xml_attribute(entity, "name")
        if not name or not generation or name in names:
            continue
        names.append(name)
        if generation == "class":
            generated.append(name + "+CoreDataClass" + extension)
        if generation in ["class", "category"]:
            generated.append(name + "+CoreDataProperties" + extension)
    if not generated:
        return []
    generated.insert(0, model_name + "+CoreDataModel" + extension)
    if extension == ".m":
        with_headers = []
        for path in generated:
            base = path[:len(path) - len(extension)]
            with_headers.extend([base + ".h", path])
        generated = with_headers
    return [out_dir + "/" + path for path in generated]

def _xcode_datamodel_sources(ctx, resources, module_name, swift_version, target_name):
    # `momc --action generate` is the same explicit source-generation action
    # used by Bazel's Apple rules. Derive its declared Swift outputs from the
    # versioned model metadata so the subsequent compiler action can consume
    # them without scanning an undeclared output directory.
    actions = []
    sources = []
    target_dir = ".once/out/" + ((ctx["label"]["package"] + "/") if ctx["label"]["package"] else "") + _xcode_sanitized_target_name(target_name) + "/CoreData"
    for model in resources:
        if not model.endswith(".xcdatamodeld"):
            continue
        selected_contents = _xcode_current_datamodel_contents(model)
        if not selected_contents:
            continue
        out_dir = target_dir + "/" + _xcode_sanitized_target_name(_basename(model))
        model_name = _basename(model)[:len(_basename(model)) - len(".xcdatamodeld")]
        outputs = _xcode_datamodel_generated_outputs(host_file_read(_xcode_abs(selected_contents[0])), model_name, out_dir)
        if not outputs:
            continue
        args = ["--action", "generate", "--module", module_name]
        if swift_version:
            args.extend(["--swift-version", swift_version])
        args.extend([model, out_dir])
        actions.append(_json_encode({"name": "Generate Core Data classes", "tool": "momc", "args": args, "inputs": [model], "outputs": outputs, "cacheable": True}))
        sources.extend(outputs)
    return {"actions": actions, "sources": _unique(sources)}

def _xcode_intent_sources(ctx, definitions, target_name):
    actions = []
    sources = []
    if not definitions:
        return {"actions": actions, "sources": sources}
    intentbuilderc = host_command([host_which("xcrun"), "--find", "intentbuilderc"]).strip()
    target_dir = ".once/out/" + ((ctx["label"]["package"] + "/") if ctx["label"]["package"] else "") + _xcode_sanitized_target_name(target_name) + "/Intents"
    for definition in definitions:
        stem = _basename(definition)
        if stem.endswith(".intentdefinition"):
            stem = stem[:len(stem) - len(".intentdefinition")]
        out_dir = target_dir + "/" + _xcode_sanitized_target_name(stem)
        absolute_output = _xcode_abs(out_dir)
        args = ["generate", "-input", _xcode_abs(definition), "-output", absolute_output, "-language", "Swift"]
        outputs = []
        for line in host_command([intentbuilderc] + args + ["-dryRun"]).split("\n"):
            output = line.strip()
            if output and output.endswith(".swift"):
                outputs.append(out_dir + "/" + _basename(output))
        if not outputs:
            fail(ctx["label"]["id"] + ": intentbuilderc did not declare generated Swift outputs for " + definition)
        actions.append(_json_encode({"name": "Generate Intent classes", "tool": "intentbuilderc", "args": args, "inputs": [definition], "outputs": outputs, "cacheable": True}))
        sources.extend(outputs)
    return {"actions": actions, "sources": _unique(sources)}

def _xcode_shell_script_phases(ctx, objects, target, subs, project_dir, target_name):
    # Shell phases run in the native target's declared order. Dependency
    # analysis is safe to cache only when the project declares both sides of
    # the action contract and does not require the phase to run every time.
    actions = []
    pending_actions = []
    generated_sources = []
    resource_inputs = []
    structured_resource_inputs = []
    workspace_root_path = _xcode_workspace_root()
    project_bundle = _basename(_xcode_project_path(ctx))
    project_name = project_bundle[:-len(".xcodeproj")] if project_bundle.endswith(".xcodeproj") else project_bundle
    target_build_dir = ".once/out/" + ((ctx["label"]["package"] + "/") if ctx["label"]["package"] else "") + _xcode_sanitized_target_name(target_name)
    derived_dir = workspace_root_path + target_build_dir
    source_root = workspace_root()
    if project_dir:
        source_root = source_root + "/" + project_dir
    script_subs = dict(subs)
    if not script_subs.get("USER_LIBRARY_DIR"):
        for phase_id in target.get("buildPhases") or []:
            phase_text = str(objects.get(phase_id) or {})
            if "USER_LIBRARY_DIR" in phase_text:
                host_home = host_env("HOME")
                if host_home:
                    script_subs["USER_LIBRARY_DIR"] = host_home + "/Library"
                break
    script_subs["SRCROOT"] = source_root
    script_subs["PROJECT_DIR"] = source_root
    script_subs["DERIVED_FILE_DIR"] = derived_dir
    script_subs["TARGET_BUILD_DIR"] = derived_dir
    script_subs["BUILT_PRODUCTS_DIR"] = derived_dir
    script_subs["PROJECT_TEMP_DIR"] = derived_dir + "/Intermediates"
    script_subs["TARGET_TEMP_DIR"] = derived_dir + "/Intermediates"
    script_subs["CONFIGURATION_TEMP_DIR"] = derived_dir + "/Intermediates"
    for phase_id in target.get("buildPhases") or []:
        phase = objects.get(phase_id) or {}
        if phase.get("isa") != "PBXShellScriptBuildPhase":
            pending_actions = []
            continue
        script = phase.get("shellScript") or ""
        if not script:
            continue
        outputs = _xcode_shell_phase_paths(phase, "outputPaths", "outputFileListPaths", script_subs)
        source_outputs = [path for path in outputs if _xcode_is_source(path)]
        link_outputs = [path for path in outputs if path.endswith(".a") or path.endswith(".dylib") or path.endswith(".o")]
        script_inputs = _xcode_shell_phase_paths(phase, "inputPaths", "inputFileListPaths", script_subs)
        resource_folder = script_subs.get("UNLOCALIZED_RESOURCES_FOLDER_PATH") or ""
        resource_prefix = target_build_dir + "/" + resource_folder if resource_folder else ""
        if resource_prefix and ("cp " in script or "ditto " in script):
            for output in outputs:
                if output == resource_prefix or output.startswith(resource_prefix + "/"):
                    for input in script_inputs:
                        if _basename(input) == _basename(output) and input not in resource_inputs:
                            resource_inputs.append(input)
                            structured_resource_inputs.append(input)
        inputs = [path for path in script_inputs if not path.startswith("/")]
        env = {}
        for key, value in script_subs.items():
            resolved = _xcode_resolve_vars(value, script_subs)
            if not resolved.startswith("$(") and not resolved.startswith("${"):
                env[key] = resolved
        env.update({
            "SRCROOT": script_subs["SRCROOT"],
            "PROJECT_DIR": script_subs["PROJECT_DIR"],
            "DERIVED_FILE_DIR": derived_dir,
            "TARGET_BUILD_DIR": derived_dir,
            "BUILT_PRODUCTS_DIR": derived_dir,
            "TARGET_NAME": target_name,
            "PRODUCT_NAME": script_subs.get("PRODUCT_NAME") or target_name,
            "PROJECT": project_name,
            "PROJECT_NAME": project_name,
            "SOURCE_ROOT": script_subs["SRCROOT"],
            "CONFIGURATION": script_subs.get("CONFIGURATION") or "Debug",
            "ACTION": "build",
        })
        for key in ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "USER", "LOGNAME", "DEVELOPER_DIR"]:
            if not env.get(key):
                value = host_env(key)
                if value:
                    env[key] = value
        for index, path in enumerate(script_inputs):
            env["SCRIPT_INPUT_FILE_" + str(index)] = path if path.startswith("/") else workspace_root_path + path
        env["SCRIPT_INPUT_FILE_COUNT"] = str(len(script_inputs))
        env["SCRIPT_INPUT_FILE_LIST_COUNT"] = "0"
        for index, path in enumerate(outputs):
            env["SCRIPT_OUTPUT_FILE_" + str(index)] = path if path.startswith("/") else workspace_root_path + path
        env["SCRIPT_OUTPUT_FILE_COUNT"] = str(len(outputs))
        env["SCRIPT_OUTPUT_FILE_LIST_COUNT"] = "0"
        encoded = _json_encode({
            "name": phase.get("name") or "Run Script",
            "shell": phase.get("shellPath") or host_which("sh"),
            "script": script,
            "inputs": inputs,
            "outputs": outputs,
            "cwd": project_dir,
            "env": env,
            "cacheable": bool(inputs) and bool(outputs) and phase.get("alwaysOutOfDate") != "1" and phase.get("basedOnDependencyAnalysis") != "0",
        })
        if not source_outputs and not link_outputs:
            if not outputs:
                pending_actions.append(encoded)
            else:
                pending_actions = []
            continue
        if link_outputs:
            actions.extend(pending_actions)
        pending_actions = []
        actions.append(encoded)
        generated_sources.extend(source_outputs)
    return {
        "actions": actions,
        "sources": _unique(generated_sources),
        "resource_inputs": _unique(resource_inputs),
        "structured_resource_inputs": _unique(structured_resource_inputs),
    }

# ---------------------------------------------------------------------------
# Swift Package Manager dependencies
# ---------------------------------------------------------------------------

def _xcode_spm_identity(url):
    # The package identity Swift Package Manager derives from a remote URL is
    # the last path component with any `.git` suffix removed.
    identity = _basename(url)
    if _ends_with(identity, ".git"):
        identity = identity[:len(identity) - 4]
    return identity

def _xcode_spm_package_refs(objects):
    # Map every package reference id to its identity, URL, and version
    # requirement. Local package references are recorded with their path.
    refs = {}
    for object_id, value in objects.items():
        isa = value.get("isa") or ""
        if isa == "XCRemoteSwiftPackageReference":
            url = value.get("repositoryURL") or ""
            refs[object_id] = {
                "kind": "remote",
                "identity": _xcode_spm_identity(url),
                "url": url,
                "requirement": value.get("requirement") or {},
            }
        elif isa == "XCLocalSwiftPackageReference":
            path = value.get("relativePath") or ""
            refs[object_id] = {
                "kind": "local",
                "identity": _basename(path),
                "path": path,
                "requirement": {},
            }
    return refs

def _xcode_target_spm_products(objects, target, package_refs):
    # Resolve package products declared directly on the target and products
    # linked through its Frameworks phase. Xcode uses both forms: extensions
    # frequently carry an `XCSwiftPackageProductDependency` only as a
    # `PBXBuildFile.productRef`.
    products = []
    product_ids = list(target.get("packageProductDependencies") or [])
    for phase_id in target.get("buildPhases") or []:
        phase = objects.get(phase_id) or {}
        if phase.get("isa") != "PBXFrameworksBuildPhase":
            continue
        for build_file_id in phase.get("files") or []:
            build_file = objects.get(build_file_id) or {}
            product_id = build_file.get("productRef")
            if product_id and product_id not in product_ids:
                product_ids.append(product_id)
    for product_id in product_ids:
        product = objects.get(product_id) or {}
        if product.get("isa") != "XCSwiftPackageProductDependency":
            continue
        name = product.get("productName") or ""
        if not name:
            continue
        ref = package_refs.get(product.get("package")) or {}
        products.append({
            "name": name,
            "package_identity": ref.get("identity") or "",
        })
    return products

def _xcode_local_package_products(ctx, wanted):
    # Some products are consumed without a package reference in the project;
    # Xcode resolves them from `Package.swift` folders in the workspace. Discover
    # those local packages and map each wanted product to its Swift Package
    # Manager identity and directory by reading each manifest with `swift
    # package dump-package` (offline, no dependency resolution). A local
    # package identity comes from its path, not the display name in
    # `Package.swift`.
    if not wanted:
        return {}
    remaining = {}
    for name in wanted:
        remaining[name] = True
    xcrun = host_which("xcrun")
    swift = host_command([xcrun, "--find", "swift"]).strip()
    resolved = {}
    for manifest in glob(["**/Package.swift"]):
        if not remaining:
            break
        if _xcode_is_excluded_source_path(manifest) or _xcode_is_dependency_tree_path(manifest):
            continue
        package_dir = _parent_dir(manifest)
        # A `Package.swift` at the workspace root has no parent segment, so
        # resolve it against the workspace root rather than an empty path (an
        # empty `--package-path` makes `swift package` search from its own
        # working directory and fail). This is the common shape of an SPM
        # monorepo whose Xcode project consumes the root package's products.
        if package_dir:
            absolute = _xcode_abs(package_dir)
        else:
            root = _xcode_workspace_root()
            absolute = root if root else "."
        raw = host_command([swift, "package", "dump-package", "--package-path", absolute])
        info = json_decode(raw)
        package_identity = _basename(package_dir)
        platforms = {}
        for entry in info.get("platforms") or []:
            name = entry.get("platformName") or ""
            version = entry.get("version") or ""
            if name and version:
                platforms[name] = version
        for product in info.get("products") or []:
            product_name = product.get("name") or ""
            if product_name in remaining:
                resolved[product_name] = {"identity": package_identity, "path": absolute, "platforms": platforms}
                remaining.pop(product_name)
    return resolved

def _xcode_swift_package_info(ctx, package_dir, identity = ""):
    xcrun = host_which("xcrun")
    swift = host_command([xcrun, "--find", "swift"]).strip()
    absolute = _xcode_abs(package_dir) if package_dir else _xcode_workspace_root()
    info = json_decode(host_command([swift, "package", "dump-package", "--package-path", absolute]))
    return {
        "identity": identity or _basename(package_dir) or (info.get("name") or "Package"),
        "path": package_dir,
        "info": info,
    }

def _xcode_local_swift_package_infos(ctx, project_dir, package_refs):
    # Package metadata is analysis input only. Compilation remains entirely in
    # Once's Apple target kinds. Follow only local package references declared
    # by the project, rather than every Package.swift in the repository: a
    # nested developer tool is a separate workspace unless Xcode references it.
    infos = []
    seen = {}
    for ref in package_refs.values():
        if ref.get("kind") != "local":
            continue
        package_dir = _xcode_join(project_dir, ref.get("path") or "")
        if package_dir in seen:
            continue
        seen[package_dir] = True
        manifest = _xcode_join(package_dir, "Package.swift")
        if not host_file_exists(_xcode_abs(manifest)):
            continue
        info = _xcode_swift_package_info(ctx, package_dir)
        infos.append(info)
    return infos

def _xcode_workspace_resolved_path(workspace_path):
    bundle = _parent_dir(workspace_path) if _ends_with(workspace_path, "/contents.xcworkspacedata") else workspace_path
    return bundle + "/xcshareddata/swiftpm/Package.resolved"

def _xcode_workspace_resolved_candidates():
    root = _xcode_workspace_root()
    if not root:
        return []
    find = host_which("find")
    return sorted([
        _xcode_workspace_relative(path)
        for path in host_command([find, root, "-type", "f", "-path", "*.xcworkspace/xcshareddata/swiftpm/Package.resolved"]).split("\n")
        if path
    ])

def _xcode_resolved_pins_from_path(path):
    if not path:
        return {}
    absolute = _xcode_abs(path)
    if not host_file_exists(absolute):
        return {}
    pins = {}
    data = json_decode(host_file_read(absolute))
    for pin in data.get("pins") or (data.get("object") or {}).get("pins") or []:
        identity = (pin.get("identity") or pin.get("package") or "").lower()
        if identity:
            pins[identity] = pin
    return pins

def _xcode_pins_match_package_refs(pins, package_refs):
    if not package_refs:
        return True
    for ref in package_refs.values():
        identity = (ref.get("identity") or "").lower()
        if identity and pins.get(identity):
            return True
    return False

def _xcode_package_resolved_pins(ctx, entry_path, project_path, package_refs):
    bundle = project_path
    if _ends_with(bundle, "/project.pbxproj"):
        bundle = _parent_dir(bundle)
    candidates = []
    if _xcode_is_workspace(entry_path):
        candidates.append(_xcode_workspace_resolved_path(entry_path))
    candidates.extend([
        bundle + "/project.xcworkspace/xcshareddata/swiftpm/Package.resolved",
        bundle + "/project.workspace/xcshareddata/swiftpm/Package.resolved",
    ])
    candidates.extend(_xcode_workspace_resolved_candidates())
    seen = {}
    fallback = {}
    for candidate in candidates:
        if candidate in seen:
            continue
        seen[candidate] = True
        pins = _xcode_resolved_pins_from_path(candidate)
        if not pins:
            continue
        if _xcode_pins_match_package_refs(pins, package_refs):
            return pins
        if not fallback:
            fallback = pins
    return fallback

def _xcode_remote_swift_package_infos(ctx, entry_path, project_path, package_refs):
    # The lockfile is authoritative for source-control revisions. Git provides
    # source acquisition only; package manifests are then lowered to Once Apple
    # targets and compiled by Once rather than by Swift Package Manager.
    pins = _xcode_package_resolved_pins(ctx, entry_path, project_path, package_refs)
    git = host_which("git")
    root = _xcode_workspace_root() + ".once/xcode-packages"
    infos = []
    seen = {}
    refs_by_identity = {}
    for ref in package_refs.values():
        if ref.get("kind") == "remote" and ref.get("identity"):
            refs_by_identity[ref["identity"].lower()] = ref
    for key, pin in pins.items():
        if pin.get("kind") != "remoteSourceControl":
            continue
        ref = refs_by_identity.get(key) or {}
        identity = ref.get("identity") or pin.get("identity") or ""
        key = identity.lower()
        if not identity or key in seen:
            continue
        seen[key] = True
        info = _xcode_remote_swift_package_info(ctx, identity, pin, ref.get("url") or "")
        if info:
            infos.append(info)
    return infos

def _xcode_remote_swift_package_info(ctx, identity, pin, url = "", checkout_root = ".once/xcode-packages"):
    state = pin.get("state") or {}
    revision = state.get("revision") or ""
    if not revision:
        return None
    git = host_which("git")
    package_dir = checkout_root + "/" + _xcode_sanitized_target_name(identity)
    absolute = _xcode_abs(package_dir)
    manifest = absolute + "/Package.swift"
    if not host_file_exists(manifest):
        if not _xcode_host_directory_exists(absolute):
            host_command([host_which("mkdir"), "-p", _parent_dir(absolute)])
            host_command([git, "clone", "--no-checkout", url or pin.get("location") or "", absolute])
        host_command([git, "-C", absolute, "fetch", "--depth", "1", "origin", revision])
        host_command([git, "-C", absolute, "checkout", "--detach", revision])
    else:
        current = host_command([git, "-C", absolute, "rev-parse", "HEAD"]).strip()
        if current != revision:
            host_command([git, "-C", absolute, "fetch", "--depth", "1", "origin", revision])
            host_command([git, "-C", absolute, "checkout", "--detach", revision])
    info = _xcode_swift_package_info(ctx, package_dir, identity)
    return info

def _xcode_package_resolved_pins_at(package_path):
    if not package_path:
        return {}
    return _xcode_resolved_pins_from_path(package_path + "/Package.resolved")

def _xcode_expand_swift_package_infos(ctx, initial_infos):
    infos = list(initial_infos)
    known = {}
    for info in infos:
        known[(info.get("identity") or "").lower()] = True
    pending = list(infos)
    # Swift package dependency graphs are shallow in practice, but the bound
    # prevents a malformed lockfile from making analysis unbounded.
    for _ in range(32):
        discovered = []
        for package in pending:
            pins = _xcode_package_resolved_pins_at(package.get("path") or "")
            for identity, pin in pins.items():
                if pin.get("kind") != "remoteSourceControl" or identity in known:
                    continue
                info = _xcode_remote_swift_package_info(ctx, pin.get("identity") or identity, pin)
                if info:
                    discovered.append(info)
                    infos.append(info)
                    known[identity] = True
        if not discovered:
            break
        pending = discovered
    return infos

def _xcode_swift_package_target_id(identity, target_name, target_prefix = "SwiftPackage"):
    return target_prefix + "_" + _xcode_sanitized_target_name(identity) + "_" + _xcode_sanitized_target_name(target_name)

def _xcode_swift_package_host_target_id(identity, target_name, target_prefix = "SwiftPackage"):
    return _xcode_swift_package_target_id(identity, target_name, target_prefix) + "_MacroHost"

def _xcode_host_directory_exists(path):
    parent = _parent_dir(path)
    if not parent or not host_path_exists(parent):
        return False
    return bool(host_command([host_which("find"), parent, "-maxdepth", "1", "-type", "d", "-name", _basename(path)]).strip())

def _xcode_swift_package_target_path_is_excluded(root, target_path, excluded, path):
    target_root = _xcode_join(root, target_path)
    normalized_path = _xcode_normalize_path(path)
    for exclude in excluded:
        excluded_path = _xcode_join(target_root, exclude)
        if normalized_path == excluded_path or normalized_path.startswith(excluded_path + "/"):
            return True
    return False

def _xcode_swift_package_target_path(target):
    path = target.get("path") or ""
    if path:
        return path
    directory = "Tests" if (target.get("type") or "") == "test" else "Sources"
    return directory + "/" + (target.get("name") or "")

def _xcode_swift_package_target_sources(package_path, target):
    target_path = _xcode_swift_package_target_path(target)
    root = package_path + "/" if package_path else ""
    excluded = target.get("exclude") or []
    patterns = [root + target_path + "/**/*"]
    paths = glob(patterns)
    if package_path.startswith(".once/"):
        absolute = _xcode_abs(root + target_path)
        if host_path_exists(absolute):
            paths = [_xcode_workspace_relative(path) for path in host_command([host_which("find"), absolute, "-type", "f"]).split("\n") if path]
    result = []
    for path in paths:
        if not _xcode_swift_package_target_path_is_excluded(root, target_path, excluded, path) and not _xcode_is_documentation_path(path) and _xcode_is_source(path):
            result.append(path)
    return _unique(result)

def _xcode_is_documentation_path(path):
    # Swift package documentation catalogs frequently contain illustrative
    # `.swift` snippets. They are resources for DocC, never target sources.
    return ".docc/" in path.lower()

def _xcode_swift_package_target_headers(package_path, target):
    target_path = _xcode_swift_package_target_path(target)
    root = package_path + "/" if package_path else ""
    public_headers_path = _xcode_join(root + target_path, target.get("publicHeadersPath") or "include")
    excluded = target.get("exclude") or []
    if package_path.startswith(".once/"):
        absolute = _xcode_abs(root + target_path)
        if not host_path_exists(absolute):
            return []
        public_absolute = _xcode_abs(public_headers_path)
        if not _xcode_host_directory_exists(public_absolute):
            return []
        public_headers = [_xcode_workspace_relative(path) for path in host_command([host_which("find"), "-L", public_absolute, "-name", "*.h", "-type", "f"]).split("\n") if path]
        return [path for path in public_headers if not _xcode_swift_package_target_path_is_excluded(root, target_path, excluded, path)]
    return [path for path in glob([public_headers_path + "/**/*.h"]) if not _xcode_swift_package_target_path_is_excluded(root, target_path, excluded, path)]

def _xcode_swift_package_target_modulemap(package_path, target):
    target_path = _xcode_swift_package_target_path(target)
    public_headers_path = _xcode_join(package_path + "/" + target_path, target.get("publicHeadersPath") or "include")
    candidate = public_headers_path + "/module.modulemap"
    return candidate if host_file_exists(_xcode_abs(candidate)) else ""

def _xcode_swift_package_include_dirs(package_path, target):
    target_path = _xcode_swift_package_target_path(target)
    include_dir = _xcode_join(package_path + "/" + target_path, target.get("publicHeadersPath") or "include")
    if package_path.startswith(".once/"):
        include_absolute = _xcode_abs(include_dir)
        if _xcode_host_directory_exists(include_absolute) and host_command([host_which("find"), "-L", include_absolute, "-name", "*.h", "-type", "f"]).strip():
            return [include_dir]
        return []
    if glob([include_dir + "/**/*.h"]):
        return [include_dir]
    return []

def _xcode_swift_package_target_header_dirs(package_path, target):
    target_path = _xcode_swift_package_target_path(target)
    root = package_path + "/" if package_path else ""
    excluded = target.get("exclude") or []
    if package_path.startswith(".once/"):
        absolute = _xcode_abs(root + target_path)
        if not host_path_exists(absolute):
            return []
        headers = [_xcode_workspace_relative(path) for path in host_command([host_which("find"), absolute, "-name", "*.h", "-type", "f"]).split("\n") if path]
    else:
        headers = glob([root + target_path + "/**/*.h"])
    return _unique([_parent_dir(header) for header in headers if _parent_dir(header) and not _xcode_swift_package_target_path_is_excluded(root, target_path, excluded, header)])

def _xcode_swift_package_target_datamodels(package_path, target):
    target_path = _xcode_swift_package_target_path(target)
    root = package_path + "/" + target_path
    absolute = _xcode_abs(root)
    if not _xcode_host_directory_exists(absolute):
        return []
    return [_xcode_workspace_relative(path) for path in host_command([host_which("find"), absolute, "-type", "d", "-name", "*.xcdatamodeld"]).split("\n") if path]

def _xcode_swift_package_binary_artifact(ctx, package_path, target_id, target):
    name = target.get("name") or ""
    url = target.get("url") or ""
    checksum = target.get("checksum") or ""
    if name and url and checksum:
        artifact_target = target_id + "_Artifact"
        package = ctx["label"].get("package") or ""
        output = ".once/out/" + ((package + "/") if package else "") + artifact_target + "/archive"
        return {
            "target": artifact_target,
            "bundle": output + "/" + name + ".xcframework",
            "url": url,
            "sha256": checksum,
        }
    target_path = target.get("path") or ""
    if target_path.endswith(".xcframework"):
        candidate = package_path + "/" + target_path
        absolute = _xcode_abs(candidate)
        if _xcode_host_directory_exists(absolute):
            return {"bundle": candidate}
    root = package_path or "."
    absolute = _xcode_abs(root)
    if not _xcode_host_directory_exists(absolute):
        return None
    bundles = [_xcode_workspace_relative(path) for path in host_command([host_which("find"), absolute, "-type", "d", "-name", name + ".xcframework"]).split("\n") if path]
    if len(bundles) == 1:
        return {"bundle": bundles[0]}
    return None

def _xcode_include_flags(paths):
    flags = []
    for path in paths:
        flags.extend(["-I", path])
    return flags

def _xcode_swift_package_resource_bundle_name(package, target):
    package_name = _xcode_sanitized_target_name(package["info"].get("name") or package["identity"])
    target_name = _xcode_sanitized_target_name(target.get("name") or "Resources")
    return package_name + "_" + target_name + ".bundle"

def _xcode_swift_package_resource_paths(package_path, target):
    target_path = _xcode_swift_package_target_path(target)
    root = package_path + "/" if package_path else ""
    paths = []
    for resource in target.get("resources") or []:
        path = resource.get("path") or ""
        if path:
            paths.append(root + target_path + "/" + path)
    return _unique(paths)

def _xcode_swift_package_structured_resource_paths(package_path, target):
    target_path = _xcode_swift_package_target_path(target)
    root = package_path + "/" if package_path else ""
    paths = []
    for resource in target.get("resources") or []:
        path = resource.get("path") or ""
        if path and "copy" in (resource.get("rule") or {}):
            candidate = root + target_path + "/" + path
            if _xcode_host_directory_exists(_xcode_abs(candidate)):
                paths.append(candidate)
    return _unique(paths)

def _xcode_swift_package_resource_accessor(package, target, target_id, has_swift_sources):
    if not has_swift_sources or not (target.get("resources") or []):
        return None
    bundle_name = _xcode_swift_package_resource_bundle_name(package, target)
    output = ".once/out/" + target_id + "/resource_bundle_accessor.swift"
    contents = """import Foundation

private final class BundleFinder {}

extension Bundle {
    static let module: Bundle = {
        let paths = [
            Bundle.main.bundleURL.appendingPathComponent(""" + repr(bundle_name) + """),
            Bundle(for: BundleFinder.self).bundleURL.appendingPathComponent(""" + repr(bundle_name) + """),
        ]
        for path in paths {
            if let bundle = Bundle(url: path) {
                return bundle
            }
        }
        fatalError(""" + repr("unable to find resource bundle " + bundle_name) + """)
    }()
}
"""
    return _json_encode({
        "name": "swift-package-resource-accessor",
        "contents": contents,
        "outputs": [output],
    })

def _xcode_package_condition_allows(condition, platform):
    names = (condition or {}).get("platformNames") or []
    if not names:
        return True
    wanted = "macos" if platform == "macosx" else platform
    return wanted.lower() in [name.lower() for name in names]

def _xcode_swift_package_dependencies(target, identity, target_ids, product_ids, platform, lazy_products = {}, lazy_dependency = ""):
    deps = []
    for dependency in target.get("dependencies") or []:
        for key in ["byName", "product", "target"]:
            values = dependency.get(key) or []
            if not values:
                continue
            condition = {}
            for value in values[1:]:
                if type(value) == "dict" and value.get("platformNames"):
                    condition = value
            if not _xcode_package_condition_allows(condition, platform):
                continue
            package_identity = identity
            if key == "product" and len(values) > 1 and type(values[1]) == "string" and values[1]:
                package_identity = values[1]
            name = values[0]
            dependency_ids = target_ids.get(package_identity + "\x1f" + name) or target_ids.get(package_identity.lower() + "\x1f" + name) or product_ids.get(package_identity + "\x1f" + name) or product_ids.get(package_identity.lower() + "\x1f" + name) or product_ids.get(name)
            if dependency_ids and type(dependency_ids) != "list":
                dependency_ids = [dependency_ids]
            if dependency_ids:
                for dep_id in dependency_ids:
                    if dep_id and "./" + dep_id not in deps:
                        deps.append("./" + dep_id)
            elif lazy_dependency and lazy_products.get(package_identity.lower() + "\x1f" + name) and "./" + lazy_dependency not in deps:
                deps.append("./" + lazy_dependency)
    return deps

def _xcode_swift_imports(sources):
    modules = []
    for source in sources:
        if not source.endswith(".swift") or not host_file_exists(_xcode_abs(source)):
            continue
        for line in host_file_read(_xcode_abs(source)).split("\n"):
            words = [word for word in line.strip().replace("\t", " ").split(" ") if word]
            if "import" not in words:
                continue
            index = words.index("import")
            module_index = index + 1
            if module_index < len(words) and words[module_index] in ["typealias", "struct", "class", "enum", "protocol", "let", "var", "func"]:
                module_index += 1
            if module_index < len(words):
                module = words[module_index].split(".")[0]
                if module and module not in modules:
                    modules.append(module)
    return modules

def _xcode_swift_package_minimum_os(package, platform, fallback):
    wanted = "macos" if platform == "macosx" else platform
    for declaration in package["info"].get("platforms") or []:
        if (declaration.get("platformName") or "").lower() == wanted:
            declared = declaration.get("version") or fallback
            return declared if _xcode_version_key(declared) > _xcode_version_key(fallback) else fallback
    return fallback

def _xcode_swift_package_language_mode(package):
    versions = package["info"].get("swiftLanguageVersions") or []
    if versions:
        value = versions[-1]
        if type(value) == "dict":
            return value.get("_version") or value.get("version") or ""
        return value
    tools_version = ((package["info"].get("toolsVersion") or {}).get("_version") or "5").split(".")[0]
    return "6" if _xcode_digits(tools_version) >= 6 else "5"

def _xcode_swift_package_name_flags(package):
    tools_version = (package["info"].get("toolsVersion") or {}).get("_version") or "5.0"
    if _xcode_version_key(tools_version) < _xcode_version_key("5.9"):
        return []
    return ["-package-name", package["info"].get("name") or package["identity"]]

def _xcode_swift_package_target_flags(target, platform, default_language_mode):
    swift_flags = []
    clang_flags = []
    swift_language_mode = default_language_mode
    for setting in target.get("settings") or []:
        if not _xcode_package_condition_allows(setting.get("condition") or {}, platform):
            continue
        kind = setting.get("kind") or {}
        flags = (kind.get("unsafeFlags") or {}).get("_0") or []
        if setting.get("tool") == "swift":
            swift_flags.extend(flags)
            language_mode = (kind.get("swiftLanguageMode") or {}).get("_0") or ""
            if language_mode:
                swift_language_mode = language_mode
            definition = (kind.get("define") or {}).get("_0") or ""
            if definition:
                swift_flags.extend(["-D", definition])
            upcoming = (kind.get("enableUpcomingFeature") or {}).get("_0") or ""
            if upcoming:
                swift_flags.extend(["-enable-upcoming-feature", upcoming])
            experimental = (kind.get("enableExperimentalFeature") or {}).get("_0") or ""
            if experimental:
                swift_flags.extend(["-enable-experimental-feature", experimental])
        elif setting.get("tool") in ["c", "cxx"]:
            clang_flags.extend(flags)
            definition = (kind.get("define") or {}).get("_0") or ""
            if definition:
                clang_flags.append("-D" + definition)
    if target.get("resources") or []:
        clang_flags.append("-DSWIFTPM_MODULE_BUNDLE=[NSBundle mainBundle]")
    return {
        "swift": swift_flags,
        "clang": clang_flags,
        "language_mode": swift_language_mode,
    }

def _xcode_local_swift_package_specs(ctx, package_infos, platform, minimum_os, sdk_variant, lazy_products = {}, lazy_dependency = "", target_prefix = "SwiftPackage"):
    # Lower source package targets as ordinary Apple libraries. Products group
    # one or more targets, so consumers receive the complete product closure.
    target_ids = {}
    product_ids = {}
    host_target_ids = {}
    host_product_ids = {}
    module_ids = {}
    for package in package_infos:
        identity = package["identity"]
        for target in package["info"].get("targets") or []:
            name = target.get("name") or ""
            if name:
                target_id = _xcode_swift_package_target_id(identity, name, target_prefix)
                host_target_id = target_id if (target.get("type") or "") in ["binary", "macro", "test"] else _xcode_swift_package_host_target_id(identity, name, target_prefix)
                target_ids[identity + "\x1f" + name] = target_id
                target_ids[identity.lower() + "\x1f" + name] = target_id
                host_target_ids[identity + "\x1f" + name] = host_target_id
                host_target_ids[identity.lower() + "\x1f" + name] = host_target_id
                module_ids[name] = target_id
        for product in package["info"].get("products") or []:
            targets = product.get("targets") or []
            if targets:
                product_name = product.get("name") or ""
                product_target_ids = []
                host_product_target_ids = []
                for target_name in targets:
                    product_target_id = target_ids.get(identity + "\x1f" + target_name) or ""
                    if product_target_id and product_target_id not in product_target_ids:
                        product_target_ids.append(product_target_id)
                    host_product_target_id = host_target_ids.get(identity + "\x1f" + target_name) or ""
                    if host_product_target_id and host_product_target_id not in host_product_target_ids:
                        host_product_target_ids.append(host_product_target_id)
                product_id = product_target_ids[0] if len(product_target_ids) == 1 else product_target_ids
                host_product_id = host_product_target_ids[0] if len(host_product_target_ids) == 1 else host_product_target_ids
                product_ids[identity + "\x1f" + product_name] = product_id
                product_ids[identity.lower() + "\x1f" + product_name] = product_id
                product_ids[product_name] = product_id
                host_product_ids[identity + "\x1f" + product_name] = host_product_id
                host_product_ids[identity.lower() + "\x1f" + product_name] = host_product_id
                host_product_ids[product_name] = host_product_id

    specs = []
    for package in package_infos:
        identity = package["identity"]
        package_path = package["path"]
        package_minimum_os = _xcode_swift_package_minimum_os(package, platform, minimum_os)
        package_host_minimum_os = _xcode_swift_package_minimum_os(package, "macos", "13.0")
        for target in package["info"].get("targets") or []:
            target_type = target.get("type") or ""
            name = target.get("name") or ""
            if not name or target_type not in ["regular", "library", "test", "executable", "binary", "macro"]:
                continue
            target_id = target_ids[identity + "\x1f" + name]
            if target_type == "binary":
                artifact = _xcode_swift_package_binary_artifact(ctx, package_path, target_id, target)
                if artifact != None:
                    artifact_target = artifact.get("target") or ""
                    if artifact_target:
                        artifact_attrs = {
                            "url": artifact["url"],
                            "sha256": artifact["sha256"],
                        }
                        authorization_env = ctx["attr"].get("binary_artifact_authorization_env") or ""
                        if authorization_env:
                            artifact_attrs["authorization_env"] = authorization_env
                        specs.append({
                            "name": artifact_target,
                            "kind": "archive_download",
                            "deps": [],
                            "srcs": [],
                            "attrs": artifact_attrs,
                        })
                    specs.append({
                        "name": target_id,
                        "kind": "apple_xcframework_import",
                        "deps": ["./" + artifact_target] if artifact_target else [],
                        "srcs": [],
                        "attrs": {
                            "bundle": artifact["bundle"],
                            "platform": platform,
                            "sdk_variant": sdk_variant,
                        },
                    })
                continue
            sources = _xcode_swift_package_target_sources(package_path, target)
            if not sources:
                continue
            default_language_mode = _xcode_swift_package_language_mode(package)
            uses_swift_testing = any(["import Testing" in host_file_read(_xcode_abs(source)) for source in sources if source.endswith(".swift")])
            uses_xctest = any(["import XCTest" in host_file_read(_xcode_abs(source)) for source in sources if source.endswith(".swift")])
            if target_type == "macro":
                flags = _xcode_swift_package_target_flags(target, "macos", default_language_mode)
                dependencies = _xcode_swift_package_dependencies(target, identity, host_target_ids, host_product_ids, "macos", lazy_products, lazy_dependency)
                specs.append({
                    "name": target_id,
                    "kind": "swift_macro",
                    "deps": dependencies,
                    "srcs": sources,
                    "attrs": {
                        "minimum_os": package_host_minimum_os,
                        "module_name": name,
                        "swift_flags": ["-D", "SWIFT_PACKAGE"] + _xcode_swift_package_name_flags(package) + ["-swift-version", flags["language_mode"]] + flags["swift"],
                    },
                })
                continue
            variants = [{
                "id": target_id,
                "platform": platform,
                "minimum_os": package_minimum_os,
                "sdk_variant": sdk_variant,
                "target_ids": target_ids,
                "product_ids": product_ids,
            }]
            if target_type != "test":
                variants.append({
                    "id": host_target_ids[identity + "\x1f" + name],
                    "platform": "macos",
                    "minimum_os": package_host_minimum_os,
                    "sdk_variant": "simulator",
                    "target_ids": host_target_ids,
                    "product_ids": host_product_ids,
                })
            for variant in variants:
                variant_id = variant["id"]
                variant_platform = variant["platform"]
                flags = _xcode_swift_package_target_flags(target, variant_platform, default_language_mode)
                prebuild_actions = []
                core_data = _xcode_datamodel_sources(ctx, _xcode_swift_package_target_datamodels(package_path, target), name, "", variant_id)
                prebuild_actions.extend(core_data["actions"])
                resource_accessor = _xcode_swift_package_resource_accessor(package, target, variant_id, any([source.endswith(".swift") for source in sources]))
                if resource_accessor:
                    prebuild_actions.append(resource_accessor)
                dependencies = _xcode_swift_package_dependencies(target, identity, variant["target_ids"], variant["product_ids"], variant_platform, lazy_products, lazy_dependency)
                resource_paths = _xcode_swift_package_resource_paths(package_path, target)
                structured_resource_paths = _xcode_swift_package_structured_resource_paths(package_path, target)
                resource_bundle_name = _xcode_swift_package_resource_bundle_name(package, target) if resource_paths else ""
                attrs = {
                    "platform": variant_platform,
                    "minimum_os": variant["minimum_os"],
                    "sdk_variant": variant["sdk_variant"],
                    "defines": ["SWIFT_PACKAGE"],
                    "swift_flags": _xcode_swift_package_name_flags(package) + ["-swift-version", flags["language_mode"]] + flags["swift"],
                    "clang_flags": ["-std=c++17"] + flags["clang"],
                    "exported_header_dirs": _xcode_swift_package_include_dirs(package_path, target),
                    "private_header_dirs": _xcode_swift_package_target_header_dirs(package_path, target),
                    "prebuild_actions": prebuild_actions,
                    "resources": resource_paths,
                    "structured_resources": structured_resource_paths,
                    "swift_testing": uses_swift_testing,
                }
                spec_kind = "apple_library"
                if target_type == "test":
                    spec_kind = "apple_test_bundle"
                    attrs["product_name"] = name
                else:
                    attrs["module_name"] = name
                    attrs["exported_deps"] = dependencies
                    attrs["swift_flags"] = attrs["swift_flags"] + ["-enable-testing"]
                    attrs["exported_headers"] = _unique(_xcode_swift_package_target_headers(package_path, target))
                    attrs["modulemap"] = _xcode_swift_package_target_modulemap(package_path, target)
                    attrs["enable_modules"] = True
                    attrs["xctest_support"] = uses_xctest
                    attrs["resource_bundle_name"] = resource_bundle_name
                    attrs["resource_bundle_id"] = identity.lower() + "." + name + ".resources" if resource_bundle_name else ""
                specs.append({
                    "name": variant_id,
                    "kind": spec_kind,
                    "deps": dependencies,
                    "srcs": sources + core_data["sources"],
                    "attrs": attrs,
                })
    return {"specs": specs, "products": product_ids, "modules": module_ids}

def _xcode_version_key(version):
    parts = (version or "0").split(".")
    major = _xcode_digits(parts[0]) if len(parts) > 0 else 0
    minor = _xcode_digits(parts[1]) if len(parts) > 1 else 0
    return major * 1000 + minor

def _xcode_digits(text):
    out = 0
    for ch in (text or "").elems():
        digit = "0123456789".find(ch)
        if digit < 0:
            return out
        out = out * 10 + digit
    return out

def _xcode_spm_min_os(ctx, objects, native_targets, platform, configuration, project_settings, path_maps):
    key = {
        "ios": "IPHONEOS_DEPLOYMENT_TARGET",
        "macos": "MACOSX_DEPLOYMENT_TARGET",
        "tvos": "TVOS_DEPLOYMENT_TARGET",
        "watchos": "WATCHOS_DEPLOYMENT_TARGET",
        "visionos": "XROS_DEPLOYMENT_TARGET",
    }.get(platform)
    if not key:
        return "11.0"
    # A source package can be consumed by targets with different deployment
    # targets. Compile its shared module for the lowest target, which keeps it
    # importable by every consumer. This mirrors the compatibility direction of
    # Apple framework imports: a higher-minimum app can use a lower-minimum
    # module, whereas an extension targeting an earlier OS cannot import a
    # module compiled only for a later one.
    best = _xcode_scalar(project_settings.get(key))
    for target in native_targets:
        config_list = objects.get(target.get("buildConfigurationList")) or {}
        default_name = config_list.get("defaultConfigurationName") or "Release"
        settings = _xcode_effective_settings_for_list(ctx, objects, config_list, default_name, configuration, path_maps)
        value = _xcode_scalar(settings.get(key))
        if value and (not best or _xcode_version_key(value) < _xcode_version_key(best)):
            best = value
    return best or "11.0"

# ---------------------------------------------------------------------------
# Target graph
# ---------------------------------------------------------------------------

def _xcode_target_dependencies(objects, target):
    # Resolve PBXTargetDependency -> target name for native target dependencies.
    names = []
    for dep_id in target.get("dependencies") or []:
        dep = objects.get(dep_id) or {}
        proxy = objects.get(dep.get("targetProxy")) or {}
        remote = dep.get("target")
        if remote == None:
            remote = proxy.get("remoteGlobalIDString")
        remote_target = objects.get(remote) or {}
        if remote_target.get("isa") == "PBXNativeTarget":
            names.append(remote_target.get("name") or "")
        elif proxy.get("remoteInfo"):
            # The referenced target lives in another project of the workspace
            # (for example a CocoaPods library in `Pods.xcodeproj`). Its id is not
            # in this project's objects, but the proxy records its name, which
            # resolves against the workspace-wide name map.
            names.append(proxy.get("remoteInfo"))
    return [name for name in names if name]

def _xcode_product_dependency_key(name):
    value = name or ""
    for suffix in [".framework", ".a", ".dylib"]:
        if _ends_with(value, suffix):
            value = value[:len(value) - len(suffix)]
    return value.replace("_", "-").lower()

def _xcode_framework_product_dependencies(objects, target, name_to_id):
    # Package and pod integration commonly link a target only through a
    # BUILT_PRODUCTS_DIR framework reference. Recover the target edge from the
    # produced module name so compiler modules and archives flow to the native
    # consumer without relying on an integration-specific configuration file.
    available = {}
    for name in name_to_id.keys():
        available[_xcode_product_dependency_key(name)] = name
    names = []
    for phase_id in target.get("buildPhases") or []:
        phase = objects.get(phase_id) or {}
        if phase.get("isa") != "PBXFrameworksBuildPhase":
            continue
        for build_file_id in phase.get("files") or []:
            build_file = objects.get(build_file_id) or {}
            file_ref = objects.get(build_file.get("fileRef")) or {}
            if file_ref.get("sourceTree") != "BUILT_PRODUCTS_DIR":
                continue
            name = file_ref.get("name") or file_ref.get("path") or ""
            dependency = available.get(_xcode_product_dependency_key(name)) or ""
            if dependency and dependency not in names:
                names.append(dependency)
    return names

def _xcode_xcframework_dependencies(objects, target, file_paths, xcframework_names):
    # A prebuilt XCFramework has no PBXTargetDependency. Its ownership is
    # expressed by a build phase, including custom Copy Files phases used for
    # static XCFramework dependencies, so recover the graph edge from the
    # phase instead of guessing from the bundle path.
    dependencies = []
    for phase_id in target.get("buildPhases") or []:
        phase = objects.get(phase_id) or {}
        if phase.get("isa") not in ["PBXFrameworksBuildPhase", "PBXCopyFilesBuildPhase"]:
            continue
        for build_file_id in phase.get("files") or []:
            build_file = objects.get(build_file_id) or {}
            file_ref_id = build_file.get("fileRef")
            file_ref = objects.get(file_ref_id) or {}
            bundle = file_paths.get(file_ref_id) or _xcode_file_ref_path({}, file_ref)
            bundle = _xcode_workspace_relative(bundle)
            if not _ends_with(bundle, ".xcframework"):
                continue
            dependency = xcframework_names.get(bundle) or ""
            if dependency and dependency not in dependencies:
                dependencies.append(dependency)
    return dependencies

def _xcode_sanitized_target_name(name):
    out = []
    for ch in name.elems():
        if ch == "/" or ch == "\\" or ch == ":":
            out.append("_")
        elif ch == " ":
            out.append("_")
        else:
            out.append(ch)
    return "".join(out)

def _xcode_product_kind(product_type):
    # Application variants: plain apps, iMessage apps, App Clips
    # (on-demand-install-capable), and the iOS container that ships a watch app.
    if product_type in [
        "com.apple.product-type.application",
        "com.apple.product-type.application.messages",
        "com.apple.product-type.application.on-demand-install-capable",
        "com.apple.product-type.application.watchapp2-container",
    ]:
        return "application"
    if product_type == "com.apple.product-type.framework" or product_type == "com.apple.product-type.framework.static":
        return "framework"
    if product_type in ["com.apple.product-type.library.static", "com.apple.product-type.library.dynamic"]:
        return "library"
    if product_type in ["com.apple.product-type.bundle.unit-test", "com.apple.product-type.bundle.ui-testing"]:
        return "test"
    # App extensions (share, widget, notification-service, intents, sticker
    # packs, ExtensionKit, driver/system/PlugInKit/Spotlight/TV variants) and
    # embedded watchOS apps are executable bundles. They lower to
    # `apple_application` so their first-party sources compile and cache like any
    # other app module. The bundle wrapper an `.appex`/watch app needs (extension
    # point Info.plist, `.appex` suffix) is not modeled yet, so they build as app
    # bundles rather than embedded extension bundles.
    if _xcode_is_extension_product(product_type):
        return "extension"
    if product_type in ["com.apple.product-type.application.watchapp2", "com.apple.product-type.application.watchapp"]:
        return "watch_app"
    if product_type == "com.apple.product-type.bundle":
        return "bundle"
    if product_type == "com.apple.product-type.tool":
        return "tool"
    return ""

def _xcode_is_extension_product(product_type):
    if product_type in [
        "com.apple.product-type.app-extension",
        "com.apple.product-type.watchkit2-extension",
        "com.apple.product-type.watchkit-extension",
        "com.apple.product-type.extensionkit-extension",
        "com.apple.product-type.xpc-service",
        "com.apple.product-type.driver-extension",
        "com.apple.product-type.system-extension",
        "com.apple.product-type.pluginkit-plugin",
        "com.apple.product-type.spotlight-importer",
        "com.apple.product-type.xcode-extension",
        "com.apple.product-type.tv-app-extension",
        "com.apple.product-type.tv-broadcast-extension",
    ]:
        return True
    # Specialized extensions carry a suffix, e.g.
    # `com.apple.product-type.app-extension.messages-sticker-pack` and
    # `com.apple.product-type.app-extension.intents-service`.
    return product_type.startswith("com.apple.product-type.app-extension.")

def _xcode_effective_platform(settings, project_settings):
    # Prefer `SDKROOT`, then infer from whichever deployment-target setting or
    # `SUPPORTED_PLATFORMS` value is present, since projects that keep their
    # settings in `.xcconfig` files often leave `SDKROOT` unset in the project.
    sdkroot = _xcode_scalar(settings.get("SDKROOT")) or _xcode_scalar(project_settings.get("SDKROOT"))
    if sdkroot:
        return _xcode_platform(sdkroot)
    deployment_platforms = [
        ("MACOSX_DEPLOYMENT_TARGET", "macos"),
        ("TVOS_DEPLOYMENT_TARGET", "tvos"),
        ("WATCHOS_DEPLOYMENT_TARGET", "watchos"),
        ("XROS_DEPLOYMENT_TARGET", "visionos"),
        ("IPHONEOS_DEPLOYMENT_TARGET", "ios"),
    ]
    for key, name in deployment_platforms:
        if _xcode_scalar(settings.get(key)):
            return name
    supported = _xcode_scalar(settings.get("SUPPORTED_PLATFORMS")).lower()
    if "macosx" in supported:
        return "macos"
    if "appletv" in supported:
        return "tvos"
    if "watch" in supported:
        return "watchos"
    if "xr" in supported:
        return "visionos"
    if "iphone" in supported:
        return "ios"
    return "ios"

def _xcode_platform(sdkroot):
    sdk = (sdkroot or "").lower()
    if sdk in ["iphoneos", "iphonesimulator"]:
        return "ios"
    if sdk in ["macosx"]:
        return "macos"
    if sdk in ["appletvos", "appletvsimulator"]:
        return "tvos"
    if sdk in ["watchos", "watchsimulator"]:
        return "watchos"
    if sdk in ["xros", "xrsimulator"]:
        return "visionos"
    return "ios"

def _xcode_minimum_os(settings, platform):
    keys = {
        "ios": "IPHONEOS_DEPLOYMENT_TARGET",
        "macos": "MACOSX_DEPLOYMENT_TARGET",
        "tvos": "TVOS_DEPLOYMENT_TARGET",
        "watchos": "WATCHOS_DEPLOYMENT_TARGET",
        "visionos": "XROS_DEPLOYMENT_TARGET",
    }
    key = keys.get(platform)
    if not key:
        return ""
    return _xcode_scalar(settings.get(key))

def _xcode_scalar(value):
    if value == None:
        return ""
    if type(value) == type([]):
        return str(value[0]) if value else ""
    return str(value)

# ---------------------------------------------------------------------------
# Build settings -> Apple target attributes
# ---------------------------------------------------------------------------

def _xcode_clean_flags(flags):
    out = []
    for flag in flags:
        if flag == "$(inherited)" or flag.startswith("$("):
            continue
        if not flag:
            continue
        out.append(flag)
    return out

def _xcode_swift_defines(settings, subs = {}):
    out = []
    for condition in _xcode_setting_to_list(settings.get("SWIFT_ACTIVE_COMPILATION_CONDITIONS")):
        out.append(_xcode_resolve_vars(condition, subs))
    return _xcode_clean_flags(_unique(out))

def _xcode_clang_defines(settings, subs = {}):
    out = []
    for definition in _xcode_setting_to_list(settings.get("GCC_PREPROCESSOR_DEFINITIONS")):
        out.append(_xcode_resolve_vars(definition, subs))
    return _xcode_clean_flags(_unique(out))

def _xcode_swift_flags(settings, subs):
    raw = _xcode_setting_to_list(settings.get("OTHER_SWIFT_FLAGS"))
    out = []
    for flag in raw:
        resolved = _xcode_resolve_vars(flag, subs)
        out.append(resolved)
    return _xcode_clean_flags(out)

def _xcode_swift_feature_name(setting_suffix):
    overrides = {
        "DISABLE_OUTWARD_ACTOR_ISOLATION": "DisableOutwardActorInference",
        "IMPORT_OBJC_FORWARD_DECLS": "ImportObjcForwardDeclarations",
    }
    overridden = overrides.get(setting_suffix)
    if overridden:
        return overridden
    out = ""
    capitalize = True
    for character in setting_suffix.elems():
        if character == "_":
            capitalize = True
        elif capitalize:
            out += character.upper()
            capitalize = False
        else:
            out += character.lower()
    return out

def _xcode_swift_feature_flags(settings):
    flags = []
    families = [
        ("SWIFT_UPCOMING_FEATURE_", "-enable-upcoming-feature"),
        ("SWIFT_EXPERIMENTAL_FEATURE_", "-enable-experimental-feature"),
    ]
    for key in sorted(settings.keys()):
        for prefix, compiler_flag in families:
            if not key.startswith(prefix):
                continue
            mode = _xcode_scalar(settings.get(key)).upper()
            if mode not in ["YES", "MIGRATE"]:
                continue
            feature = _xcode_swift_feature_name(key[len(prefix):])
            if mode == "MIGRATE":
                feature += ":migrate"
            flags.extend([compiler_flag, feature])
    return flags

def _xcode_swift_language_flags(settings):
    # Translate the Swift language-mode and concurrency build settings into
    # compiler flags. Without these, code written for a specific Swift language
    # version or for main-actor-by-default isolation does not type-check.
    flags = []
    version = _xcode_scalar(settings.get("SWIFT_VERSION"))
    if version:
        major = version.split(".")[0]
        if version == "4.2":
            flags.extend(["-swift-version", "4.2"])
        elif major in ["4", "5", "6"]:
            flags.extend(["-swift-version", major])
    if _xcode_scalar(settings.get("SWIFT_DEFAULT_ACTOR_ISOLATION")) == "MainActor":
        flags.extend(["-default-isolation", "MainActor"])
    if _xcode_scalar(settings.get("SWIFT_APPROACHABLE_CONCURRENCY")).upper() == "YES":
        flags.extend(["-enable-upcoming-feature", "NonisolatedNonsendingByDefault"])
    if _xcode_scalar(settings.get("APPLICATION_EXTENSION_API_ONLY")).upper() == "YES":
        flags.append("-application-extension")
    flags.extend(_xcode_swift_feature_flags(settings))
    return flags

def _xcode_swift_compilation_flags(settings):
    flags = []
    optimization = _xcode_scalar(settings.get("SWIFT_OPTIMIZATION_LEVEL"))
    if optimization == "-Owholemodule":
        flags.append("-O")
    elif optimization in ["-Onone", "-O", "-Osize", "-Ounchecked"]:
        flags.append(optimization)
    whole_module = (
        optimization == "-Owholemodule" or
        _xcode_scalar(settings.get("SWIFT_COMPILATION_MODE")) == "wholemodule" or
        _xcode_scalar(settings.get("SWIFT_WHOLE_MODULE_OPTIMIZATION")).upper() == "YES"
    )
    if whole_module:
        flags.append("-whole-module-optimization")
    else:
        flags.append("-j1")
        if _xcode_scalar(settings.get("SWIFT_ENABLE_BATCH_MODE")).upper() == "NO":
            flags.append("-disable-batch-mode")
        else:
            flags.append("-enable-batch-mode")
    return flags

def _xcode_clang_flags(settings, subs):
    flags = []
    for key in ["OTHER_CFLAGS", "OTHER_CPLUSPLUSFLAGS"]:
        for flag in _xcode_setting_to_list(settings.get(key)):
            resolved = _xcode_resolve_vars(flag, subs)
            if resolved and resolved != "$(inherited)" and not resolved.startswith("$("):
                flags.append(resolved)
    for path in _xcode_header_search_dirs(settings, subs):
        flags.extend(["-I", path])
    if _xcode_scalar(settings.get("APPLICATION_EXTENSION_API_ONLY")).upper() == "YES":
        flags.append("-fapplication-extension")
    return flags

def _xcode_header_search_dirs(settings, subs):
    paths = []
    project_dir = subs.get("PROJECT_DIR") or ""
    for key in ["HEADER_SEARCH_PATHS", "USER_HEADER_SEARCH_PATHS"]:
        for path in _xcode_setting_to_list(settings.get(key)):
            resolved = _xcode_resolve_vars(path, subs).replace('"', "")
            if resolved and resolved != "$(inherited)" and not resolved.startswith("$("):
                if not resolved.startswith("/") and project_dir and resolved != project_dir and not resolved.startswith(project_dir + "/"):
                    resolved = _xcode_join(project_dir, resolved)
                paths.append(_xcode_normalize_path(resolved))
    return _unique(paths)

def _xcode_auxiliary_modulemaps(settings, subs, primary_modulemap):
    modulemaps = []
    primary_modulemap = _xcode_workspace_input_path(primary_modulemap)
    for header_dir in _xcode_header_search_dirs(settings, subs):
        header_dir = _xcode_workspace_input_path(header_dir)
        if not header_dir or "*" in header_dir:
            continue
        candidate = _xcode_join(header_dir, "module.modulemap")
        if candidate != primary_modulemap and host_file_exists(_xcode_abs(candidate)):
            modulemaps.append(candidate)
    return _unique(modulemaps)

def _xcode_modulemap_headers(modulemap, headers):
    if not modulemap or not host_file_exists(_xcode_abs(modulemap)):
        return []
    names = []
    for raw_line in host_file_read(_xcode_abs(modulemap)).split("\n"):
        line = raw_line.strip()
        for marker in ["umbrella header", "private textual header", "textual header", "header"]:
            if not line.startswith(marker + " "):
                continue
            opening = line.find('"')
            closing = line.find('"', opening + 1) if opening >= 0 else -1
            if closing > opening:
                names.append(line[opening + 1:closing])
            break
    resolved = []
    for name in names:
        relative_matches = [header for header in headers if header == name or header.endswith("/" + name)]
        matches = relative_matches
        if not matches:
            basename_matches = [header for header in headers if _basename(header) == _basename(name)]
            if len(basename_matches) == 1:
                matches = basename_matches
        for header in matches:
            if header not in resolved:
                resolved.append(header)
    return resolved

def _xcode_linkopts(settings, subs):
    resolved_flags = []
    for flag in _xcode_setting_to_list(settings.get("OTHER_LDFLAGS")):
        resolved = _xcode_resolve_vars(flag, subs)
        if resolved and resolved != "$(inherited)" and not resolved.startswith("$("):
            resolved_flags.append(resolved)
    flags = []
    skip_remaining = 0
    for index in range(len(resolved_flags)):
        if skip_remaining > 0:
            skip_remaining -= 1
            continue
        resolved = resolved_flags[index]
        if resolved == "-Xlinker" and index + 1 < len(resolved_flags):
            flags.extend([resolved, resolved_flags[index + 1]])
            skip_remaining = 1
            continue
        if resolved == "-fprofile-instr-generate":
            # Xcode applies this Clang spelling to the final Swift driver
            # link as well. Swift exposes the equivalent instrumentation
            # through its own driver option.
            flags.append("-profile-generate")
            continue
        arity = _XCODE_LINKER_OPTION_ARITY.get(resolved)
        if arity != None:
            flags.extend(["-Xlinker", resolved])
            for offset in range(arity):
                argument_index = index + offset + 1
                if argument_index < len(resolved_flags):
                    flags.extend(["-Xlinker", resolved_flags[argument_index]])
            skip_remaining = arity
            continue
        if resolved.startswith("-Wl,"):
            for option in resolved[len("-Wl,"):].split(","):
                if option:
                    flags.extend(["-Xlinker", option])
            continue
        if resolved.startswith("-") and not resolved.startswith("-l") and not resolved.startswith("-L") and not resolved.startswith("-F") and resolved != "-framework":
            flags.extend(["-Xlinker", resolved])
            continue
        flags.append(resolved)
    return flags

def _xcode_sdk_frameworks(settings, file_frameworks):
    frameworks = list(file_frameworks)
    return _unique([fw for fw in frameworks if fw])

def _xcode_families(settings):
    raw = _xcode_scalar(settings.get("TARGETED_DEVICE_FAMILY"))
    if not raw:
        return []
    families = []
    for code in raw.split(","):
        code = code.strip()
        if code == "1":
            families.append("iphone")
        elif code == "2":
            families.append("ipad")
    return families

# ---------------------------------------------------------------------------
# Target kind lowering
# ---------------------------------------------------------------------------

def _xcode_common_attrs(ctx, target, settings, subs, platform, files):
    attrs = {"platform": platform}
    minimum_os = _xcode_minimum_os(settings, platform)
    if minimum_os:
        attrs["minimum_os"] = minimum_os
    sdk_variant = ctx["attr"].get("sdk_variant") or "simulator"
    if platform != "macos":
        attrs["sdk_variant"] = sdk_variant
    swift_defines = _xcode_swift_defines(settings, subs)
    if swift_defines:
        attrs["swift_defines"] = swift_defines
    clang_defines = _xcode_clang_defines(settings, subs)
    if clang_defines:
        attrs["clang_defines"] = clang_defines
    swift_flags = _xcode_swift_flags(settings, subs) + _xcode_swift_language_flags(settings) + _xcode_swift_compilation_flags(settings)
    if swift_flags:
        attrs["swift_flags"] = swift_flags
    clang_flags = _xcode_clang_flags(settings, subs)
    if clang_flags:
        attrs["clang_flags"] = clang_flags
    if files["source_flags"]:
        attrs["per_source_clang_flags"] = {path: _json_encode(flags) for path, flags in files["source_flags"].items()}
    linkopts = _xcode_linkopts(settings, subs)
    if linkopts:
        attrs["linkopts"] = linkopts
    bridging_header = _xcode_workspace_input_path(_xcode_resolve_vars(_xcode_scalar(settings.get("SWIFT_OBJC_BRIDGING_HEADER")), subs))
    if bridging_header and not bridging_header.startswith("$("):
        attrs["bridging_header"] = bridging_header
        bridging_header_dirs = []
        for header_dir in files["project_header_dirs"]:
            if host_path_exists(_xcode_abs(header_dir)):
                bridging_header_dirs.append(header_dir)
        for header_dir in _xcode_header_search_dirs(settings, subs):
            header_dir = _xcode_workspace_input_path(header_dir)
            if header_dir and "*" not in header_dir and host_path_exists(_xcode_abs(header_dir)) and header_dir not in bridging_header_dirs:
                bridging_header_dirs.append(header_dir)
        if bridging_header_dirs:
            attrs["exported_header_dirs"] = bridging_header_dirs
    prefix_header = _xcode_workspace_input_path(_xcode_resolve_vars(_xcode_scalar(settings.get("GCC_PREFIX_HEADER")), subs))
    if prefix_header and not prefix_header.startswith("$("):
        project_dir = subs.get("PROJECT_DIR") or ""
        if not prefix_header.startswith("/") and project_dir:
            candidate = _xcode_join(project_dir, prefix_header)
            if host_file_exists(_xcode_abs(candidate)):
                prefix_header = candidate
        attrs["prefix_header"] = prefix_header
    exported_headers = files.get("exported_headers") or []
    if exported_headers:
        attrs["exported_headers"] = exported_headers
    private_header_dirs = []
    for header_dir in [_parent_dir(path) for path in files["sources"] + files["headers"] if _parent_dir(path)] + files["project_header_dirs"] + _xcode_header_search_dirs(settings, subs):
        header_dir = _xcode_workspace_input_path(header_dir)
        if header_dir and "*" not in header_dir and header_dir not in private_header_dirs:
            private_header_dirs.append(header_dir)
    if private_header_dirs:
        attrs["private_header_dirs"] = private_header_dirs
    sdk_frameworks = _xcode_sdk_frameworks(settings, files["frameworks"])
    if sdk_frameworks:
        attrs["sdk_frameworks"] = sdk_frameworks
    developer_dir = ctx["attr"].get("xcode_developer_dir")
    if developer_dir:
        attrs["xcode_developer_dir"] = developer_dir
    return attrs

def _xcode_library_attrs(ctx, target, settings, subs, platform, files):
    attrs = _xcode_common_attrs(ctx, target, settings, subs, platform, files)
    # Xcode frameworks expose their public headers as Clang modules. This also
    # permits qualified imports such as `Framework.Header` from Swift and
    # Objective-C sources without requiring project-specific source rewrites.
    if files["headers"] or _xcode_scalar(settings.get("CLANG_ENABLE_MODULES")).upper() == "YES":
        attrs["enable_modules"] = True
    modulemap = _xcode_workspace_input_path(_xcode_resolve_vars(_xcode_scalar(settings.get("MODULEMAP_FILE")), subs))
    if modulemap and not modulemap.startswith("$("):
        attrs["modulemap"] = modulemap
        modulemap_headers = _xcode_modulemap_headers(modulemap, files["headers"])
        if modulemap_headers:
            attrs["modulemap_headers"] = modulemap_headers
    auxiliary_modulemaps = _xcode_auxiliary_modulemaps(settings, subs, modulemap)
    if auxiliary_modulemaps:
        attrs["auxiliary_modulemaps"] = auxiliary_modulemaps
    product_name = _xcode_product_name(settings, target, subs)
    if product_name and product_name != _xcode_sanitized_target_name(target["name"]):
        attrs["module_name"] = product_name
    attrs["emit_dsym"] = True
    if _xcode_scalar(settings.get("ENABLE_TESTABILITY")).upper() == "YES":
        attrs["enable_testing"] = True
    return attrs, product_name

def _xcode_app_icon_exists(catalogs, name):
    # An app icon set lives in an asset catalog as `<name>.appiconset` (or a
    # sticker/icon-stack variant) containing a `Contents.json`.
    for catalog in catalogs:
        if _basename(catalog) == name + ".icon":
            return True
        for suffix in [".appiconset", ".stickersiconset", ".icon", ".solidimagestacklayer"]:
            if glob([catalog + "/**/" + name + suffix + "/Contents.json"]):
                return True
    return False

def _xcode_bundle_id(settings, subs, product_name):
    # The bundle identifier is frequently set through an `.xcconfig` or a
    # variable that does not resolve here. Fall back to a stable synthesized
    # identifier so bundle creation still succeeds.
    bundle_id = _xcode_resolve_vars(_xcode_scalar(settings.get("PRODUCT_BUNDLE_IDENTIFIER")), subs)
    if bundle_id and not bundle_id.startswith("$("):
        return bundle_id
    return "dev.once." + _xcode_bundle_id_component(product_name or "app")

def _xcode_bundle_id_component(name):
    out = []
    for ch in name.elems():
        allowed = (ch >= "a" and ch <= "z") or (ch >= "A" and ch <= "Z") or (ch >= "0" and ch <= "9") or ch == "-" or ch == "."
        out.append(ch if allowed else "-")
    return "".join(out) or "app"

def _xcode_add_info_plist_attrs(attrs, settings, subs, product_name, bundle_id):
    info_plist = _xcode_info_plist_file(settings, subs)
    project_dir = subs.get("PROJECT_DIR") or ""
    if info_plist and not info_plist.startswith("/") and project_dir:
        candidate = _xcode_join(project_dir, info_plist)
        if host_file_exists(_xcode_abs(candidate)):
            info_plist = candidate
    if not info_plist:
        return
    attrs["info_plist"] = info_plist
    contents = host_file_read(_xcode_abs(info_plist))
    plist_subs = dict(subs)
    project_root = workspace_root()
    if project_dir:
        project_root = project_root + "/" + project_dir
    plist_subs["SRCROOT"] = project_root
    plist_subs["PROJECT_DIR"] = project_root
    plist_subs["EXECUTABLE_NAME"] = product_name
    plist_subs["PRODUCT_NAME"] = product_name
    plist_subs["PRODUCT_BUNDLE_IDENTIFIER"] = bundle_id
    substitutions = {}
    for name in _xcode_variable_names(contents):
        value = _xcode_resolve_vars(str(plist_subs.get(name) or ""), plist_subs)
        if value and "$(" not in value and "${" not in value:
            substitutions[name] = value
    if substitutions:
        attrs["info_plist_substitutions"] = substitutions

def _xcode_application_attrs(ctx, target, settings, subs, platform, files):
    attrs = _xcode_common_attrs(ctx, target, settings, subs, platform, files)
    product_name = _xcode_product_name(settings, target, subs)
    if product_name:
        attrs["product_name"] = product_name
    bundle_id = _xcode_bundle_id(settings, subs, product_name)
    attrs["bundle_id"] = bundle_id
    _xcode_add_info_plist_attrs(attrs, settings, subs, product_name, bundle_id)
    development_team = _xcode_resolve_vars(_xcode_scalar(settings.get("DEVELOPMENT_TEAM")), subs)
    if development_team and not development_team.startswith("$("):
        attrs["development_team"] = development_team
    families = _xcode_families(settings)
    if families:
        attrs["families"] = families
    if _xcode_scalar(settings.get("ENABLE_TESTABILITY")).upper() == "YES":
        attrs["enable_testing"] = True
    entitlements = _xcode_resolve_vars(_xcode_scalar(settings.get("CODE_SIGN_ENTITLEMENTS")), subs)
    if entitlements and not entitlements.startswith("$("):
        if not entitlements.startswith("/") and (subs.get("PROJECT_DIR") or ""):
            entitlements = _xcode_join(subs["PROJECT_DIR"], entitlements)
        attrs["entitlements"] = entitlements
        contents = host_file_read(_xcode_abs(entitlements))
        substitutions = {}
        for name in _xcode_variable_names(contents):
            substitutions[name] = _xcode_resolve_vars(str(subs.get(name) or ""), subs)
        if substitutions:
            attrs["entitlements_substitutions"] = substitutions
    if files["asset_catalogs"]:
        attrs["asset_catalogs"] = files["asset_catalogs"]
        app_icon = _xcode_scalar(settings.get("ASSETCATALOG_COMPILER_APPICON_NAME"))
        # Only ask `actool` to compile the app icon when a matching icon set is
        # actually present; otherwise `actool` treats the missing icon as a hard
        # error and fails the build.
        if app_icon and _xcode_app_icon_exists(files["asset_catalogs"], app_icon):
            attrs["app_icon"] = app_icon
    if files["resources"]:
        attrs["resources"] = files["resources"]
    if files["structured_resources"]:
        attrs["structured_resources"] = files["structured_resources"]
    return attrs, product_name

def _xcode_test_attrs(ctx, target, settings, subs, platform, files):
    attrs = _xcode_common_attrs(ctx, target, settings, subs, platform, files)
    product_name = _xcode_product_name(settings, target, subs)
    if product_name:
        attrs["product_name"] = product_name
    bundle_id = _xcode_bundle_id(settings, subs, product_name)
    attrs["bundle_id"] = bundle_id
    test_style = _xcode_test_style(ctx, files["sources"])
    if test_style == "swift_testing":
        attrs["swift_testing"] = True
    if files["resources"]:
        attrs["resources"] = files["resources"]
    if files["structured_resources"]:
        attrs["structured_resources"] = files["structured_resources"]
    if files["asset_catalogs"]:
        attrs["asset_catalogs"] = files["asset_catalogs"]
    _xcode_add_info_plist_attrs(attrs, settings, subs, product_name, bundle_id)
    return attrs, product_name

def _xcode_product_name(settings, target, subs):
    raw = _xcode_scalar(settings.get("PRODUCT_NAME"))
    if not raw:
        raw = "$(TARGET_NAME)"
    resolved = _xcode_resolve_vars(raw, subs)
    if not resolved or resolved.startswith("$("):
        return _xcode_sanitized_target_name(target["name"])
    return resolved

def _xcode_test_style(ctx, source_paths):
    for path in source_paths:
        if not host_file_exists(_xcode_abs(path)):
            continue
        if "import Testing" in host_file_read(_xcode_abs(path)):
            return "swift_testing"
    return "xctest"

def _xcode_info_plist_file(settings, subs):
    generate = _xcode_scalar(settings.get("GENERATE_INFOPLIST_FILE")).upper()
    if generate == "YES":
        return ""
    raw = _xcode_scalar(settings.get("INFOPLIST_FILE"))
    if not raw:
        return ""
    resolved = _xcode_resolve_vars(raw, subs)
    if resolved.startswith("$("):
        return ""
    return resolved

def _xcode_referenced_test_plan_paths(ctx):
    paths = []
    entry_path = _xcode_project_path(ctx)
    scheme_roots = [entry_path + "/xcshareddata/xcschemes"]
    if _xcode_is_workspace(entry_path):
        for project_path in _xcode_workspace_projects(ctx, entry_path):
            scheme_roots.append(project_path + "/xcshareddata/xcschemes")
    find = host_which("find")
    for root in scheme_roots:
        absolute_root = _xcode_abs(root)
        if not host_path_exists(absolute_root):
            continue
        schemes = host_command([find, "-L", absolute_root, "-type", "f", "-name", "*.xcscheme"]).split("\n")
        for absolute_scheme in schemes:
            if not absolute_scheme:
                continue
            contents = host_file_read(absolute_scheme)
            for block in contents.split("<TestPlanReference")[1:]:
                reference = _xcode_xml_attribute(block.replace(" = ", "="), "reference")
                if reference.startswith("container:"):
                    reference = reference[len("container:"):]
                if reference and host_file_exists(_xcode_abs(reference)) and reference not in paths:
                    paths.append(reference)
    return paths

def _xcode_test_plan_settings(ctx):
    settings = {}
    for path in _xcode_referenced_test_plan_paths(ctx):
        plan = json_decode(host_file_read(_xcode_abs(path)))
        default_options = plan.get("defaultOptions") or {}
        options = dict(default_options)
        configured_options = {}
        configurations = plan.get("configurations") or []
        if configurations:
            configured_options = configurations[0].get("options") or {}
            options.update(configured_options)
        environment = {}
        environment_entries = (default_options.get("environmentVariableEntries") or []) + (configured_options.get("environmentVariableEntries") or [])
        for entry in environment_entries:
            if entry.get("enabled") == False:
                continue
            key = entry.get("key") or ""
            if key:
                environment[key] = str(entry.get("value") or "")
        language = options.get("language") or ""
        region = options.get("region") or ""
        if language:
            environment["AppleLanguages"] = "(" + language + ")"
        if language and region:
            environment["AppleLocale"] = language + "_" + region
        arguments = []
        argument_entries = (default_options.get("commandLineArgumentEntries") or []) + (configured_options.get("commandLineArgumentEntries") or [])
        for entry in argument_entries:
            if entry.get("enabled") == False:
                continue
            argument = entry.get("argument") or ""
            if argument:
                arguments.append(argument)
        for test_target in plan.get("testTargets") or []:
            name = ((test_target.get("target") or {}).get("name") or "")
            if not name:
                continue
            current = settings.get(name) or {"test_env": {}, "test_arguments": [], "skipped_tests": []}
            current["test_env"].update(environment)
            current["test_arguments"] = _unique(current["test_arguments"] + arguments)
            current["skipped_tests"] = _unique(current["skipped_tests"] + (test_target.get("skippedTests") or []))
            settings[name] = current
    return settings

def _xcode_test_host_ref(objects, settings, name_map):
    # A unit-test target embeds its host through TEST_HOST; resolve the host
    # application target name and wire it as the Once test_host dependency.
    # TEST_HOST values look like
    #   $(BUILT_PRODUCTS_DIR)/App.app/$(BUNDLE_EXECUTABLE_FOLDER_PATH)/App
    # so the host is identified by the `/<name>.app` fragment.
    raw = _xcode_scalar(settings.get("TEST_HOST"))
    if not raw:
        return ""
    for name, target_name in name_map.items():
        marker = "/" + name + ".app"
        if marker in raw:
            return target_name
    return ""

# ---------------------------------------------------------------------------
# Resolver
# ---------------------------------------------------------------------------

def _xcode_workspace_resolver(ctx):
    # `plutil` may be present off macOS, but only Xcode's `xcrun` supplies the
    # SDK and compiler information this resolver needs. Probe it first so the
    # generic resolver machinery can surface a structured unavailable-tool
    # diagnostic instead of executing an incompatible `plutil`.
    host_which("xcrun")
    entry_path = _xcode_project_path(ctx)
    configuration = ctx["attr"].get("configuration") or "Debug"

    # A `.xcworkspace` groups several `.xcodeproj` files: an application project
    # plus, for example, the `Pods.xcodeproj` CocoaPods generates. Resolve every
    # referenced project and merge their native targets into one graph so a
    # dependency that crosses project boundaries (an app target that links a pod
    # library) is wired. A bare `.xcodeproj` resolves as a single-project
    # workspace.
    # A workspace can reference a project that is not materialized on disk (one a
    # generator produces, or a `Pods.xcodeproj` before `pod install`). Skip those
    # rather than failing the whole workspace, since the projects Once can read
    # still resolve. A directly configured single project is always attempted so
    # a genuine typo surfaces a clear error instead of an empty graph.
    if _xcode_is_workspace(entry_path):
        project_paths = _xcode_workspace_projects(ctx, entry_path)
        skip_missing = True
    else:
        project_paths = [entry_path]
        skip_missing = False

    # Pass one: read every project and build a workspace-wide target name map so
    # a dependency that crosses a project boundary resolves to the emitted id.
    projects = []
    name_to_id = {}
    for project_path in project_paths:
        if skip_missing and not host_file_exists(_xcode_abs(_xcode_pbxproj_path(project_path))):
            continue
        project = _xcode_read_pbxproj(ctx, project_path)
        objects = project["objects"]
        root_project = objects[project["rootObject"]] or {}
        project_dir = _xcode_project_dir(project_path)
        # File references store paths relative to their enclosing group, so
        # resolve the full package-relative path of every file and group once by
        # walking the group tree rooted at the project directory.
        path_maps = _xcode_group_file_paths(objects, root_project, project_dir)
        project_settings = _xcode_project_settings(ctx, objects, project["rootObject"], configuration, path_maps)
        native_targets = [
            objects[target_id]
            for target_id in (root_project.get("targets") or [])
            if (objects.get(target_id) or {}).get("isa") == "PBXNativeTarget"
        ]
        native_by_name = {}
        for target in native_targets:
            native_by_name[target.get("name") or ""] = target
            name = _xcode_sanitized_target_name(target.get("name") or "")
            if name:
                name_to_id[target.get("name") or ""] = name
        projects.append({
            "objects": objects,
            "path_maps": path_maps,
            "file_paths": path_maps["files"],
            "project_settings": project_settings,
            "project_dir": project_dir,
            "project_path": project_path,
            "native_targets": native_targets,
            "native_by_name": native_by_name,
        })

    # Pass two: lower every project's native targets, resolving dependencies and
    # test hosts against the workspace-wide name map.
    all_specs = []
    test_plan_settings = _xcode_test_plan_settings(ctx)
    for project in projects:
        objects = project["objects"]
        native_targets = project["native_targets"]
        native_by_name = project["native_by_name"]
        path_maps = project["path_maps"]
        file_paths = project["file_paths"]
        project_settings = project["project_settings"]
        project_dir = project["project_dir"]
        project_path = project["project_path"]

        # Transitive closure of native target dependencies, so a test bundle that
        # imports a framework reached only through its host application still sees
        # that framework's module on its compile path.
        dep_closure = {}
        for target in native_targets:
            dep_closure[target.get("name") or ""] = _xcode_transitive_deps(objects, target, name_to_id, native_by_name)

        # Reconcile local Swift package products into Once Apple libraries. The
        # package manifest supplies graph metadata only; Once compiles every
        # source target directly rather than delegating package builds.
        package_refs = _xcode_spm_package_refs(objects)
        local_package_infos = _xcode_local_swift_package_infos(ctx, project_dir, package_refs)
        package_infos = _xcode_expand_swift_package_infos(
            ctx,
            local_package_infos + _xcode_remote_swift_package_infos(ctx, entry_path, project_path, package_refs),
        )
        per_target_products = {}
        for target in native_targets:
            products = _xcode_target_spm_products(objects, target, package_refs)
            per_target_products[target.get("name") or ""] = products
        package_platform = _xcode_spm_platform(ctx, objects, native_targets, project_settings, configuration, path_maps)
        package_minimum_os = _xcode_spm_min_os(ctx, objects, native_targets, package_platform, configuration, project_settings, path_maps)
        package_graph = _xcode_local_swift_package_specs(
            ctx,
            package_infos,
            package_platform,
            package_minimum_os,
            ctx["attr"].get("sdk_variant") or "simulator",
            target_prefix = "XcodePackage_" + _xcode_sanitized_target_name(ctx["label"]["id"]),
        )
        xcframework_specs = _xcode_workspace_xcframework_specs(ctx, package_platform, ctx["attr"].get("sdk_variant") or "simulator")
        xcframework_names = {
            spec["attrs"]["bundle"]: spec["name"]
            for spec in xcframework_specs
        }

        for target in native_targets:
            spec = _xcode_lower_target(ctx, objects, target, project_settings, name_to_id, dep_closure, configuration, file_paths, project_dir, path_maps, test_plan_settings)
            if spec == None:
                continue
            for dependency in _xcode_xcframework_dependencies(objects, target, file_paths, xcframework_names):
                spec["deps"] = _unique(spec["deps"] + ["./" + dependency])
            for product in per_target_products[target.get("name") or ""]:
                identity = product.get("package_identity") or ""
                dep_ids = package_graph["products"].get(identity + "\x1f" + product["name"]) or package_graph["products"].get(product["name"])
                if dep_ids and type(dep_ids) != "list":
                    dep_ids = [dep_ids]
                for dep_id in dep_ids or []:
                    spec["deps"] = _unique(spec["deps"] + ["./" + dep_id])
            for module in _xcode_swift_imports(spec["srcs"]):
                dep_ids = name_to_id.get(module) or package_graph["products"].get(module) or package_graph["modules"].get(module)
                if dep_ids and type(dep_ids) != "list":
                    dep_ids = [dep_ids]
                for dep_id in dep_ids or []:
                    if dep_id != spec["name"]:
                        spec["deps"] = _unique(spec["deps"] + ["./" + dep_id])
            if spec["kind"] == "apple_library":
                spec["attrs"]["exported_deps"] = list(spec["deps"])
            all_specs.append(spec)
        all_specs.extend(package_graph["specs"])
        all_specs.extend(xcframework_specs)

    # A target id can appear in more than one project (rare); keep the first.
    specs = []
    seen_ids = {}
    for spec in all_specs:
        if spec["name"] in seen_ids:
            continue
        seen_ids[spec["name"]] = True
        specs.append(spec)

    # Drop dependency edges to targets that were not emitted (for example a
    # resource-only extension, or a pod linked only through an xcconfig), so no
    # consumer references a target that is not in the graph.
    emitted = {spec["name"]: True for spec in specs}
    for spec in specs:
        spec["deps"] = [dep for dep in spec["deps"] if dep[2:] in emitted]

    # The application targets are the natural build roots; tests are roots too
    # so `once test` can reach them, but applications take precedence in order.
    return {"targets": specs, "roots": _xcode_roots(specs)}

def _xcode_spm_platform(ctx, objects, native_targets, project_settings, configuration, path_maps):
    # The synthesized package builds for one platform. Prefer the project's
    # `SDKROOT`; multi-platform projects leave it empty, so fall back to a
    # consuming target's SDK.
    sdkroot = _xcode_scalar(project_settings.get("SDKROOT"))
    if sdkroot:
        return _xcode_platform(sdkroot)
    for target in native_targets:
        config_list = objects.get(target.get("buildConfigurationList")) or {}
        default_name = config_list.get("defaultConfigurationName") or "Release"
        settings = _xcode_effective_settings_for_list(ctx, objects, config_list, default_name, configuration, path_maps)
        if _xcode_scalar(settings.get("SDKROOT")) or _xcode_scalar(settings.get("SUPPORTED_PLATFORMS")) or _xcode_scalar(settings.get("MACOSX_DEPLOYMENT_TARGET")) or _xcode_scalar(settings.get("IPHONEOS_DEPLOYMENT_TARGET")):
            return _xcode_effective_platform(settings, project_settings)
    return "macos"

def _xcode_workspace_xcframework_specs(ctx, platform, sdk_variant):
    # CocoaPods and similar project generators may leave a prebuilt framework
    # next to a native target instead of expressing it as a package dependency.
    # Treat each discovered bundle as a typed import so its selected slice
    # participates in compilation and linking. `find -L` follows generated
    # workspace symlinks to verified artifact caches.
    root = workspace_root()
    infos = host_command([host_which("find"), "-L", root, "-type", "f", "-name", "Info.plist"])
    specs = []
    seen = {}
    for absolute in infos.split("\n"):
        relative = _xcode_workspace_relative(absolute)
        marker = ".xcframework/Info.plist"
        index = relative.find(marker)
        if index < 0:
            continue
        bundle = relative[:index + len(".xcframework")]
        name = "XCFramework_" + _xcode_sanitized_target_name(bundle)
        if bundle in seen:
            continue
        seen[bundle] = True
        specs.append({
            "name": name,
            "kind": "apple_xcframework_import",
            "deps": [],
            "srcs": [],
            "attrs": {
                "bundle": bundle,
                "platform": platform,
                "sdk_variant": sdk_variant,
            },
        })
    return specs

def _xcode_transitive_deps(objects, target, name_to_id, native_by_name, seen = None):
    seen = seen or {}
    out = []
    for name in _unique(_xcode_target_dependencies(objects, target) + _xcode_framework_product_dependencies(objects, target, name_to_id)):
        dep_id = name_to_id.get(name) or _xcode_sanitized_target_name(name)
        if not dep_id or dep_id in seen:
            continue
        seen[dep_id] = True
        out.append(dep_id)
        dep_target = native_by_name.get(name)
        if dep_target != None:
            for transitive in _xcode_transitive_deps(objects, dep_target, name_to_id, native_by_name, seen):
                if transitive not in out:
                    out.append(transitive)
    return out

def _xcode_roots(specs):
    applications = [spec["name"] for spec in specs if spec["kind"] == "apple_application"]
    if applications:
        return applications
    return [spec["name"] for spec in specs]

def _xcode_lower_target(ctx, objects, target, project_settings, name_to_id, dep_closure, configuration, file_paths, project_dir, path_maps, test_plan_settings = {}):
    product_type = target.get("productType") or ""
    kind = _xcode_product_kind(product_type)
    if not kind:
        return None

    config_list = objects.get(target.get("buildConfigurationList")) or {}
    default_name = config_list.get("defaultConfigurationName") or "Release"
    selected_config_id = _xcode_choose_config(objects, config_list.get("buildConfigurations") or [], default_name, configuration)
    selected_configuration = (objects.get(selected_config_id) or {}).get("name") or configuration
    settings = _xcode_effective_settings(ctx, objects, config_list, selected_configuration, project_settings, target.get("name") or "", path_maps)

    target_name = target.get("name") or ""
    sanitized = _xcode_sanitized_target_name(target_name)
    sdkroot = _xcode_scalar(settings.get("SDKROOT")) or _xcode_scalar(project_settings.get("SDKROOT"))
    platform = _xcode_effective_platform(settings, project_settings)
    sdk_variant = ctx["attr"].get("sdk_variant") or "simulator"
    sdk_name = {
        "ios": "iphonesimulator" if sdk_variant == "simulator" else "iphoneos",
        "tvos": "appletvsimulator" if sdk_variant == "simulator" else "appletvos",
        "watchos": "watchsimulator" if sdk_variant == "simulator" else "watchos",
        "visionos": "xrsimulator" if sdk_variant == "simulator" else "xros",
        "macos": "macosx",
    }.get(platform) or sdkroot
    settings = _xcode_select_conditional_settings(settings, {
        "arch": host_arch(),
        "config": selected_configuration,
        "sdk": sdk_name,
        "target": target_name,
    })
    product_name_seed = _xcode_resolve_vars(_xcode_scalar(settings.get("PRODUCT_NAME")), {"TARGET_NAME": target_name}) or target_name
    if not product_name_seed or product_name_seed.startswith("$("):
        product_name_seed = target_name
    resolved_sdkroot = host_command([host_which("xcrun"), "--sdk", sdk_name, "--show-sdk-path"]).strip() if sdk_name else sdkroot
    subs = _xcode_setting_subs(ctx, target_name, product_name_seed, resolved_sdkroot, settings, project_dir, selected_configuration)
    wrapper_suffix = ".xctest" if kind == "test" else (".app" if kind in ["application", "extension", "watch_app"] else "")
    if wrapper_suffix:
        wrapper_name = product_name_seed + wrapper_suffix
        subs["WRAPPER_NAME"] = wrapper_name
        subs["FULL_PRODUCT_NAME"] = wrapper_name
        subs["CONTENTS_FOLDER_PATH"] = wrapper_name
        subs["UNLOCALIZED_RESOURCES_FOLDER_PATH"] = wrapper_name
        subs["EXECUTABLE_NAME"] = product_name_seed

    files = _xcode_target_files(ctx, objects, target, file_paths, project_dir, path_maps)
    shell_scripts = _xcode_shell_script_phases(ctx, objects, target, subs, project_dir, target_name)
    files["resources"] = _unique(files["resources"] + shell_scripts["resource_inputs"])
    files["structured_resources"] = _unique(files["structured_resources"] + shell_scripts["structured_resource_inputs"])
    for file_kind in ["sources", "headers", "exported_headers", "resources", "structured_resources", "asset_catalogs", "intent_definitions"]:
        files[file_kind] = _xcode_filter_excluded_files(files[file_kind], settings)
    swift_version = _xcode_scalar(settings.get("SWIFT_VERSION"))
    data_models = _xcode_datamodel_sources(ctx, files["resources"], product_name_seed, swift_version, target_name)
    intents = _xcode_intent_sources(ctx, files["intent_definitions"], target_name)
    files["sources"] = _unique(files["sources"] + data_models["sources"] + intents["sources"])
    direct_deps = _unique(_xcode_target_dependencies(objects, target) + _xcode_framework_product_dependencies(objects, target, name_to_id))
    is_test = kind == "test"
    closure = dep_closure.get(target_name) or []
    dep_names = closure if is_test else direct_deps
    deps = [_xcode_dep_ref(name_to_id, name) for name in dep_names]
    deps = [dep for dep in deps if dep]

    test_host = _xcode_test_host_ref(objects, settings, name_to_id)
    if test_host and _xcode_dep_ref(name_to_id, test_host) not in deps:
        deps.append(_xcode_dep_ref(name_to_id, test_host))

    if kind == "application" or kind == "extension" or kind == "watch_app":
        attrs, product_name = _xcode_application_attrs(ctx, target, settings, subs, platform, files)
        spec_kind = "apple_application"
        if kind == "extension":
            attrs["application_extension"] = True
    elif kind == "framework":
        attrs, product_name = _xcode_library_attrs(ctx, target, settings, subs, platform, files)
        if product_type == "com.apple.product-type.framework":
            attrs["product_name"] = product_name
            attrs["bundle_id"] = _xcode_bundle_id(settings, subs, product_name)
            if files["resources"]:
                attrs["resources"] = files["resources"]
                attrs["structured_resources"] = files["structured_resources"]
            if files["asset_catalogs"]:
                attrs["asset_catalogs"] = files["asset_catalogs"]
            spec_kind = "apple_framework"
        else:
            spec_kind = "apple_library"
    elif kind == "library":
        attrs, product_name = _xcode_library_attrs(ctx, target, settings, subs, platform, files)
        spec_kind = "apple_library"
    elif kind == "test":
        attrs, product_name = _xcode_test_attrs(ctx, target, settings, subs, platform, files)
        if product_type == "com.apple.product-type.bundle.ui-testing":
            attrs["ui_testing"] = True
        plan_settings = test_plan_settings.get(target_name) or {}
        if plan_settings.get("test_env"):
            attrs["test_env"] = plan_settings["test_env"]
        if plan_settings.get("test_arguments"):
            attrs["test_arguments"] = plan_settings["test_arguments"]
        if plan_settings.get("skipped_tests"):
            attrs["skipped_tests"] = plan_settings["skipped_tests"]
        spec_kind = "apple_test_bundle"
    elif kind == "bundle":
        product_name = _xcode_product_name(settings, target, subs)
        attrs = {
            "platform": platform,
            "bundle_name": product_name,
            "bundle_id": _xcode_bundle_id(settings, subs, product_name),
            "resources": files["resources"],
            "structured_resources": files["structured_resources"],
        }
        minimum_os = _xcode_minimum_os(settings, platform)
        if minimum_os:
            attrs["minimum_os"] = minimum_os
        if platform != "macos":
            attrs["sdk_variant"] = ctx["attr"].get("sdk_variant") or "simulator"
        developer_dir = ctx["attr"].get("xcode_developer_dir")
        if developer_dir:
            attrs["xcode_developer_dir"] = developer_dir
        spec_kind = "apple_resource_bundle"
    else:
        return None

    authored_modulemap = attrs.get("modulemap") or ""
    if authored_modulemap and not authored_modulemap.startswith("/") and project_dir:
        candidate = project_dir + "/" + authored_modulemap
        if host_file_exists(_xcode_abs(candidate)):
            attrs["modulemap"] = candidate
            authored_modulemap = candidate
    if authored_modulemap:
        modulemap_headers = _xcode_modulemap_headers(authored_modulemap, files["headers"])
        if modulemap_headers:
            attrs["modulemap_headers"] = modulemap_headers

    prebuild_actions = shell_scripts["actions"] + data_models["actions"] + intents["actions"]
    if prebuild_actions:
        attrs["prebuild_actions"] = prebuild_actions

    if not files["sources"] and kind != "bundle":
        # A target with no compilable sources cannot be lowered to a code
        # target: a resource-only bundle, or a script-only Safari App Extension
        # whose only input is JavaScript.
        return None

    attrs = _xcode_lowered_attrs(ctx, attrs)

    return {
        "name": sanitized,
        "kind": spec_kind,
        "deps": _unique(deps),
        "srcs": [_xcode_target_input_path(ctx, path) for path in files["sources"]],
        "attrs": attrs,
    }

def _xcode_dep_ref(name_to_id, name):
    target_name = name_to_id.get(name) or _xcode_sanitized_target_name(name)
    if not target_name:
        return ""
    return "./" + target_name

def _xcode_workspace_impl(ctx):
    return {}

# ---------------------------------------------------------------------------
# Public target kind + native integration
# ---------------------------------------------------------------------------

xcode_workspace = target_kind(
    docs = "Native Xcode project seed. Its resolver reads `project.pbxproj` through `plutil`, flattens `.xcconfig` includes and layered build settings, resolves file references including Xcode file-system synchronized groups, and lowers every native target into the existing Apple target kinds so Once can compile and test the project directly.",
    attrs = [
        attr("project", "string", docs = "Package-relative path to the `.xcodeproj` directory, such as `App.xcodeproj`. Inferred from a single `*.xcodeproj` when omitted.", configurable = False),
        attr("configuration", "string", default = "Debug", docs = "Xcode build configuration whose settings drive target lowering.", configurable = False),
        attr("sdk_variant", "string", default = "simulator", docs = "`simulator` or `device` SDK selection applied to lowered Apple targets on non-macOS platforms.", configurable = False),
        attr("xcode_developer_dir", "string", docs = "Optional `DEVELOPER_DIR` override folded into lowered Apple target cache keys.", configurable = False),
        attr("binary_artifact_authorization_env", "string", docs = "Optional environment-variable name supplying an Authorization header while downloading private binary package artifacts. The variable value is never recorded in the graph or cache.", configurable = False),
        attr("resolver_inputs", "list<string>", default = "[]", docs = "Package-relative text globs supplied to native integration resolution. Defaults to srcs when empty.", configurable = False),
    ],
    resolver = _xcode_workspace_resolver,
    deps = [dep("deps", ["apple_linkable", "apple_application", "apple_test_bundle", "native_linkable"], "Native Xcode targets lowered into Apple application, library, framework, and test targets.")],
    providers = ["xcode_workspace"],
    capabilities = [capability("build", [])],
    tools = [_XCODE_TOOL],
    examples = [
        example(
            "xcode-workspace-native-project",
            name = "Xcode native integration seed",
            use_when = "Use this when an Xcode project should derive Apple application, framework, and test targets from project.pbxproj.",
            platforms = ["macos"],
        ),
        example(
            "xcode-generated-source-e2e",
            name = "Xcode generated Swift source",
            use_when = "Use this when an Xcode shell phase generates Swift source consumed by the same target.",
            path = "examples/xcode-generated-source-e2e",
            platforms = ["macos"],
        ),
    ],
    impl = _xcode_workspace_impl,
)

xcode = native_project(
    target_kind = "xcode_workspace",
    name = "xcode",
    target_name = "xcode",
    docs = "Recognizes a native Xcode project from a checked-in `*.xcodeproj/project.pbxproj`.",
    markers = ["*.xcodeproj/project.pbxproj"],
    inputs = ["*.xcodeproj/**/*.xcscheme", "*.xcworkspace/contents.xcworkspacedata", "**/*.xcconfig"],
    exclude = _native_project_generated_dirs() + ["Pods", "Carthage", "DerivedData", "node_modules"],
    on_match = "all",
    max_depth = 16,
    requires_tools = ["plutil", "xcrun"],
)
