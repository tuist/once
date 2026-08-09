# Xcode native project reader.
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

_XCODE_TOOL = tool("xcode", executables = ["xcodebuild", "plutil", "xcrun"])

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
        return configured
    candidates = [path for path in glob(["*.xcodeproj/project.pbxproj"]) if _ends_with(path, "/project.pbxproj")]
    if len(candidates) == 0:
        fail(ctx["label"]["id"] + ": no `project` attribute was set and no `*.xcodeproj/project.pbxproj` was found in the package")
    if len(candidates) > 1:
        fail(ctx["label"]["id"] + ": multiple Xcode projects found; set the `project` attribute to one of: " + ", ".join(candidates))
    # candidates are workspace-relative; trim to package-relative.
    package = ctx["label"]["package"]
    relative = candidates[0]
    if package and relative.startswith(package + "/"):
        relative = relative[len(package) + 1:]
    return relative

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
    # Enumerate every `.xcodeproj` a workspace references.
    workspace_dir = _xcode_workspace_dir(workspace_path)
    raw = host_file_read(_xcode_abs(_xcode_workspace_data_path(workspace_path)))
    return _xcode_parse_workspace_data(raw, workspace_dir)

def _xcode_project_dir(project_path):
    # The project directory (`SOURCE_ROOT`) is the parent of the `.xcodeproj`
    # bundle, relative to the package. `project_path` is either the bundle path
    # (`App.xcodeproj`) or the manifest path (`App.xcodeproj/project.pbxproj`);
    # normalize both to the bundle's parent directory.
    if _ends_with(project_path, "/project.pbxproj"):
        return _parent_dir(_parent_dir(project_path))
    return _parent_dir(project_path)

def _xcode_read_pbxproj(ctx, project_path):
    pbxproj = project_path if _ends_with(project_path, "/project.pbxproj") else project_path + "/project.pbxproj"
    raw = host_command(["plutil", "-convert", "json", "-o", "-", _xcode_abs(pbxproj)])
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
    # Keep quoted fragments intact; otherwise split on whitespace.
    if "'" in stripped or '"' in stripped:
        return [stripped]
    parts = []
    for piece in stripped.split(" "):
        if piece:
            parts.append(piece)
    return parts

def _xcode_read_xcconfig(ctx, path):
    # Flatten a single .xcconfig file. Returns a dict of key -> raw value
    # string (no variable expansion yet). `#include` directives are resolved
    # relative to the file, then relative to the project directory. Own values
    # win; included values fill only the gaps, matching XcodeProj semantics.
    if not path or not host_file_exists(_xcode_abs(path)):
        return {}
    base_dir = _parent_dir(path)
    content = host_file_read(_xcode_abs(path))
    own = {}
    includes = []
    for raw_line in content.split("\n"):
        line = raw_line.strip()
        if not line or line.startswith("//"):
            continue
        include = _xcode_parse_include(line)
        if include != None:
            includes.append(include)
            continue
        pair = _xcode_parse_setting(line)
        if pair != None:
            own[pair[0]] = pair[1]
    flattened = {}
    for include_path in includes:
        # An `#include` is relative to the including file's directory (a leading
        # slash means package-relative).
        resolved = include_path if include_path.startswith("/") else _xcode_join(base_dir, include_path)
        for key, value in _xcode_read_xcconfig(ctx, resolved).items():
            if key not in own:
                flattened[key] = value
    for key, value in own.items():
        flattened[key] = value
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
    equals = line.find("=")
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
    if "$(" not in text:
        return text
    return _xcode_expand_once(text, subs, 0)

def _xcode_expand_once(text, subs, depth):
    if depth > 16:
        return text
    start = text.find("$(")
    if start < 0:
        return text
    end = text.find(")", start + 2)
    if end < 0:
        return text
    name = text[start + 2:end]
    head = text[:start]
    tail = text[end + 1:]
    if name in subs:
        replacement = _xcode_expand_once(str(subs[name]), subs, depth + 1)
    else:
        replacement = "$(" + name + ")"
    return _xcode_expand_once(head + replacement + tail, subs, depth + 1)

def _xcode_setting_subs(ctx, target_name, product_name, sdkroot):
    return {
        "SRCROOT": "",
        "PROJECT_DIR": "",
        "TARGET_NAME": target_name,
        "PRODUCT_NAME": product_name,
        "PRODUCT_MODULE_NAME": product_name.replace("-", "_"),
        "SDKROOT": sdkroot or "",
        "CONFIGURATION": ctx["attr"].get("configuration") or "Debug",
    }

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
        return prefix
    if not prefix:
        return segment
    if segment.startswith("/"):
        return segment
    if prefix.endswith("/"):
        return prefix + segment
    return prefix + "/" + segment

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
    # package-relative path of every PBXFileReference. Xcode stores file
    # references with a leaf `path` that is relative to the enclosing group, so
    # the full path only exists as the concatenation of the group chain. Group
    # directories are recorded too, so a reference given as an anchor group plus
    # a relative path (the newer `.xcconfig` reference form) can be resolved.
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

