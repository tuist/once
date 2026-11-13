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

def _nx_env_with_path(env, node, cwd = None):
    merged = {}
    for name, value in env.items():
        merged[name] = value
    # Standard system binaries (mkdir, cp, rm, ...) live under /usr/bin or
    # /bin on macOS and Linux and are required to run `nx:run-commands` shell
    # commands that shell out to them. On Windows the corresponding path
    # segments are added.
    path_segments = []
    if cwd != None:
        parts = [part for part in cwd.split("/") if part and part != "."]
        for index in range(len(parts), -1, -1):
            directory = "/".join(parts[:index])
            path_segments.append(execution_path((directory + "/" if directory else "") + "node_modules/.bin"))
    node_dir = _parent_dir(node)
    if node_dir:
        path_segments.append(node_dir)
    if host_os() == "windows":
        path_segments.extend(["C:\\Windows\\System32", "C:\\Windows"])
    else:
        path_segments.extend(["/usr/bin", "/bin", "/usr/sbin", "/sbin"])
    separator = ";" if host_os() == "windows" else ":"
    host_path = host_env("PATH")
    if host_path:
        path_segments.append(host_path)
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

def _nx_expand_output(project_root, project_name, template, options = {}):
    replaced = template
    for token in template.split("{")[1:]:
        key = token.split("}")[0]
        if key.startswith("options."):
            value = options
            for part in key.removeprefix("options.").split("."):
                value = value.get(part) if type(value) == type({}) else None
            if type(value) == type(""):
                replaced = replaced.replace("{" + key + "}", value)
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

def _nx_upstream_tasks(project_name, task_name, nodes, project_dependencies):
    pending = project_dependencies.get(project_name) or []
    seen = {project_name: True}
    out = []
    for _ in range(len(nodes)):
        if not pending:
            break
        next_projects = []
        for project in pending:
            if project in seen:
                continue
            seen[project] = True
            data = (nodes.get(project) or {}).get("data") or {}
            if task_name in (data.get("targets") or {}):
                out.append("./" + _nx_task_name(project, task_name))
            else:
                next_projects.extend(project_dependencies.get(project) or [])
        pending = next_projects
    return out

