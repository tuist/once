_NX_NODE_TOOL = tool("node", executables = ["node"])
_NX_TOOL = tool("nx", executables = ["nx"])

_NX_DEFAULT_TASKS = ["build", "test", "lint"]

# Executor names that resolve to `nx:run-commands` behavior: a resolved shell
# command that Once can run directly without going through `nx run`. Nx's own
# aliases and the historical `@nrwl/*` names all land here.
_NX_RUN_COMMANDS_EXECUTORS = {
    "nx:run-commands": True,
    "@nx/run-commands:run-commands": True,
    "@nrwl/run-commands:run-commands": True,
    "@nrwl/workspace:run-commands": True,
    "@nx/workspace:run-commands": True,
}

# The run-script executor exposes a package.json script. Nx resolves the exact
# invocation (respecting the workspace's package manager) at graph-load time
# and exposes it as `metadata.runCommand`, so Once can treat these as
# single-command run-commands tasks.
_NX_RUN_SCRIPT_EXECUTORS = {
    "nx:run-script": True,
    "@nx/run-script:run-script": True,
    "@nrwl/run-script:run-script": True,
}

def _nx_attr(ctx, name, default):
    return _configured_attr(ctx, name, default)

def _nx_env():
    return {"NX_TUI": "false", "NO_COLOR": "1", "CI": "1"}

def _nx_node(ctx):
    requested = _nx_attr(ctx, "node", "node")
    resolved = _resolve_host_executable(requested)
    if not resolved:
        fail(ctx["label"]["id"] + ": Node.js executable `" + requested + "` was not found")
    return resolved

# Prefer the workspace-local install so we run the exact nx version pinned in
# package.json. Fall back to nx on PATH so a repo without node_modules but
# with a globally installed nx still loads its graph.
def _nx_binary(ctx):
    for candidate in ["node_modules/nx/bin/nx.js", "node_modules/.bin/nx"]:
        resolved = _resolve_host_executable(candidate)
        if resolved:
            return resolved
    resolved = _resolve_host_executable("nx")
    if resolved:
        return resolved
    fail(ctx["label"]["id"] + ": nx was not found; install it in the workspace (`npm install`) or make `nx` available on PATH")

def _nx_env_with_path(env, node):
    merged = {}
    for name, value in env.items():
        merged[name] = value
    # Standard system binaries (mkdir, cp, rm, ...) live under /usr/bin or
    # /bin on macOS and Linux and are required to run `nx:run-commands` shell
    # commands that shell out to them. On Windows the corresponding path
    # segments are added.
    path_segments = []
    node_dir = _parent_dir(node)
    if node_dir:
        path_segments.append(node_dir)
    if host_os() == "windows":
        path_segments.extend(["C:\\Windows\\System32", "C:\\Windows"])
    else:
        path_segments.extend(["/usr/bin", "/bin", "/usr/sbin", "/sbin"])
    separator = ";" if host_os() == "windows" else ":"
    merged["PATH"] = separator.join(path_segments)
    return merged

# Nx's `--print` opens a browser and hangs in v19+, and `--file=/dev/stdout`
# is rejected because Nx enforces a `.json`/`.html` suffix. The reliable path
# is to point `--file` at a workspace-local JSON file, run nx once, then read
# the file back and decode it. `.nx/` is Nx's own cache directory and is
# gitignored by convention.
_NX_GRAPH_OUTPUT = ".nx/once-workspace-graph.json"