def _xcode_filter_excluded_sources(sources, settings):
    # Drop sources matched by `EXCLUDED_SOURCE_FILE_NAMES` unless
    # `INCLUDED_SOURCE_FILE_NAMES` matches them back in. Patterns match against
    # both the file's basename and its package-relative path, mirroring how
    # Xcode filters per-platform source variants out of a target.
    excluded = _xcode_setting_to_list(settings.get("EXCLUDED_SOURCE_FILE_NAMES"))
    if not excluded:
        return sources
    included = _xcode_setting_to_list(settings.get("INCLUDED_SOURCE_FILE_NAMES"))
    out = []
    for src in sources:
        base = _basename(src)
        if _xcode_matches_any(excluded, src, base) and not _xcode_matches_any(included, src, base):
            continue
        out.append(src)
    return out

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
    return _ends_with(path, ".xcassets")

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

def _xcode_asset_catalog_dir(path):
    # If a path is a `.xcassets` bundle or lives inside one, return the catalog
    # directory (up to and including `.xcassets`); otherwise the empty string.
    marker = ".xcassets"
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
    buckets = {"sources": [], "headers": [], "resources": [], "asset_catalogs": []}
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
            _xcode_classify_synced_path(_xcode_join(entry["base"], trimmed), buckets)

    return {
        "sources": _unique(buckets["sources"]),
        "headers": _unique(buckets["headers"]),
        "resources": _unique(buckets["resources"]),
        "asset_catalogs": _unique(buckets["asset_catalogs"]),
    }

def _xcode_classic_phase_files(ctx, objects, target, file_paths):
    # Classic projects list PBXBuildFile entries per phase. Resolve each back
    # to its PBXFileReference's full package-relative path (through the group
    # tree), falling back to the leaf path when the reference is not rooted in
    # a group (for example an SDK framework).
    sources = []
    headers = []
    resources = []
    asset_catalogs = []
    frameworks = []
    for phase_id in target.get("buildPhases") or []:
        phase = objects.get(phase_id) or {}
        isa = phase.get("isa") or ""
        for build_file_id in phase.get("files") or []:
            build_file = objects.get(build_file_id) or {}
            file_ref_id = build_file.get("fileRef")
            file_ref = objects.get(file_ref_id) or {}
            path = file_paths.get(file_ref_id) or _xcode_file_ref_path(ctx, file_ref)
            if not path:
                continue
            if isa == "PBXSourcesBuildPhase":
                if _xcode_is_source(path):
                    sources.append(path)
                elif _xcode_is_header(path):
                    headers.append(path)
            elif isa == "PBXResourcesBuildPhase":
                if _xcode_is_asset_catalog(path):
                    asset_catalogs.append(path)
                else:
                    resources.append(path)
            elif isa == "PBXHeadersBuildPhase":
                headers.append(path)
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
        "resources": _unique(resources),
        "asset_catalogs": _unique(asset_catalogs),
        "frameworks": _unique(frameworks),
    }