# Translate one Nx `dependsOn` entry to task labels. Supports:
#   "build"                        - same project's build
#   "^build"                       - build on each upstream project
#   {"target": "build", ...}       - explicit form; `projects` may be
#                                    "dependencies", "self", or a list of names
def _nx_dependencies_for_target(project_name, task_name, project_data, project_dependencies, nodes):
    depends_on = ((project_data.get("targets") or {}).get(task_name) or {}).get("dependsOn") or []
    out = []
    for entry in depends_on:
        if type(entry) == type(""):
            if entry.startswith("^"):
                inner = entry[1:]
                out.extend(_nx_upstream_tasks(project_name, inner, nodes, project_dependencies))
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
                out.extend(_nx_upstream_tasks(project_name, target, nodes, project_dependencies))
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
    if type(command) == type([]):
        command = " ".join(command)
    commands = [{"command": command}] if command else (options.get("commands") or [])
    args = options.get("args") or ""
    if type(args) == type([]):
        args = " ".join(args)
    out = []
    for entry in commands:
        forward = options.get("forwardAllArgs", True)
        inner = ""
        if type(entry) == type(""):
            inner = entry
        elif type(entry) == type({}):
            inner = entry.get("command") or ""
            forward = entry.get("forwardAllArgs", forward)
        if inner:
            out.append(inner + (" " + args if args and forward else ""))
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
    workspace_outputs = []
    for project_name, node_entry in nodes.items():
        data = node_entry.get("data") or {}
        project_root = data.get("root") or ""
        node_type = node_entry.get("type") or ""
        for task_name, task_config in (data.get("targets") or {}).items():
            deps = _nx_dependencies_for_target(project_name, task_name, data, project_dependencies, nodes)
            outputs = []
            options = dict(task_config.get("options") or {})
            options.update((task_config.get("configurations") or {}).get(task_config.get("defaultConfiguration")) or {})
            for template in task_config.get("outputs") or []:
                expanded = _nx_expand_output(project_root, project_name, template, options)
                if expanded and "{" not in expanded:
                    outputs.append(expanded)
            workspace_outputs.extend(outputs)
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
                "executor_graph": "",
                "env_inherit": [entry["env"] for entry in task_config.get("inputs") or [] if type(entry) == type({}) and entry.get("env")],
            }
            if _NX_RUN_COMMANDS_EXECUTORS.get(executor):
                run_commands = _nx_run_commands_attrs({"options": options})
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
                    attrs["command_cwd"] = project_root
                    attrs["command_env"] = (task_config.get("options") or {}).get("env") or {}
            elif executor:
                attrs["runnable"] = True
                attrs["executor_graph"] = _json_encode(graph)
            targets.append({
                "name": _nx_task_name(project_name, task_name),
                "kind": "nx_test" if task_name == "test" else "nx_task",
                "deps": deps,
                "srcs": srcs,
                "attrs": attrs,
            })
            if task_name == "build" and (node_type == "app" or node_type == "e2e"):
                roots.append(_nx_task_name(project_name, task_name))

    by_name = {target["name"]: target for target in targets}
    for target in targets:
        target["attrs"]["workspace_outputs"] = sorted(_unique(workspace_outputs))
        target["deps"] = [dependency for dependency in target["deps"] if dependency.removeprefix("./") in by_name]
    if task_filter_set:
        selected = {target["name"]: True for target in targets if task_filter_set.get(target["attrs"]["task"])}
        pending = selected.keys()
        for _ in range(len(targets)):
            if not pending:
                break
            dependencies = []
            for name in pending:
                for dependency in by_name[name]["deps"]:
                    dependency_name = dependency.removeprefix("./")
                    if dependency_name in by_name and dependency_name not in selected:
                        selected[dependency_name] = True
                        dependencies.append(dependency_name)
            pending = dependencies
        targets = [target for target in targets if target["name"] in selected]
        roots = [name for name in roots if name in selected]

    return {
        "targets": targets,
        "roots": _unique(roots),
        "attrs": {"_default_test_roots": [target["name"] for target in targets if target["attrs"]["task"] == "test"]},
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
        return [host_which("cmd.exe"), "/d", "/s", "/c", joined]
    return [host_which("sh"), "-c", joined]

_NX_SNAPSHOT_OUTPUTS = """
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const spec = JSON.parse(process.argv[1]);
const root = process.cwd();
async function main() {
let status = 1;
const started = performance.now();
try {
  if (spec.graph) {
    const { runExecutor, readJsonFile } = require('@nx/devkit');
    const graph = JSON.parse(fs.readFileSync(spec.graph, 'utf8'));
    const projects = Object.fromEntries(Object.entries(graph.nodes).map(([name, node]) => [name, node.data]));
    const context = {
      root, cwd: root, projectGraph: graph,
      projectsConfigurations: { version: 2, projects },
      nxJsonConfiguration: readJsonFile('nx.json'),
      isVerbose: false,
    };
    let success = false;
    for await (const result of await runExecutor({ project: spec.project, target: spec.task }, {}, context)) {
      success = result.success === true;
      if (!success) break;
    }
    status = success ? 0 : 1;
  } else {
    const result = spawnSync(spec.argv[0], spec.argv.slice(1), { cwd: spec.cwd, stdio: 'inherit' });
    if (result.error) throw result.error;
    if (result.signal) process.kill(process.pid, result.signal);
    status = result.status ?? 1;
  }
} catch (error) {
  console.error(error);
}
if (spec.results) {
  const state = status === 0 ? 'passed' : 'failed';
  fs.writeFileSync(spec.results, JSON.stringify({
    schema: 'once.test_results.v1', target: spec.target,
    runner: { type: 'nx', metadata: { unit: 'suite' } }, status: state,
    summary: { total: 1, passed: status === 0 ? 1 : 0, failed: status === 0 ? 0 : 1, skipped: 0, flaky: 0 },
    cases: [{ id: spec.target + '::suite', name: spec.project + ':' + spec.task, suite: spec.target,
      status: state, attempts: [{ status: state, duration_ms: Math.round(performance.now() - started) }],
      runner_metadata: { unit: 'suite' } }],
    artifacts: { logs: [spec.log], native_results: [] },
  }));
}
if (status !== 0) { process.exitCode = status; return; }
if (!spec.snapshot) return;
const patterns = spec.outputs;
const expanded = patterns.some(p => /[*?{}[\\]!]/.test(p))
  ? require('nx/src/native').expandOutputs(root, patterns)
  : patterns.filter(p => fs.existsSync(p));
const entries = [];
for (const output of [...new Set(expanded)].sort()) {
  const source = path.resolve(output);
  const relative = path.relative(root, source);
  if (!relative || relative === '..' || relative.startsWith('..' + path.sep) || path.isAbsolute(relative)
      || relative.split(path.sep)[0] === '.once') {
    throw new Error('Nx output must be a scoped workspace path: ' + output);
  }
  const index = entries.length;
  const kind = fs.lstatSync(source).isDirectory() ? 'tree' : 'file';
  fs.mkdirSync(path.join(spec.snapshot, 'files'), { recursive: true });
  fs.cpSync(source, path.join(spec.snapshot, 'files', String(index)), { recursive: true, verbatimSymlinks: true });
  entries.push({ path: relative.split(path.sep).join('/'), kind });
}
fs.mkdirSync(spec.snapshot, { recursive: true });
fs.writeFileSync(path.join(spec.snapshot, 'manifest.json'), JSON.stringify(entries));
}
main().catch(error => { console.error(error); process.exitCode = 1; });
"""

def nx_restore_outputs(ctx):
    snapshot = ctx["args"]["snapshot"]
    entries = json_decode(host_file_read(workspace_root() + "/" + snapshot + "/manifest.json"))
    for index in range(len(entries)):
        entry = entries[index]
        source = snapshot + "/files/" + str(index)
        copy_path(
            source,
            entry["path"],
            kind = entry["kind"],
            inputs = [source],
            identifier = "nx-restore-" + str(index),
        )

def _nx_task_impl(ctx, is_test = False):
    project = _nx_attr(ctx, "project", "")
    task = _nx_attr(ctx, "task", "")
    if not project or not task:
        fail(ctx["label"]["id"] + ": nx_task requires `project` and `task` attributes")

    outputs = _nx_attr(ctx, "outputs", [])
    executor = _nx_attr(ctx, "executor", "")
    runnable = _nx_attr(ctx, "runnable", False)
    commands = _nx_attr(ctx, "commands", [])
    executor_graph = _nx_attr(ctx, "executor_graph", "")

    provider = {
        "label_id": ctx["label"]["id"],
        "target_kind": "nx_test" if is_test else "nx_task",
        "nx_project": project,
        "nx_task_name": task,
        "executor": executor,
        "runnable": runnable,
        "outputs": outputs,
    }
    if ctx["capability"] != "build" and not is_test:
        return provider

    if not runnable or (not commands and not executor_graph):
        fail(ctx["label"]["id"] + ": nx executor `" + executor + "` has no resolved commands or executor graph")

    all_outputs = _nx_attr(ctx, "workspace_outputs", []) or outputs
    generated = [output for output in all_outputs if not output.startswith("!")]
    output_patterns = generated + [output + "/**/*" for output in generated]
    inputs = _unique(
        glob(["**/*"], exclude = output_patterns + ["node_modules/**/*", ".git/**/*", ".nx/**/*", ".once/**/*", "dist/**/*", "tmp/**/*"]) +
        _file_globs([output[1:] for output in all_outputs if output.startswith("!")]) +
        _file_globs(_nx_attr(ctx, "config", ["nx.json", "package.json", "pnpm-lock.yaml", "yarn.lock", "package-lock.json"])) +
        _file_globs(_nx_attr(ctx, "dependencies", ["node_modules/**/*"])),
    )
    inputs = [path for path in inputs if "/node_modules/.vite/" not in "/" + path + "/"]
    dependency_artifacts = _unique([path for dep in ctx["deps"] for path in dep.get("artifact_inputs", [])])
    inputs = _unique(inputs + dependency_artifacts)

    command_cwd = _nx_attr(ctx, "command_cwd", "")
    cwd = command_cwd if command_cwd else "."
    node = _nx_node(ctx)
    env = _nx_env_with_path(_nx_env(), node, cwd)
    home = host_env("HOME")
    if home:
        env["HOME"] = home
    for name in _nx_attr(ctx, "env_inherit", []):
        value = host_env(name)
        if value:
            env[name] = value
    for name, value in _nx_attr(ctx, "command_env", {}).items():
        env[name] = value
    for name, value in _nx_attr(ctx, "env", {}).items():
        env[name] = value

    node_version = host_command([node, "--version"]).strip()

    argv = _nx_shell_argv(commands) if commands else []
    action_outputs = outputs
    snapshot = None
    if outputs:
        snapshot = declare_output("nx-outputs")
        action_outputs = [snapshot]
    provider["artifact_inputs"] = dependency_artifacts + ([snapshot] if snapshot else [])
    spec = {
        "argv": argv, "cwd": cwd, "outputs": outputs, "snapshot": snapshot,
        "graph": declare_output("nx-graph.json") if executor_graph else "", "project": project, "task": task,
    }
    log = ""
    test_dir = None
    if is_test:
        test_dir = _test_output_dir(ctx)
        results = test_dir + "/test_results.json"
        log = test_dir + "/nx.log"
        spec.update({"results": results, "log": log, "target": ctx["label"]["id"]})
        argv = [node, "-e", _NX_SNAPSHOT_OUTPUTS, _json_encode(spec)]
        action_outputs = action_outputs + [results, log]
        provider["affected_inputs"] = inputs
        provider["test_discovery_inputs"] = inputs
        provider["test_info"] = {
            "schema": "once.test_info.v1", "target": ctx["label"]["id"],
            "runner": {"type": "nx", "display_name": "Nx", "metadata": {"unit": "suite"}},
            "command": {"argv": argv, "env": env, "cwd": "."},
            "outputs": {"results": results, "logs": [log], "native_results": [], "coverage": []},
            "listing": {"supported": False, "strategy": "unsupported"},
            "filtering": {"case_filtering": "unsupported"},
            "sharding": {"supported": False, "granularity": "target"},
            "retries": {"supported": False, "default_attempts": 1},
            "execution": {"cacheable": True, "run_from_workspace_root": True},
            "labels": [], "metadata": {},
        }
        if ctx["capability"] not in ["build", "test"]:
            return provider
    argv = [node, "-e", _NX_SNAPSHOT_OUTPUTS, _json_encode(spec)]
    if executor_graph:
        write_path(spec["graph"], executor_graph)
        inputs = inputs + [spec["graph"]]
    redirects = {"stdout": log, "stderr": log} if log else {}

    run_action(
        display_name = "Run Nx task · " + project + ":" + task,
        presentation = _action_metadata(context = [_action_context("nx.task", project + ":" + task)]),
        source_files = _action_source_files(inputs),
        argv = argv,
        inputs = inputs,
        outputs = action_outputs,
        clean_paths = ([snapshot] if snapshot else []) + ([results, log] if test_dir else []),
        create_dirs = [test_dir] if test_dir else [],
        env = env,
        cwd = ".",
        toolchain_identity = "once.nx.task.v3\x00" + executor + "\x00" + node + "\x00" + node_version,
        identifier = ctx["label"]["id"] + ":nx-run-commands",
        **redirects
    )
    if snapshot:
        expand_actions(
            implementation = "nx_restore_outputs",
            inputs = [snapshot + "/manifest.json"],
            outputs = [],
            args = {"snapshot": snapshot},
        )
    return provider

def _nx_test_impl(ctx):
    return _nx_task_impl(ctx, is_test = True)

nx_workspace = target_kind(
    docs = "Native Nx workspace seed. Its resolver runs `nx graph --print` once during graph load, translates every project and task into a first-class Once target, and hands scheduling, caching, and remote execution to Once instead of `nx run`.",
    attrs = [
        attr("node", "string", default = "\"node\"", docs = "Node.js executable name, absolute path, or workspace-relative path.", configurable = False),
        attr("targets", "list<string>", default = "[\"build\", \"test\", \"lint\"]", docs = "Nx task names to emit, together with their transitive task dependencies. Defaults to build, test, and lint. Set an empty list to include every task in the graph.", configurable = False),
        attr("resolver_inputs", "list<string>", default = "[]", docs = "Package-relative text globs supplied to native integration resolution. Defaults to srcs when empty.", configurable = False),
        attr("graph_file", "string", default = "\"\"", docs = "Optional workspace-relative path to a checked-in `nx graph --view=projects` JSON snapshot. When set, Once reads it directly instead of running `nx graph`, which lets a workspace load without an installed Node.js and `node_modules/nx`.", configurable = False),
        attr("_default_test_roots", "list<string>", default = "[]", docs = "Resolver-owned first-party test target names, including library suites, for targetless test selection.", configurable = False),
    ],
    resolver = _nx_workspace_resolver,
    deps = [dep("deps", ["nx_task", "nx_test"], "Nx tasks emitted by native integration discovery.")],
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

def _nx_task_attrs():
    return [
        attr("project", "string", required = True, docs = "Nx project name as it appears in the project graph.", configurable = False),
        attr("task", "string", required = True, docs = "Nx target name for this project, such as `build`, `test`, or `lint`.", configurable = False),
        attr("project_root", "string", default = "\"\"", docs = "Workspace-relative directory that holds the project's sources.", configurable = False),
        attr("outputs", "list<string>", default = "[]", docs = "Workspace-relative Nx output paths or patterns with `{projectRoot}`, `{workspaceRoot}`, and `{projectName}` expanded. Only existing matches are cached; absent outputs are valid.", configurable = False),
        attr("executor", "string", default = "\"\"", docs = "Nx executor id, kept for provenance so cache keys segregate between executors.", configurable = False),
        attr("executor_graph", "string", default = "\"\"", docs = "Resolver-owned canonical project graph used to invoke a plugin executor directly, without Nx scheduling or caching.", configurable = False),
        attr("workspace_outputs", "list<string>", default = "[]", docs = "Resolver-owned workspace output patterns excluded from source hashing.", configurable = False),
        attr("project_type", "string", default = "\"\"", docs = "Nx project type reported by the graph: `app`, `lib`, or `e2e`.", configurable = False),
        attr("runnable", "bool", default = "false", docs = "True when the resolver supplied commands or a plugin executor graph.", configurable = False),
        attr("commands", "list<string>", default = "[]", docs = "Resolver-owned shell commands lowered from the Nx `run-commands` executor options.", configurable = False),
        attr("command_cwd", "string", default = "\"\"", docs = "Workspace-relative working directory for the lowered command, mirroring the executor's `cwd` option.", configurable = False),
        attr("command_env", "map<string, string>", default = "{}", docs = "Environment variables from the executor's `env` option applied before user-supplied `env`.", configurable = False),
        attr("node", "string", default = "\"node\"", docs = "Node.js executable used to identify the toolchain in the cache key.", configurable = False),
        attr("dependencies", "list<string>", default = "[\"node_modules/**/*\"]", docs = "Installed package files required at execution time.", configurable = False),
        attr("config", "list<string>", default = "[\"nx.json\", \"package.json\", \"pnpm-lock.yaml\", \"yarn.lock\", \"package-lock.json\"]", docs = "Workspace-level configuration inputs.", configurable = False),
        attr("env", "map<string, string>", default = "{}", docs = "User-supplied environment variables applied last.", configurable = True),
        attr("env_inherit", "list<string>", default = "[]", docs = "Host environment variables inherited by name.", configurable = False),
    ]

nx_task = target_kind(
    docs = "One Nx task. Once schedules and caches resolved commands or invokes a plugin through Nx's executor API without Nx scheduling or caching.",
    attrs = _nx_task_attrs(),
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

nx_test = target_kind(
    docs = "One Nx test task, executed as a cacheable target-level suite with normalized results and logs. Uses the same resolved commands and plugin executor API as nx_task.",
    attrs = _nx_task_attrs(),
    deps = [dep("deps", ["nx_task"], "Prerequisite Nx tasks built before the test suite.")],
    providers = ["nx_task", "once_test_info"],
    capabilities = [capability("build", []), capability("test", ["default", "test_results", "logs"])],
    tools = [_NX_NODE_TOOL, _NX_TOOL],
    examples = [example("nx-test", name = "Nx test suite", use_when = "Run one Nx test command as a cacheable suite with normalized results.")],
    impl = _nx_test_impl,
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