def _nx_load_graph(ctx):
    # A checked-in graph snapshot (`graph_file` attribute) skips the live
    # `nx graph` invocation entirely. Bundled starters and CI-only workspaces
    # use it to prove the target kind loads without an installed Node.js and
    # `node_modules/nx`.
    graph_file = _nx_attr(ctx, "graph_file", "")
    if graph_file:
        raw = host_file_read(workspace_root() + "/" + graph_file)
        return json_decode(raw)
    node = _nx_node(ctx)
    nx_binary = _nx_binary(ctx)
    # `--view=projects` returns every project's full `data.targets`, so no
    # `--targets` filter is needed at graph-load time. The filter, when set,
    # is applied downstream in the resolver.
    argv = [node, nx_binary, "graph", "--file=" + _NX_GRAPH_OUTPUT, "--view=projects"]
    host_command(
        argv,
        env = _nx_env_with_path(_nx_env(), node),
        cwd = workspace_root(),
    )
    raw = host_file_read(workspace_root() + "/" + _NX_GRAPH_OUTPUT)
    return json_decode(raw)

def _nx_sanitize(name):
    out = ""
    for ch in name.elems():
        if ch.isalnum() or ch == "_":
            out += ch
        else:
            out += "_"
    return out

def _nx_task_name(project, task):
    return _nx_sanitize(project) + "__" + _nx_sanitize(task)

# Nx output paths use tokens: {workspaceRoot}, {projectRoot}, {projectName},
# {options.*}. We expand the first three; {options.*} depends on runtime
# configuration and stays as-is so the action still runs, just without a
# declared output for that entry.
def _nx_expand_output(project_root, project_name, template):
    replaced = template
    replaced = replaced.replace("{workspaceRoot}/", "")
    replaced = replaced.replace("{workspaceRoot}", "")
    replaced = replaced.replace("{projectRoot}", project_root or ".")
    replaced = replaced.replace("{projectName}", project_name)
    if replaced.startswith("./"):
        replaced = replaced[2:]
    return replaced

def _nx_project_dependencies(graph_root):
    graph = graph_root.get("graph") or graph_root
    dependencies = graph.get("dependencies") or {}
    out = {}
    for source, edges in dependencies.items():
        upstream = []
        for edge in edges:
            target = edge.get("target") or ""
            if target and not target.startswith("npm:"):
                upstream.append(target)
        out[source] = _unique(upstream)
    return out

# Translate one Nx `dependsOn` entry to task labels. Supports:
#   "build"                        - same project's build
#   "^build"                       - build on each upstream project
#   {"target": "build", ...}       - explicit form; `projects` may be
#                                    "dependencies", "self", or a list of names
def _nx_dependencies_for_target(project_name, task_name, project_data, project_dependencies):
    depends_on = ((project_data.get("targets") or {}).get(task_name) or {}).get("dependsOn") or []
    out = []
    for entry in depends_on:
        if type(entry) == type(""):
            if entry.startswith("^"):
                inner = entry[1:]
                for up in project_dependencies.get(project_name) or []:
                    out.append("./" + _nx_task_name(up, inner))
            else:
                out.append("./" + _nx_task_name(project_name, entry))
        elif type(entry) == type({}):
            target = entry.get("target") or ""
            projects = entry.get("projects")
            if not target:
                continue
            if projects == None or projects == "self":
                out.append("./" + _nx_task_name(project_name, target))
            elif projects == "dependencies":
                for up in project_dependencies.get(project_name) or []:
                    out.append("./" + _nx_task_name(up, target))
            elif type(projects) == type([]):
                for project in projects:
                    if project == "self":
                        out.append("./" + _nx_task_name(project_name, target))
                    else:
                        out.append("./" + _nx_task_name(project, target))
    return _unique(out)

# Extract the shell commands from an `nx:run-commands` executor task. Nx's
# schema accepts three shapes: `command` (a single string), `commands` (a list
# of strings), and `commands` (a list of `{command, ...}` dicts). We normalize
# all three into a plain list of strings that a POSIX shell can run in order.
def _nx_run_commands_from_options(options):
    if not options:
        return []
    command = options.get("command")
    if type(command) == type("") and command:
        return [command]
    commands = options.get("commands") or []
    out = []
    for entry in commands:
        if type(entry) == type(""):
            if entry:
                out.append(entry)
        elif type(entry) == type({}):
            inner = entry.get("command") or ""
            if inner:
                out.append(inner)
    return out