def _xcode_target_files(ctx, objects, target, file_paths, project_dir, path_maps):
    classic = _xcode_classic_phase_files(ctx, objects, target, file_paths)
    synced = _xcode_synced_group_files(ctx, objects, target, project_dir, path_maps)
    return {
        "sources": _unique(classic["sources"] + synced["sources"]),
        "headers": _unique(classic["headers"] + synced["headers"]),
        "resources": _unique(classic["resources"] + synced["resources"]),
        "asset_catalogs": _unique(classic["asset_catalogs"] + synced["asset_catalogs"]),
        "frameworks": classic["frameworks"],
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
    # Resolve a native target's `packageProductDependencies` to
    # `{name, package_identity}` records, skipping products whose package is
    # declared in a workspace rather than the project (unresolved here).
    products = []
    for product_id in target.get("packageProductDependencies") or []:
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
    # those local packages and map each wanted product to its package name and
    # directory by reading each manifest with `swift package dump-package`
    # (offline, no dependency resolution). Returns product name -> {package,
    # path (absolute)}.
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
        if _xcode_is_excluded_source_path(manifest):
            continue
        package_dir = _parent_dir(manifest)
        absolute = _xcode_abs(package_dir)
        raw = host_command([swift, "package", "dump-package", "--package-path", absolute])
        info = json_decode(raw)
        package_name = info.get("name") or _basename(package_dir)
        platforms = {}
        for entry in info.get("platforms") or []:
            name = entry.get("platformName") or ""
            version = entry.get("version") or ""
            if name and version:
                platforms[name] = version
        for product in info.get("products") or []:
            product_name = product.get("name") or ""
            if product_name in remaining:
                resolved[product_name] = {"package": package_name, "path": absolute, "platforms": platforms}
                remaining.pop(product_name)
    return resolved

def _xcode_spm_dependency_clause(url, requirement):
    # Render a `Package.swift` `.package(url:...)` clause from a pbxproj version
    # requirement so a synthesized manifest resolves to the project's versions.
    kind = requirement.get("kind") or ""
    if kind == "upToNextMajorVersion":
        return '.package(url: "' + url + '", .upToNextMajor(from: "' + (requirement.get("minimumVersion") or "0.0.0") + '"))'
    if kind == "upToNextMinorVersion":
        return '.package(url: "' + url + '", .upToNextMinor(from: "' + (requirement.get("minimumVersion") or "0.0.0") + '"))'
    if kind == "exactVersion":
        return '.package(url: "' + url + '", exact: "' + (requirement.get("version") or "0.0.0") + '")'
    if kind == "versionRange":
        return '.package(url: "' + url + '", "' + (requirement.get("minimumVersion") or "0.0.0") + '"..<"' + (requirement.get("maximumVersion") or "0.0.0") + '")'
    if kind == "branch":
        return '.package(url: "' + url + '", branch: "' + (requirement.get("branch") or "main") + '")'
    if kind == "revision":
        return '.package(url: "' + url + '", revision: "' + (requirement.get("revision") or "") + '")'
    # Fall back to a permissive lower bound so resolution can still proceed.
    return '.package(url: "' + url + '", .upToNextMajor(from: "0.0.0"))'

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
    best = _xcode_scalar(project_settings.get(key))
    for target in native_targets:
        config_list = objects.get(target.get("buildConfigurationList")) or {}
        default_name = config_list.get("defaultConfigurationName") or "Release"
        settings = _xcode_effective_settings_for_list(ctx, objects, config_list, default_name, configuration, path_maps)
        value = _xcode_scalar(settings.get(key))
        if value and (not best or _xcode_version_key(value) > _xcode_version_key(best)):
            best = value
    return best or "11.0"

def _xcode_spm_platform_clause(platform, minimum_os):
    version = minimum_os or "10.15"
    names = {
        "ios": "iOS",
        "macos": "macOS",
        "tvos": "tvOS",
        "watchos": "watchOS",
        "visionos": "visionOS",
    }
    name = names.get(platform) or "macOS"
    return "." + name + '("' + version + '")'

def _xcode_spm_manifest(platform, minimum_os, packages, products):
    # Synthesize an aggregating `Package.swift`: one static library target that
    # depends on every product the Xcode targets consume, so a single
    # `swift build` produces a linkable archive plus each dependency's module
    # and framework. Handles both source packages and binary xcframework
    # products distributed through Swift Package Manager.
    dependency_lines = []
    for package in packages:
        if package.get("path"):
            clause = '.package(path: "' + package["path"] + '")'
        else:
            clause = _xcode_spm_dependency_clause(package["url"], package["requirement"])
        dependency_lines.append("        " + clause + ",")
    product_lines = []
    for product in products:
        product_lines.append('            .product(name: "' + product["name"] + '", package: "' + product["package_identity"] + '"),')
    return "\n".join([
        "// swift-tools-version:5.9",
        "import PackageDescription",
        "",
        "let package = Package(",
        '    name: "OnceXcodeDeps",',
        "    platforms: [" + _xcode_spm_platform_clause(platform, minimum_os) + "],",
        "    products: [",
        '        .library(name: "OnceXcodeDeps", type: .static, targets: ["OnceXcodeDeps"]),',
        "    ],",
        "    dependencies: [",
    ] + dependency_lines + [
        "    ],",
        "    targets: [",
        '        .target(',
        '            name: "OnceXcodeDeps",',
        "            dependencies: [",
    ] + product_lines + [
        "            ]",
        "        ),",
        "    ]",
        ")",
        "",
    ])

# ---------------------------------------------------------------------------
# Target graph
# ---------------------------------------------------------------------------

def _xcode_target_dependencies(objects, target):
    # Resolve PBXTargetDependency -> target name for native target dependencies.
    names = []
    for dep_id in target.get("dependencies") or []:
        dep = objects.get(dep_id) or {}
        remote = dep.get("target")
        if remote == None:
            proxy = objects.get(dep.get("targetProxy")) or {}
            remote = proxy.get("remoteGlobalIDString")
        remote_target = objects.get(remote) or {}
        if remote_target.get("isa") == "PBXNativeTarget":
            names.append(remote_target.get("name") or "")
    return [name for name in names if name]

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

def _xcode_define_flags(settings):
    swift_conds = _xcode_setting_to_list(settings.get("SWIFT_ACTIVE_COMPILATION_CONDITIONS"))
    c_defs = _xcode_setting_to_list(settings.get("GCC_PREPROCESSOR_DEFINITIONS"))
    defines = []
    for cond in swift_conds:
        defines.append(cond)
    for definition in c_defs:
        if definition.startswith("DEBUG=1"):
            defines.append("DEBUG")
        elif definition.startswith("DEBUG=") or definition == "DEBUG":
            defines.append("DEBUG")
        else:
            defines.append(definition)
    return _xcode_clean_flags(_unique(defines))

def _xcode_swift_flags(settings, subs):
    raw = _xcode_setting_to_list(settings.get("OTHER_SWIFT_FLAGS"))
    out = []
    for flag in raw:
        resolved = _xcode_resolve_vars(flag, subs)
        out.append(resolved)
    return _xcode_clean_flags(out)

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
    defines = _xcode_define_flags(settings)
    if defines:
        attrs["defines"] = defines
    swift_flags = _xcode_swift_flags(settings, subs) + _xcode_swift_language_flags(settings)
    if swift_flags:
        attrs["swift_flags"] = swift_flags
    bridging_header = _xcode_resolve_vars(_xcode_scalar(settings.get("SWIFT_OBJC_BRIDGING_HEADER")), subs)
    if bridging_header and not bridging_header.startswith("$("):
        attrs["bridging_header"] = bridging_header
    if files["headers"]:
        attrs["exported_headers"] = files["headers"]
    sdk_frameworks = _xcode_sdk_frameworks(settings, files["frameworks"])
    if sdk_frameworks:
        attrs["sdk_frameworks"] = sdk_frameworks
    developer_dir = ctx["attr"].get("xcode_developer_dir")
    if developer_dir:
        attrs["xcode_developer_dir"] = developer_dir
    return attrs

def _xcode_library_attrs(ctx, target, settings, subs, platform, files):
    attrs = _xcode_common_attrs(ctx, target, settings, subs, platform, files)
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

def _xcode_application_attrs(ctx, target, settings, subs, platform, files):
    attrs = _xcode_common_attrs(ctx, target, settings, subs, platform, files)
    product_name = _xcode_product_name(settings, target, subs)
    if product_name:
        attrs["product_name"] = product_name
    attrs["bundle_id"] = _xcode_bundle_id(settings, subs, product_name)
    families = _xcode_families(settings)
    if families:
        attrs["families"] = families
    if _xcode_scalar(settings.get("ENABLE_TESTABILITY")).upper() == "YES":
        attrs["enable_testing"] = True
    if files["asset_catalogs"]:
        attrs["asset_catalogs"] = files["asset_catalogs"]
        app_icon = _xcode_scalar(settings.get("ASSETCATALOG_COMPILER_APPICON_NAME"))
        # Only ask `actool` to compile the app icon when a matching icon set is
        # actually present; otherwise `actool` treats the missing icon as a hard
        # error and fails the build.
        if app_icon and _xcode_app_icon_exists(files["asset_catalogs"], app_icon):
            attrs["app_icon"] = app_icon
    # apple_application generates its own Info.plist and does not yet accept
    # arbitrary resources or an Info.plist template; those are recorded for
    # future use without lowering them.
    return attrs, product_name

def _xcode_test_attrs(ctx, target, settings, subs, platform, files):
    attrs = _xcode_common_attrs(ctx, target, settings, subs, platform, files)
    product_name = _xcode_product_name(settings, target, subs)
    if product_name:
        attrs["product_name"] = product_name
    bundle_id = _xcode_resolve_vars(_xcode_scalar(settings.get("PRODUCT_BUNDLE_IDENTIFIER")), subs)
    if bundle_id and not bundle_id.startswith("$("):
        attrs["bundle_id"] = bundle_id
    test_style = _xcode_test_style(ctx, files["sources"])
    if test_style == "swift_testing":
        attrs["swift_testing"] = True
    # apple_test_bundle does not yet accept test_host, resources, or an
    # Info.plist template. The host application is still wired through deps
    # so the code under test links.
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
    project_path = _xcode_project_path(ctx)
    project = _xcode_read_pbxproj(ctx, project_path)
    objects = project["objects"]
    root_id = project["rootObject"]
    root_project = objects[root_id] or {}

    configuration = ctx["attr"].get("configuration") or "Debug"

    # File references store paths relative to their enclosing group, so resolve
    # the full package-relative path of every file and group once by walking the
    # group tree rooted at the project directory (the parent of the
    # `.xcodeproj`). The group directories also resolve `.xcconfig` references
    # given as an anchor group plus a relative path.
    project_dir = _xcode_project_dir(project_path)
    path_maps = _xcode_group_file_paths(objects, root_project, project_dir)
    file_paths = path_maps["files"]

    project_settings = _xcode_project_settings(ctx, objects, root_id, configuration, path_maps)

    native_targets = [
        objects[target_id]
        for target_id in (root_project.get("targets") or [])
        if (objects.get(target_id) or {}).get("isa") == "PBXNativeTarget"
    ]

    # Build the name map first so test hosts can resolve to emitted target ids.
    specs_by_name = {}
    name_to_id = {}
    native_by_name = {}
    for target in native_targets:
        native_by_name[target.get("name") or ""] = target
        name = _xcode_sanitized_target_name(target.get("name") or "")
        if not name:
            continue
        specs_by_name[name] = target
        name_to_id[target.get("name") or ""] = name

    # Transitive closure of native target dependencies, so a test bundle that
    # imports a framework reached only through its host application still sees
    # that framework's module on its compile path.
    dep_closure = {}
    for target in native_targets:
        name = target.get("name") or ""
        dep_closure[name] = _xcode_transitive_deps(objects, target, name_to_id, native_by_name)

    # Swift Package Manager dependencies: collect the products every target
    # consumes so a single aggregating package can build them and every consumer
    # can link against the result. Only products backed by a remote package
    # reference in the project are built here; products that resolve through a
    # workspace-local package are not modeled yet and are skipped so the
    # aggregate stays buildable.
    package_refs = _xcode_spm_package_refs(objects)
    remote_identities = {}
    for ref in package_refs.values():
        if ref.get("kind") == "remote" and ref.get("identity"):
            remote_identities[ref.get("identity")] = True

    # First pass: gather each target's products and note the ones with no
    # remote package reference so they can be resolved against local packages.
    per_target_products = {}
    unresolved_names = {}
    for target in native_targets:
        products = _xcode_target_spm_products(objects, target, package_refs)
        per_target_products[target.get("name") or ""] = products
        for product in products:
            if product["package_identity"] not in remote_identities:
                unresolved_names[product["name"]] = True

    local_products = _xcode_local_package_products(ctx, [name for name in unresolved_names])

    used_products = {}
    local_packages = {}
    local_platform_reqs = {}
    target_uses_spm = {}
    for target in native_targets:
        uses = False
        for product in per_target_products[target.get("name") or ""]:
            name = product["name"]
            if product["package_identity"] in remote_identities:
                used_products[name] = product["package_identity"]
                uses = True
            elif name in local_products:
                info = local_products[name]
                used_products[name] = info["package"]
                local_packages[info["package"]] = info["path"]
                for platform_name, version in (info.get("platforms") or {}).items():
                    if platform_name not in local_platform_reqs or _xcode_version_key(version) > _xcode_version_key(local_platform_reqs[platform_name]):
                        local_platform_reqs[platform_name] = version
                uses = True
        if uses:
            target_uses_spm[target.get("name") or ""] = True

    spm_target_name = ""
    spm_spec = None
    if used_products:
        spm_target_name = "OnceSwiftPackages"
        spm_platform = _xcode_spm_platform(ctx, objects, native_targets, project_settings, configuration, path_maps)
        spm_min_os = _xcode_spm_min_os(ctx, objects, native_targets, spm_platform, configuration, project_settings, path_maps)
        required = local_platform_reqs.get(spm_platform)
        if required and _xcode_version_key(required) > _xcode_version_key(spm_min_os):
            spm_min_os = required
        spm_spec = _xcode_spm_spec(ctx, package_refs, used_products, local_packages, spm_platform, spm_min_os, project_path, spm_target_name)

    specs = []
    roots = []
    for target in native_targets:
        spec = _xcode_lower_target(ctx, objects, target, project_settings, name_to_id, dep_closure, configuration, file_paths, project_dir, path_maps)
        if spec == None:
            continue
        if spm_spec != None and target_uses_spm.get(target.get("name") or ""):
            dep_ref = "./" + spm_target_name
            if dep_ref not in spec["deps"]:
                spec["deps"] = _unique(spec["deps"] + [dep_ref])
        specs.append(spec)
        roots.append(spec["name"])

    # Drop dependency edges to targets that were not lowered (for example a
    # resource-only extension), so no consumer references a target that is not
    # emitted.
    emitted = {spec["name"]: True for spec in specs}
    if spm_spec != None:
        emitted[spm_spec["name"]] = True
    for spec in specs:
        spec["deps"] = [dep for dep in spec["deps"] if dep[2:] in emitted]

    if spm_spec != None:
        specs.append(spm_spec)

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

def _xcode_spm_spec(ctx, package_refs, used_products, local_packages, platform, minimum_os, project_path, spm_target_name):
    # Assemble the `xcode_spm_dependencies` target that builds the project's
    # Swift package products: remote packages referenced in the project and
    # workspace-local packages discovered from their `Package.swift`.
    used_identities = {}
    for identity in used_products.values():
        used_identities[identity] = True
    packages = []
    seen = {}
    for ref in package_refs.values():
        if ref.get("kind") != "remote":
            continue
        identity = ref.get("identity") or ""
        if identity not in used_identities or identity in seen:
            continue
        seen[identity] = True
        packages.append(ref)
    for package_name in sorted(local_packages.keys()):
        packages.append({"path": local_packages[package_name]})

    product_records = []
    product_specs = []
    for name in sorted(used_products.keys()):
        package_identity = used_products[name]
        product_records.append({"name": name, "package_identity": package_identity})
        product_specs.append(name + "\x1f" + package_identity)

    manifest = _xcode_spm_manifest(platform, minimum_os, packages, product_records)

    resolved = _xcode_read_package_resolved(ctx, project_path)

    attrs = {
        "manifest": manifest,
        "resolved": resolved,
        "products": product_specs,
        "platform": platform,
        "minimum_os": minimum_os,
    }
    attrs["configuration"] = "release" if "release" in (ctx["attr"].get("configuration") or "Debug").lower() else "debug"
    sdk_variant = ctx["attr"].get("sdk_variant")
    if sdk_variant:
        attrs["sdk_variant"] = sdk_variant
    developer_dir = ctx["attr"].get("xcode_developer_dir")
    if developer_dir:
        attrs["xcode_developer_dir"] = developer_dir
    return {
        "name": spm_target_name,
        "kind": "xcode_spm_dependencies",
        "deps": [],
        "srcs": [],
        "attrs": attrs,
    }

def _xcode_read_package_resolved(ctx, project_path):
    bundle = project_path
    if _ends_with(bundle, "/project.pbxproj"):
        bundle = _parent_dir(bundle)
    candidates = [
        bundle + "/project.xcworkspace/xcshareddata/swiftpm/Package.resolved",
        bundle + "/project.workspace/xcshareddata/swiftpm/Package.resolved",
    ]
    for candidate in candidates:
        if host_file_exists(_xcode_abs(candidate)):
            return host_file_read(_xcode_abs(candidate))
    return ""

def _xcode_transitive_deps(objects, target, name_to_id, native_by_name, seen = None):
    seen = seen or {}
    out = []
    for name in _xcode_target_dependencies(objects, target):
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

def _xcode_lower_target(ctx, objects, target, project_settings, name_to_id, dep_closure, configuration, file_paths, project_dir, path_maps):
    product_type = target.get("productType") or ""
    kind = _xcode_product_kind(product_type)
    if not kind:
        return None

    config_list = objects.get(target.get("buildConfigurationList")) or {}
    default_name = config_list.get("defaultConfigurationName") or "Release"
    settings = _xcode_effective_settings(ctx, objects, config_list, default_name, project_settings, target.get("name") or "", path_maps)

    target_name = target.get("name") or ""
    sanitized = _xcode_sanitized_target_name(target_name)
    product_name_seed = _xcode_resolve_vars(_xcode_scalar(settings.get("PRODUCT_NAME")), {"TARGET_NAME": target_name}) or target_name
    if not product_name_seed or product_name_seed.startswith("$("):
        product_name_seed = target_name
    sdkroot = _xcode_scalar(settings.get("SDKROOT")) or _xcode_scalar(project_settings.get("SDKROOT"))
    platform = _xcode_effective_platform(settings, project_settings)
    subs = _xcode_setting_subs(ctx, target_name, product_name_seed, sdkroot)

    files = _xcode_target_files(ctx, objects, target, file_paths, project_dir, path_maps)
    files["sources"] = _xcode_filter_excluded_sources(files["sources"], settings)
    direct_deps = _xcode_target_dependencies(objects, target)
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
        spec_kind = "apple_library"
    elif kind == "library":
        attrs, product_name = _xcode_library_attrs(ctx, target, settings, subs, platform, files)
        spec_kind = "apple_library"
    elif kind == "test":
        attrs, product_name = _xcode_test_attrs(ctx, target, settings, subs, platform, files)
        spec_kind = "apple_test_bundle"
    else:
        return None

    if not files["sources"]:
        # A target with no compilable sources cannot be lowered to a code
        # target: a resource-only bundle, or a script-only Safari App Extension
        # whose only input is JavaScript.
        return None

    return {
        "name": sanitized,
        "kind": spec_kind,
        "deps": _unique(deps),
        "srcs": files["sources"],
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
# Swift Package Manager dependency set
# ---------------------------------------------------------------------------

def _xcode_spm_stage_script(checkouts_dir, bin_dir, include_dir):
    # Stage every source package's Clang module (Objective-C shims, C shims such
    # as a Swift package's atomics support, and Objective-C libraries) into a
    # single include directory with one combined module map, so the consuming
    # Apple targets can import any module a built swiftmodule references.
    #
    # Remote package modules live at `<target>/include/module.modulemap` in the
    # checkouts and reference their headers relatively, so the headers are
    # copied alongside the combined map. Local path packages instead have Swift
    # Package Manager generate the Objective-C target module map under
    # `<target>.build/module.modulemap` in the build directory with absolute
    # header paths, so those maps are appended directly. The `<target>.build/
    # include/module.modulemap` variant is a Swift module's generated
    # compatibility header and is skipped. Binary framework products live in the
    # build directory and are reached through the framework search path.
    return """set -eu
mkdir -p {include}
: > {include}/module.modulemap
find {checkouts} -path '*/include/module.modulemap' 2>/dev/null | while IFS= read -r mm; do
  target_dir=$(dirname "$(dirname "$mm")")
  find "$target_dir" -name '*.h' -exec cp -f {{}} {include}/ \\; 2>/dev/null || true
  cat "$mm" >> {include}/module.modulemap
  printf '\\n' >> {include}/module.modulemap
done
find {bin} -path '*.build/module.modulemap' 2>/dev/null | while IFS= read -r mm; do
  cat "$mm" >> {include}/module.modulemap
  printf '\\n' >> {include}/module.modulemap
done
""".format(
        include = _shell_literal(include_dir),
        checkouts = _shell_literal(checkouts_dir),
        bin = _shell_literal(bin_dir),
    )

def _xcode_spm_dependencies_impl(ctx):
    attrs = ctx["attr"]
    platform = attrs.get("platform") or "macos"
    minimum_os = attrs.get("minimum_os") or "10.15"
    sdk_variant = attrs.get("sdk_variant") or "simulator"
    arch = attrs.get("arch") or host_arch()
    # Build the packages with the project's configuration (Debug by default),
    # matching what Xcode builds and avoiding release-only optimizer crashes in
    # some packages.
    configuration = attrs.get("configuration") or "debug"
    xcode_developer_dir = attrs.get("xcode_developer_dir") or ""
    manifest = attrs.get("manifest") or ""
    resolved = attrs.get("resolved") or ""
    product_specs = attrs.get("products") or []

    swiftc = _resolve_swiftc(platform, sdk_variant, xcode_developer_dir)
    swift = _swiftpm_swift_executable(attrs.get("swift") or "swift", xcode_developer_dir, swiftc["swiftc_path"])
    version = host_command([swift, "--version"], env = swiftc["env"]).strip()
    action_env = dict(swiftc["env"])
    action_path = _parent_dir(swiftc["swiftc_path"]) + ":" + _parent_dir(swift) + ":/usr/bin:/bin"
    action_env["PATH"] = action_path
    triple = _apple_triple(platform, minimum_os, sdk_variant, arch, False)
    build_triple_dir = _swiftpm_build_triple_dir(platform, sdk_variant, arch)

    # Materialize the synthesized aggregating package.
    pkg_dir = ctx["build_dir"] + "/spm"
    manifest_path = pkg_dir + "/Package.swift"
    source_path = pkg_dir + "/Sources/OnceXcodeDeps/Empty.swift"
    write_path(manifest_path, manifest)
    write_path(source_path, "// Once aggregating package for Xcode SwiftPM dependencies\n")
    build_inputs = [manifest_path, source_path]
    if resolved:
        resolved_path = pkg_dir + "/Package.resolved"
        write_path(resolved_path, resolved)
        build_inputs.append(resolved_path)

    scratch = declare_output("spm-build")
    bin_dir = scratch + "/" + build_triple_dir + "/" + configuration
    checkouts_dir = scratch + "/checkouts"
    archive = bin_dir + "/libOnceXcodeDeps.a"
    module_dir = bin_dir + "/Modules"

    build_argv = [
        swift,
        "build",
        "--package-path",
        pkg_dir,
        "--scratch-path",
        scratch,
        "--configuration",
        configuration,
        "--triple",
        triple,
        "--sdk",
        swiftc["sdk_path"],
        "--manifest-cache",
        "local",
    ]
    run_action(
        argv = build_argv,
        inputs = build_inputs,
        outputs = [bin_dir, checkouts_dir],
        clean_paths = [scratch],
        env = action_env,
        toolchain_identity = "once.xcode.spm.build.v1\x00" + version + "\x00" + swiftc["identity"] + "\x00" + triple,
        identifier = "xcode_spm_build:" + ctx["label"]["id"],
    )

    include_dir = ctx["build_dir"] + "/spmroot/include"
    stage_script = _xcode_spm_stage_script(checkouts_dir, bin_dir, include_dir)
    run_action(
        argv = [host_which("sh"), "-c", stage_script],
        inputs = [bin_dir, checkouts_dir],
        outputs = [include_dir],
        clean_paths = [include_dir],
        env = {},
        toolchain_identity = "once.xcode.spm.stage.v1",
        identifier = "xcode_spm_stage:" + ctx["label"]["id"],
    )

    return {
        "label_id": ctx["label"]["id"],
        "xcode_spm_dependencies": True,
        "swiftmodule_dir": module_dir,
        "archive": archive,
        "transitive_archives": [archive],
        "transitive_alwayslink_archives": [],
        "transitive_swiftmodule_dirs": [module_dir],
        "transitive_exported_headers": [],
        "transitive_exported_header_dirs": [include_dir],
        "transitive_modulemaps": [include_dir + "/module.modulemap"],
        "transitive_hmaps": [],
        "transitive_framework_search_dirs": [bin_dir],
        "transitive_embed_framework_dirs": [bin_dir],
        "transitive_sdk_frameworks": [],
        "transitive_sdk_dylibs": [],
        "transitive_linkopts": [],
        "transitive_defines": [],
        "transitive_link_framework_bundles": [],
        "transitive_framework_bundles": [],
    }

# ---------------------------------------------------------------------------
# Public target kind + native project
# ---------------------------------------------------------------------------

xcode_workspace = target_kind(
    docs = "Native Xcode project seed. Its resolver reads `project.pbxproj` through `plutil`, flattens `.xcconfig` includes and layered build settings, resolves file references including Xcode file-system synchronized groups, and lowers every native target into the existing Apple target kinds so Once can compile and test the project directly.",
    attrs = [
        attr("project", "string", docs = "Package-relative path to the `.xcodeproj` directory, such as `App.xcodeproj`. Inferred from a single `*.xcodeproj` when omitted.", configurable = False),
        attr("configuration", "string", default = "Debug", docs = "Xcode build configuration whose settings drive target lowering.", configurable = False),
        attr("sdk_variant", "string", default = "simulator", docs = "`simulator` or `device` SDK selection applied to lowered Apple targets on non-macOS platforms.", configurable = False),
        attr("xcode_developer_dir", "string", docs = "Optional `DEVELOPER_DIR` override folded into lowered Apple target cache keys.", configurable = False),
        attr("resolver_inputs", "list<string>", default = "[]", docs = "Package-relative text globs supplied to native project resolution. Defaults to srcs when empty.", configurable = False),
    ],
    resolver = _xcode_workspace_resolver,
    deps = [dep("deps", ["apple_linkable", "apple_application", "apple_test_bundle", "native_linkable"], "Native Xcode targets lowered into Apple application, library, framework, and test targets.")],
    providers = ["xcode_workspace"],
    capabilities = [capability("build", [])],
    tools = [_XCODE_TOOL],
    examples = [
        example(
            "xcode-workspace-native-project",
            name = "Xcode native project seed",
            use_when = "Use this when an Xcode project should derive Apple application, framework, and test targets from project.pbxproj.",
            platforms = ["macos"],
        ),
    ],
    impl = _xcode_workspace_impl,
)

xcode_spm_dependencies = target_kind(
    docs = "Builds the Swift Package Manager dependencies an Xcode project consumes. The resolver synthesizes an aggregating `Package.swift` from the project's package references and per-target product dependencies; this target runs one `swift build` and exposes the resulting archive, modules, Objective-C module maps, and built frameworks to the lowered Apple targets.",
    attrs = [
        attr("manifest", "string", docs = "Synthesized `Package.swift` describing the consumed packages and products.", configurable = False),
        attr("resolved", "string", default = "", docs = "Contents of the project's `Package.resolved`, applied so the build resolves to the project's pinned versions.", configurable = False),
        attr("products", "list<string>", default = "[]", docs = "Consumed products as `product\\x1fpackage` records used to stage source package modules.", configurable = False),
        attr("platform", "string", default = "macos", docs = "Apple platform the packages are built for.", configurable = False),
        attr("minimum_os", "string", default = "10.15", docs = "Minimum OS version passed to the package build triple.", configurable = False),
        attr("configuration", "string", default = "debug", docs = "`swift build` configuration (`debug` or `release`) matching the project's build configuration.", configurable = False),
        attr("sdk_variant", "string", default = "simulator", docs = "`simulator` or `device` SDK selection on non-macOS platforms.", configurable = False),
        attr("arch", "string", default = "", docs = "Build architecture. Defaults to the host architecture.", configurable = False),
        attr("swift", "string", default = "swift", docs = "Swift driver used to run `swift build`.", configurable = False),
        attr("xcode_developer_dir", "string", default = "", docs = "Optional `DEVELOPER_DIR` override folded into the build cache key.", configurable = False),
    ],
    deps = [],
    providers = ["xcode_spm_dependencies", "apple_linkable"],
    capabilities = [capability("build", ["default"])],
    tools = [_XCODE_TOOL],
    examples = [
        example(
            "xcode-spm-dependencies-minimal",
            name = "Xcode Swift package dependencies",
            use_when = "Use this when an Xcode project's Swift package products should be built once and shared with its native targets.",
        ),
    ],
    impl = _xcode_spm_dependencies_impl,
)