def _nx_run_commands_attrs(task_config):
    options = task_config.get("options") or {}
    commands = _nx_run_commands_from_options(options)
    return {
        "commands": commands,
        "cwd": options.get("cwd") or "",
        "env": options.get("env") or {},
    }

def _nx_workspace_resolver(ctx):
    graph_root = _nx_load_graph(ctx)
    graph = graph_root.get("graph") or graph_root
    nodes = graph.get("nodes") or {}
    project_dependencies = _nx_project_dependencies(graph_root)
    task_filter = _nx_attr(ctx, "targets", _NX_DEFAULT_TASKS)
    task_filter_set = {}
    for name in task_filter:
        task_filter_set[name] = True

    targets = []
    roots = []
    for project_name, node_entry in nodes.items():
        data = node_entry.get("data") or {}
        project_root = data.get("root") or ""
        node_type = node_entry.get("type") or ""
        for task_name, task_config in (data.get("targets") or {}).items():
            if task_filter_set and not task_filter_set.get(task_name):
                continue
            deps = _nx_dependencies_for_target(project_name, task_name, data, project_dependencies)
            outputs = []
            for template in task_config.get("outputs") or []:
                expanded = _nx_expand_output(project_root, project_name, template)
                if expanded and "{" not in expanded:
                    outputs.append(expanded)
            srcs = []
            if project_root:
                srcs.append(project_root + "/**/*")
            executor = task_config.get("executor") or ""
            attrs = {
                "project": project_name,
                "task": task_name,
                "project_root": project_root,
                "outputs": outputs,
                "executor": executor,
                "project_type": node_type,
                "runnable": False,
                "commands": [],
                "command_cwd": "",
                "command_env": {},
            }
            if _NX_RUN_COMMANDS_EXECUTORS.get(executor):
                run_commands = _nx_run_commands_attrs(task_config)
                if run_commands["commands"]:
                    attrs["runnable"] = True
                    attrs["commands"] = run_commands["commands"]
                    attrs["command_cwd"] = run_commands["cwd"]
                    attrs["command_env"] = run_commands["env"]
            elif _NX_RUN_SCRIPT_EXECUTORS.get(executor):
                run_command = (task_config.get("metadata") or {}).get("runCommand") or ""
                if run_command:
                    attrs["runnable"] = True
                    attrs["commands"] = [run_command]
                    attrs["command_cwd"] = ((task_config.get("options") or {}).get("cwd")) or ""
                    attrs["command_env"] = (task_config.get("options") or {}).get("env") or {}
            targets.append({
                "name": _nx_task_name(project_name, task_name),
                "kind": "nx_task",
                "deps": deps,
                "srcs": srcs,
                "attrs": attrs,
            })
            if task_name == "build" and (node_type == "app" or node_type == "e2e"):
                roots.append(_nx_task_name(project_name, task_name))

    return {
        "targets": targets,
        "roots": _unique(roots),
    }

def _nx_workspace_impl(ctx):
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "nx_workspace",
        "nx_workspace": True,
        "targets": ctx["deps"],
    }

def _nx_shell_argv(commands):
    joined = ""
    for index in range(len(commands)):
        if index > 0:
            joined += " && "
        joined += "( " + commands[index] + " )"
    if host_os() == "windows":
        return ["cmd.exe", "/d", "/s", "/c", joined]
    return ["/bin/sh", "-c", joined]

def _nx_task_impl(ctx):
    project = _nx_attr(ctx, "project", "")
    task = _nx_attr(ctx, "task", "")
    if not project or not task:
        fail(ctx["label"]["id"] + ": nx_task requires `project` and `task` attributes")

    outputs = _nx_attr(ctx, "outputs", [])
    executor = _nx_attr(ctx, "executor", "")
    runnable = _nx_attr(ctx, "runnable", False)
    commands = _nx_attr(ctx, "commands", [])

    provider = {
        "label_id": ctx["label"]["id"],
        "target_kind": "nx_task",
        "nx_project": project,
        "nx_task_name": task,
        "executor": executor,
        "runnable": runnable,
        "outputs": outputs,
    }
    if ctx["capability"] != "build":
        return provider

    if not runnable or not commands:
        # Once owns scheduling, caching, and remote execution for tasks it can
        # run directly. Executors whose behavior is defined by an Nx plugin do
        # not surface a resolved command in `nx graph`, so Once cannot run them
        # itself yet. Register the target as informational and stop before
        # emitting an action so downstream deps still see the provider.
        fail(ctx["label"]["id"] + ": nx executor `" + executor + "` is not yet supported by Once. Add an `nx_task_kind` override or run it through `nx run " + project + ":" + task + "` for now.")

    project_source_globs = []
    project_root = _nx_attr(ctx, "project_root", "")
    if project_root:
        project_source_globs.append(project_root + "/**/*")

    inputs = _unique(
        _file_globs(project_source_globs) +
        _file_globs(_nx_attr(ctx, "config", ["nx.json", "package.json", "pnpm-lock.yaml", "yarn.lock", "package-lock.json"])) +
        _file_globs(_nx_attr(ctx, "dependencies", ["node_modules/**/*"])),
    )

    node = _nx_node(ctx)
    env = _nx_env_with_path(_nx_env(), node)
    for name in _nx_attr(ctx, "env_inherit", []):
        value = host_env(name)
        if value:
            env[name] = value
    for name, value in _nx_attr(ctx, "command_env", {}).items():
        env[name] = value
    for name, value in _nx_attr(ctx, "env", {}).items():
        env[name] = value

    # `run_action` requires a workspace-relative cwd. `nx:run-commands`
    # commands run from the workspace root when `cwd` is unset, and from
    # `options.cwd` (which is already workspace-relative) otherwise.
    command_cwd = _nx_attr(ctx, "command_cwd", "")
    cwd = command_cwd if command_cwd else "."

    node_version = host_command([node, "--version"]).strip()

    run_action(
        argv = _nx_shell_argv(commands),
        inputs = inputs,
        outputs = outputs,
        env = env,
        cwd = cwd,
        toolchain_identity = "once.nx.run-commands.v1\x00" + node + "\x00" + node_version,
        identifier = ctx["label"]["id"] + ":nx-run-commands",
    )
    return provider

nx_workspace = target_kind(
    docs = "Native Nx workspace seed. Its resolver runs `nx graph --print` once during graph load, translates every project and task into a first-class Once target, and hands scheduling, caching, and remote execution to Once instead of `nx run`.",
    attrs = [
        attr("node", "string", default = "\"node\"", docs = "Node.js executable name, absolute path, or workspace-relative path.", configurable = False),
        attr("targets", "list<string>", default = "[\"build\", \"test\", \"lint\"]", docs = "Nx task names to emit. Defaults to build, test, and lint. Set an empty list to include every task in the graph.", configurable = False),
        attr("resolver_inputs", "list<string>", default = "[]", docs = "Package-relative text globs supplied to native integration resolution. Defaults to srcs when empty.", configurable = False),
        attr("graph_file", "string", default = "\"\"", docs = "Optional workspace-relative path to a checked-in `nx graph --view=projects` JSON snapshot. When set, Once reads it directly instead of running `nx graph`, which lets a workspace load without an installed Node.js and `node_modules/nx`.", configurable = False),
    ],
    resolver = _nx_workspace_resolver,
    deps = [dep("deps", ["nx_task"], "Nx tasks emitted by native integration discovery.")],
    providers = ["nx_workspace"],
    capabilities = [capability("build", [])],
    tools = [_NX_NODE_TOOL, _NX_TOOL],
    examples = [
        example(
            "nx-workspace-native-project",
            name = "Nx native integration seed",
            use_when = "Use this when an Nx workspace should derive first-party build, test, and lint targets from nx.json without redeclaring them in Once.",
        ),
    ],
    impl = _nx_workspace_impl,
)

nx_task = target_kind(
    docs = "One Nx task. Once hashes its declared inputs, schedules it against the resolved graph, and runs the task's underlying command directly. Only `nx:run-commands` executors are run natively today; other executors surface as provider-only targets until Once learns their contract.",
    attrs = [
        attr("project", "string", required = True, docs = "Nx project name as it appears in the project graph.", configurable = False),
        attr("task", "string", required = True, docs = "Nx target name for this project, such as `build`, `test`, or `lint`.", configurable = False),
        attr("project_root", "string", default = "\"\"", docs = "Workspace-relative directory that holds the project's sources.", configurable = False),
        attr("outputs", "list<string>", default = "[]", docs = "Workspace-relative output paths declared by the Nx target with `{projectRoot}`, `{workspaceRoot}`, and `{projectName}` tokens already expanded.", configurable = False),
        attr("executor", "string", default = "\"\"", docs = "Nx executor id, kept for provenance so cache keys segregate between executors.", configurable = False),
        attr("project_type", "string", default = "\"\"", docs = "Nx project type reported by the graph: `app`, `lib`, or `e2e`.", configurable = False),
        attr("runnable", "bool", default = "false", docs = "True when the resolver was able to lower the task to a concrete command Once can run directly. False for plugin executors that require Nx to expand.", configurable = False),
        attr("commands", "list<string>", default = "[]", docs = "Resolver-owned shell commands lowered from the Nx `run-commands` executor options.", configurable = False),
        attr("command_cwd", "string", default = "\"\"", docs = "Workspace-relative working directory for the lowered command, mirroring the executor's `cwd` option.", configurable = False),
        attr("command_env", "map<string, string>", default = "{}", docs = "Environment variables from the executor's `env` option applied before user-supplied `env`.", configurable = False),
        attr("node", "string", default = "\"node\"", docs = "Node.js executable used to identify the toolchain in the cache key.", configurable = False),
        attr("dependencies", "list<string>", default = "[\"node_modules/**/*\"]", docs = "Installed package files required at execution time.", configurable = False),
        attr("config", "list<string>", default = "[\"nx.json\", \"package.json\", \"pnpm-lock.yaml\", \"yarn.lock\", \"package-lock.json\"]", docs = "Workspace-level configuration inputs.", configurable = False),
        attr("env", "map<string, string>", default = "{}", docs = "User-supplied environment variables applied last.", configurable = True),
        attr("env_inherit", "list<string>", default = "[]", docs = "Host environment variables inherited by name.", configurable = False),
    ],
    deps = [dep("deps", ["nx_task"], "Upstream Nx tasks whose outputs this task depends on.")],
    providers = ["nx_task"],
    capabilities = [capability("build", [])],
    tools = [_NX_NODE_TOOL, _NX_TOOL],
    examples = [
        example(
            "nx-task",
            name = "Nx task",
            use_when = "Use this to schedule one Nx task through Once when the workspace is not fully covered by the `nx` native project.",
        ),
    ],
    impl = _nx_task_impl,
)

nx = native_project(
    target_kind = "nx_workspace",
    docs = "Recognizes a native Nx workspace from nx.json and emits `nx_task` targets for each project's build, test, and lint tasks.",
    markers = ["nx.json"],
    target_name = "nx",
    inputs = ["package.json", "pnpm-lock.yaml", "yarn.lock", "package-lock.json", "**/project.json", "**/package.json"],
    exclude = _native_project_generated_dirs() + ["node_modules", "dist", ".nx", "tmp"],
    input_exclude = ["node_modules", ".nx", ".git", "dist", "tmp"],
    on_match = "stop",
    requires_tools = ["node"],
)
