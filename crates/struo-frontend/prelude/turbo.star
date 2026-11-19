_TURBO_NODE_TOOL = tool("node", executables = ["node"])
_TURBO_TOOL = tool("turbo", executables = ["turbo"])

_TURBO_DEFAULT_TASKS = ["build", "test", "lint"]

_TURBO_ROOT_PACKAGE = "//"

_TURBO_CONFIG = [
    "package.json",
    "turbo.json",
    "turbo.jsonc",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "yarn.lock",
    ".yarnrc.yml",
    "bun.lock",
    "bun.lockb",
    ".npmrc",
    "tsconfig*.json",
]

_TURBO_IGNORED_DIRS = ["node_modules", ".git", ".struo", ".turbo"]

def _turbo_attr(ctx, name, default):
    return _configured_attr(ctx, name, default)

def _turbo_node(ctx):
    requested = _turbo_attr(ctx, "node", "node")
    resolved = _resolve_host_executable(requested)
    if not resolved:
        fail(ctx["label"]["id"] + ": Node.js executable `" + requested + "` was not found")
    return resolved

# Prefer the workspace-local install so the graph comes from the turbo version
# pinned in package.json, then fall back to a turbo binary on PATH.
def _turbo_command(ctx, node):
    local = _resolve_host_executable("node_modules/turbo/bin/turbo")
    if local:
        return [node, local]
    resolved = _resolve_host_executable("turbo")
    if resolved:
        return [resolved]
    fail(ctx["label"]["id"] + ": turbo was not found; install it in the workspace (`npm install --save-dev turbo`) or make `turbo` available on PATH")

def _turbo_env(node, cwd, host_path = False):
    return {"PATH": _javascript_task_path(node, cwd, host_path), "NO_COLOR": "1", "CI": "1"}

# Asks turbo for its package graph and the resolved task graph of the
# requested tasks. `turbo run` rejects task names that no package defines, so
# the requested names are first intersected with the tasks `turbo query`
# reports. The dry run keeps only the fields the resolver reads, because turbo
# lists every hashed file of every task.
_TURBO_GRAPH = """
const { spawnSync } = require('node:child_process');
const [turbo, requested] = JSON.parse(process.argv[1]);
function run(args) {
  const result = spawnSync(turbo[0], [...turbo.slice(1), ...args], {
    encoding: 'utf8', maxBuffer: 1 << 30, stdio: ['ignore', 'pipe', 'pipe'],
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    process.stderr.write(result.stderr);
    throw new Error('`turbo ' + args[0] + '` exited with status ' + result.status);
  }
  const start = result.stdout.search(/^\\{/m);
  if (start < 0) throw new Error('`turbo ' + args[0] + '` printed no JSON: ' + result.stdout);
  return JSON.parse(result.stdout.slice(start));
}
const query = run(['query', 'query { packages { items { name path tasks { items { name } } allDependencies { items { name } } } } }']);
if (query.errors && query.errors.length) {
  throw new Error('turbo query failed: ' + JSON.stringify(query.errors) + '; Struo requires Turborepo 2');
}
const packages = query.data.packages.items;
const defined = new Set(packages.flatMap(item => item.tasks.items.map(task => task.name)));
const tasks = requested.length ? requested.filter(task => defined.has(task)) : [...defined].sort();
let dry = { tasks: [], globalCacheInputs: {} };
if (tasks.length) {
  dry = run(['run', ...tasks, '--dry=json', '--cache=local:,remote:', '--output-logs=none']);
}
process.stdout.write(JSON.stringify({
  packages,
  dry: {
    turboVersion: dry.turboVersion,
    globalCacheInputs: {
      files: dry.globalCacheInputs.files || {},
      environmentVariables: dry.globalCacheInputs.environmentVariables || {},
    },
    tasks: dry.tasks.map(task => ({
      taskId: task.taskId, task: task.task, package: task.package, command: task.command,
      directory: task.directory, dependencies: task.dependencies,
      resolvedTaskDefinition: task.resolvedTaskDefinition,
      environmentVariables: task.environmentVariables,
    })),
  },
}));
"""

def _turbo_load_graph(ctx):
    # A checked-in snapshot skips the live turbo invocation, so bundled starters
    # and CI-only workspaces load without Node.js or an installed turbo.
    graph_file = _turbo_attr(ctx, "graph_file", "")
    if graph_file:
        return json_decode(host_file_read(workspace_root() + "/" + graph_file))
    node = _turbo_node(ctx)
    turbo = _turbo_command(ctx, node)
    requested = _turbo_attr(ctx, "targets", _TURBO_DEFAULT_TASKS)
    # Turbo expands wildcard environment patterns against the caller's
    # environment, so the derived graph depends on it.
    host_environment_sha256()
    env = _turbo_env(node, ".", host_path = True)
    env["TURBO_TELEMETRY_DISABLED"] = "1"
    env["TURBO_NO_UPDATE_NOTIFIER"] = "1"
    raw = host_command(
        [node, "-e", _TURBO_GRAPH, _json_encode([turbo, requested])],
        env = env,
        cwd = workspace_root(),
        max_output_bytes = 256 * 1024 * 1024,
    )
    return json_decode(raw)

def _turbo_sanitize(name):
    out = ""
    for ch in name.elems():
        if ch.isalnum() or ch == "_":
            out += ch
        else:
            out += "_"
    return out

def _turbo_target_name(package, task):
    return ("root" if package == _TURBO_ROOT_PACKAGE else _turbo_sanitize(package)) + "__" + _turbo_sanitize(task)

def _turbo_package_dir(directory):
    return "" if directory in ["", "."] else directory.removesuffix("/")

def _turbo_workspace_path(package_dir, path):
    if path.startswith("$TURBO_ROOT$/"):
        return path.removeprefix("$TURBO_ROOT$/")
    path = path.removeprefix("./")
    return package_dir + "/" + path if package_dir else path

def _turbo_output(package_dir, pattern):
    if pattern.startswith("!"):
        return "!" + _turbo_workspace_path(package_dir, pattern[1:])
    return _turbo_workspace_path(package_dir, pattern)

# Turbo reports every variable name it hashes or passes through: exact names
# from `env` and `passThroughEnv`, plus the set variables matched by wildcard
# patterns and framework inference as `NAME=hash` entries.
def _turbo_env_names(variables):
    variables = variables or {}
    specified = variables.get("specified") or {}
    names = []
    for name in (specified.get("env") or []) + (specified.get("passThroughEnv") or []):
        if name and "*" not in name and not name.startswith("!"):
            names.append(name)
    for key in ["configured", "inferred", "passthrough"]:
        for entry in variables.get(key) or []:
            name = entry.split("=")[0]
            if name:
                names.append(name)
    return names

def _turbo_runnable(task):
    definition = task.get("resolvedTaskDefinition") or {}
    command = task.get("command") or ""
    return command and command != "<NONEXISTENT>" and not definition.get("persistent") and not definition.get("interactive")

# Tasks without a script, such as transit nodes, are not targets. Their
# dependents depend on the nearest runnable tasks behind them instead.
def _turbo_runnable_dependencies(task, by_id):
    out = []
    seen = {}
    pending = task.get("dependencies") or []
    for _ in range(len(by_id) + 1):
        if not pending:
            break
        next_pending = []
        for task_id in pending:
            if task_id in seen or task_id not in by_id:
                continue
            seen[task_id] = True
            dependency = by_id[task_id]
            if _turbo_runnable(dependency):
                out.append("./" + _turbo_target_name(dependency["package"], dependency["task"]))
            elif not (dependency.get("resolvedTaskDefinition") or {}).get("persistent"):
                next_pending.extend(dependency.get("dependencies") or [])
        pending = next_pending
    return _unique(out)

def _turbo_workspace_resolver(ctx):
    graph = _turbo_load_graph(ctx)
    package_dirs = {}
    package_dependencies = {}
    for package in graph.get("packages") or []:
        name = package.get("name") or ""
        package_dirs[name] = _turbo_package_dir(package.get("path") or "")
        package_dependencies[name] = [
            item["name"]
            for item in ((package.get("allDependencies") or {}).get("items") or [])
            if item.get("name") and item["name"] != _TURBO_ROOT_PACKAGE and item["name"] != name
        ]
    dry = graph.get("dry") or {}
    global_inputs = dry.get("globalCacheInputs") or {}
    global_env = _turbo_env_names(global_inputs.get("environmentVariables"))
    global_files = sorted((global_inputs.get("files") or {}).keys())
    tasks = dry.get("tasks") or []
    by_id = {task["taskId"]: task for task in tasks}

    targets = []
    workspace_outputs = []
    for task in tasks:
        if not _turbo_runnable(task):
            continue
        package = task["package"]
        definition = task.get("resolvedTaskDefinition") or {}
        package_dir = _turbo_package_dir(task.get("directory") or "")
        outputs = [_turbo_output(package_dir, pattern) for pattern in definition.get("outputs") or []]
        workspace_outputs.extend([output for output in outputs if not output.startswith("!")])
        if package == _TURBO_ROOT_PACKAGE:
            source_dirs = [""]
            other_packages = []
        else:
            linked = [package] + (package_dependencies.get(package) or [])
            source_dirs = _unique([package_dir] + [package_dirs[name] for name in linked if package_dirs.get(name)])
            other_packages = sorted([name for name, directory in package_dirs.items() if directory and name not in linked])
        targets.append({
            "name": _turbo_target_name(package, task["task"]),
            "kind": "turbo_test" if task["task"] == "test" else "turbo_task",
            "deps": _turbo_runnable_dependencies(task, by_id),
            "srcs": [(package_dir + "/**/*") if package_dir else "*"],
            "attrs": {
                "package": package,
                "task": task["task"],
                "package_dir": package_dir,
                "command": task["command"],
                "outputs": outputs,
                "cache": definition.get("cache", True) != False,
                "source_dirs": source_dirs,
                "other_packages": other_packages,
                "global_inputs": global_files,
                "env_inherit": _unique(global_env + _turbo_env_names(task.get("environmentVariables"))),
            },
        })

    workspace_outputs = sorted(_unique(workspace_outputs))
    for target in targets:
        target["attrs"]["workspace_outputs"] = workspace_outputs
    return {
        "targets": targets,
        "roots": [target["name"] for target in targets if target["attrs"]["task"] == "build"],
        "attrs": {"_default_test_roots": [target["name"] for target in targets if target["attrs"]["task"] == "test"]},
    }

def _turbo_workspace_impl(ctx):
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "turbo_workspace",
        "turbo_workspace": True,
        "targets": ctx["deps"],
    }

# Runs one task's script and snapshots its outputs into a declared directory.
# Everything the action touches is either a declared input or a declared
# output, and the script does its own cleanup and log capture instead of
# relying on `clean_paths`, `create_dirs`, or stream redirection, so every
# remote execution provider can run it.
_TURBO_RUN = """
const fs = require('node:fs');
const path = require('node:path');
const { spawn } = require('node:child_process');
const spec = JSON.parse(process.argv[1]);
const root = process.cwd();

function globToRegExp(pattern) {
  let source = '';
  let braces = 0;
  for (let i = 0; i < pattern.length; i++) {
    const ch = pattern[i];
    if (ch === '*' && pattern[i + 1] === '*') {
      const slash = pattern[i + 2] === '/';
      source += slash ? '(?:.*/)?' : '.*';
      i += slash ? 2 : 1;
    } else if (ch === '*') source += '[^/]*';
    else if (ch === '?') source += '[^/]';
    else if (ch === '{') { braces++; source += '(?:'; }
    else if (ch === '}' && braces > 0) { braces--; source += ')'; }
    else if (ch === ',' && braces > 0) source += '|';
    else source += ch.replace(/[.+^$()|[\\]{}\\\\]/g, '\\\\$&');
  }
  return new RegExp('^' + source + '(?:/.*)?$');
}

function staticBase(pattern) {
  const parts = [];
  for (const part of pattern.split('/')) {
    if (/[*?{}[\\]]/.test(part)) break;
    parts.push(part);
  }
  return parts.join('/');
}

function walk(relative, files) {
  let stat;
  try { stat = fs.lstatSync(relative); } catch { return; }
  if (stat.isDirectory()) {
    for (const entry of fs.readdirSync(relative).sort()) {
      if (!['node_modules', '.git', '.struo'].includes(entry)) walk(relative + '/' + entry, files);
    }
  } else {
    files.push(relative);
  }
}

function snapshotOutputs() {
  const include = spec.outputs.filter(p => !p.startsWith('!'));
  const exclude = spec.outputs.filter(p => p.startsWith('!')).map(p => globToRegExp(p.slice(1)));
  const entries = [];
  const covered = [];
  for (const pattern of include) {
    const base = staticBase(pattern);
    const whole = /^(.*)\\/\\*\\*(\\/\\*)?$/.exec(pattern);
    if (whole && whole[1] === base && base
        && !spec.outputs.some(p => p.startsWith('!') && p.slice(1).startsWith(base + '/'))
        && fs.existsSync(base) && fs.lstatSync(base).isDirectory()) {
      entries.push({ path: base, kind: 'tree' });
      covered.push(base + '/');
    }
  }
  const files = new Set();
  for (const pattern of include) {
    const matcher = globToRegExp(pattern);
    const found = [];
    walk(staticBase(pattern) || '.', found);
    for (const file of found) {
      const normalized = file.replace(/^\\.\\//, '');
      if (covered.some(prefix => normalized.startsWith(prefix))) continue;
      if (matcher.test(normalized) && !exclude.some(regexp => regexp.test(normalized))) files.add(normalized);
    }
  }
  for (const file of [...files].sort()) entries.push({ path: file, kind: 'file' });
  fs.mkdirSync(path.join(spec.snapshot, 'files'), { recursive: true });
  entries.forEach((entry, index) => {
    fs.cpSync(entry.path, path.join(spec.snapshot, 'files', String(index)), { recursive: true, verbatimSymlinks: true });
  });
  fs.writeFileSync(path.join(spec.snapshot, 'manifest.json'), JSON.stringify(entries));
}

// Locally, dependency outputs are already restored in place. A remote worker
// receives only their snapshots, so unpack whatever is missing.
for (const snapshot of spec.restore) {
  const manifest = path.join(snapshot, 'manifest.json');
  if (!fs.existsSync(manifest)) continue;
  JSON.parse(fs.readFileSync(manifest, 'utf8')).forEach((entry, index) => {
    if (fs.existsSync(entry.path)) return;
    fs.mkdirSync(path.dirname(entry.path), { recursive: true });
    fs.cpSync(path.join(snapshot, 'files', String(index)), entry.path, { recursive: true, verbatimSymlinks: true });
  });
}
for (const stale of [spec.snapshot, spec.results, spec.log]) {
  if (stale) fs.rmSync(stale, { recursive: true, force: true });
}

let log = null;
if (spec.log) {
  fs.mkdirSync(path.dirname(spec.log), { recursive: true });
  log = fs.openSync(spec.log, 'w');
}
const started = performance.now();
const child = spawn(spec.command, {
  cwd: path.join(root, spec.cwd), shell: true,
  stdio: ['ignore', log === null ? 'inherit' : 'pipe', log === null ? 'inherit' : 'pipe'],
});
if (log !== null) {
  for (const [stream, target] of [[child.stdout, process.stdout], [child.stderr, process.stderr]]) {
    stream.on('data', chunk => { fs.writeSync(log, chunk); target.write(chunk); });
  }
}
child.on('error', error => { console.error(error); process.exitCode = 1; });
child.on('close', (code, signal) => {
  if (log !== null) fs.closeSync(log);
  const status = signal ? 1 : (code ?? 1);
  if (spec.results) {
    const state = status === 0 ? 'passed' : 'failed';
    fs.writeFileSync(spec.results, JSON.stringify({
      schema: 'struo.test_results.v1', target: spec.target,
      runner: { type: 'turbo', metadata: { unit: 'suite' } }, status: state,
      summary: { total: 1, passed: status === 0 ? 1 : 0, failed: status === 0 ? 0 : 1, skipped: 0, flaky: 0 },
      cases: [{ id: spec.target + '::suite', name: spec.task_id, suite: spec.target,
        status: state, attempts: [{ status: state, duration_ms: Math.round(performance.now() - started) }],
        runner_metadata: { unit: 'suite' } }],
      artifacts: { logs: [spec.log], native_results: [] },
    }));
  }
  if (status === 0 && spec.snapshot) snapshotOutputs();
  if (signal) process.kill(process.pid, signal);
  process.exitCode = status;
});
"""

def turbo_restore_outputs(ctx):
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
            identifier = "turbo-restore-" + str(index),
        )

def _turbo_glob_exclusions(patterns):
    out = []
    for pattern in patterns:
        out.append(pattern)
        if pattern.endswith("/**"):
            out.append(pattern + "/*")
        elif not pattern.endswith("/**/*"):
            out.append(pattern + "/**/*")
    return out

def _turbo_inputs(ctx):
    outputs = [output for output in _turbo_attr(ctx, "workspace_outputs", []) or _turbo_attr(ctx, "outputs", []) if not output.startswith("!")]
    excluded = _turbo_glob_exclusions(outputs) + ["**/" + name + "/**/*" for name in _TURBO_IGNORED_DIRS] + [name + "/**/*" for name in _TURBO_IGNORED_DIRS]
    source_dirs = _turbo_attr(ctx, "source_dirs", [])
    sources = []
    for directory in source_dirs:
        sources.append((directory + "/**/*") if directory else "**/*")
    dependencies = []
    for pattern in _turbo_attr(ctx, "dependencies", ["node_modules/**/*"]):
        dependencies.append(pattern + "/*" if pattern.endswith("/**") else pattern)
    for directory in source_dirs:
        if directory:
            dependencies.append(directory + "/node_modules/**/*")
    # Package managers link workspace packages into node_modules, including
    # pnpm's hidden `.pnpm/node_modules`. Links to packages whose files are not
    # inputs would dangle on a remote worker.
    unlinked = []
    for name in _turbo_attr(ctx, "other_packages", []):
        unlinked.extend(["node_modules/" + name, "**/node_modules/" + name])
    return _unique(
        glob(sources, exclude = excluded) +
        _file_globs(_turbo_attr(ctx, "config", _TURBO_CONFIG)) +
        _file_globs(_turbo_attr(ctx, "global_inputs", [])) +
        glob(dependencies, exclude = unlinked),
    )

def _turbo_task_impl(ctx, is_test = False):
    package = _turbo_attr(ctx, "package", "")
    task = _turbo_attr(ctx, "task", "")
    command = _turbo_attr(ctx, "command", "")
    if not package or not task:
        fail(ctx["label"]["id"] + ": turbo tasks require `package` and `task` attributes")
    task_id = package + "#" + task
    outputs = _turbo_attr(ctx, "outputs", [])
    cacheable = _turbo_attr(ctx, "cache", True)
    provider = {
        "label_id": ctx["label"]["id"],
        "target_kind": "turbo_test" if is_test else "turbo_task",
        "turbo_package": package,
        "turbo_task_name": task,
        "outputs": outputs,
    }
    if ctx["capability"] != "build" and not is_test:
        return provider
    if not command:
        fail(ctx["label"]["id"] + ": turbo task `" + task_id + "` has no script to run")

    dependency_artifacts = _unique([path for dep in ctx["deps"] for path in dep.get("artifact_inputs", [])])
    inputs = _unique(_turbo_inputs(ctx) + dependency_artifacts)

    package_dir = _turbo_attr(ctx, "package_dir", "")
    node = _turbo_node(ctx)
    env = _turbo_env(node, package_dir)
    for name in _turbo_attr(ctx, "env_inherit", []):
        value = host_env(name)
        if value:
            env[name] = value
    for name, value in _turbo_attr(ctx, "env", {}).items():
        env[name] = value
    node_version = host_command([node, "--version"]).strip()

    snapshot = declare_output("turbo-outputs") if outputs and cacheable else None
    action_outputs = [snapshot] if snapshot else []
    provider["artifact_inputs"] = dependency_artifacts + action_outputs
    spec = {
        "command": command,
        "cwd": package_dir or ".",
        "outputs": outputs,
        "snapshot": snapshot,
        "restore": dependency_artifacts,
        "task_id": task_id,
    }
    if is_test:
        test_dir = _test_output_dir(ctx)
        results = test_dir + "/test_results.json"
        log = test_dir + "/turbo.log"
        spec.update({"results": results, "log": log, "target": ctx["label"]["id"]})
        action_outputs = action_outputs + [results, log]
        argv = [node, "-e", _TURBO_RUN, _json_encode(spec)]
        provider["affected_inputs"] = inputs
        provider["test_discovery_inputs"] = inputs
        provider["test_info"] = {
            "schema": "struo.test_info.v1", "target": ctx["label"]["id"],
            "runner": {"type": "turbo", "display_name": "Turborepo", "metadata": {"unit": "suite"}},
            "command": {"argv": argv, "env": env, "cwd": "."},
            "outputs": {"results": results, "logs": [log], "native_results": [], "coverage": []},
            "listing": {"supported": False, "strategy": "unsupported"},
            "filtering": {"case_filtering": "unsupported"},
            "sharding": {"supported": False, "granularity": "target"},
            "retries": {"supported": False, "default_attempts": 1},
            "execution": {"cacheable": cacheable, "run_from_workspace_root": True},
            "labels": [], "metadata": {},
        }
        if ctx["capability"] not in ["build", "test"]:
            return provider

    run_action(
        display_name = "Run Turborepo task · " + task_id,
        presentation = _action_metadata(context = [_action_context("turbo.task", task_id)]),
        source_files = _action_source_files(inputs),
        argv = [node, "-e", _TURBO_RUN, _json_encode(spec)],
        inputs = inputs,
        outputs = action_outputs,
        env = env,
        cwd = ".",
        cacheable = cacheable,
        toolchain_identity = "struo.turbo.task.v1\x00" + node + "\x00" + node_version,
        identifier = ctx["label"]["id"] + ":turbo-task",
    )
    if snapshot:
        expand_actions(
            implementation = "turbo_restore_outputs",
            inputs = [snapshot + "/manifest.json"],
            outputs = [],
            args = {"snapshot": snapshot},
        )
    return provider

def _turbo_test_impl(ctx):
    return _turbo_task_impl(ctx, is_test = True)

turbo_workspace = target_kind(
    docs = "Native Turborepo workspace seed. Its resolver asks turbo for the package graph and the resolved task graph, translates every runnable package task into a first-class Struo target, and hands scheduling, caching, and remote execution to Struo instead of `turbo run`.",
    attrs = [
        attr("node", "string", default = "\"node\"", docs = "Node.js executable name, absolute path, or workspace-relative path.", configurable = False),
        attr("targets", "list<string>", default = "[\"build\", \"test\", \"lint\"]", docs = "Turborepo task names to emit, together with the tasks they depend on. Names that no package defines are skipped. Defaults to build, test, and lint. Set an empty list to include every task.", configurable = False),
        attr("resolver_inputs", "list<string>", default = "[]", docs = "Package-relative text globs supplied to native integration resolution. Defaults to srcs when empty.", configurable = False),
        attr("graph_file", "string", default = "\"\"", docs = "Optional workspace-relative path to a checked-in graph snapshot: a JSON object whose `packages` holds the items of `turbo query` packages (with `name`, `path`, and `allDependencies`) and whose `dry` holds the output of `turbo run --dry=json`. Struo reads it directly instead of invoking turbo.", configurable = False),
        attr("_default_test_roots", "list<string>", default = "[]", docs = "Resolver-owned first-party test target names for targetless test selection.", configurable = False),
    ],
    resolver = _turbo_workspace_resolver,
    deps = [dep("deps", ["turbo_task"], "Turborepo tasks emitted by native integration discovery.")],
    providers = ["turbo_workspace"],
    capabilities = [capability("build", [])],
    tools = [_TURBO_NODE_TOOL, _TURBO_TOOL],
    examples = [
        example(
            "turbo-workspace-native-project",
            name = "Turborepo native integration seed",
            use_when = "Use this when a Turborepo workspace should derive build, test, and lint targets from turbo.json without redeclaring them in Struo.",
        ),
    ],
    impl = _turbo_workspace_impl,
)

def _turbo_task_attrs():
    return [
        attr("package", "string", required = True, docs = "Workspace package name, or `//` for a root task.", configurable = False),
        attr("task", "string", required = True, docs = "Turborepo task name, such as `build`, `test`, or `lint`.", configurable = False),
        attr("command", "string", required = True, docs = "The package script the task runs, executed through the platform shell from `package_dir`.", configurable = False),
        attr("package_dir", "string", default = "\"\"", docs = "Workspace-relative package directory. Empty for the workspace root.", configurable = False),
        attr("outputs", "list<string>", default = "[]", docs = "Workspace-relative output globs, with `!` negations. Existing matches are cached and restored; absent outputs are valid.", configurable = False),
        attr("cache", "bool", default = "true", docs = "False when turbo.json disables caching for the task. The action then always runs and its outputs are not restored.", configurable = False),
        attr("source_dirs", "list<string>", default = "[]", docs = "Workspace-relative directories whose files are action inputs: the package and its internal workspace dependencies. An empty string means the whole workspace.", configurable = False),
        attr("other_packages", "list<string>", default = "[]", docs = "Workspace packages outside `source_dirs`. Their `node_modules` links are left out of the inputs so no input links to undeclared files.", configurable = False),
        attr("workspace_outputs", "list<string>", default = "[]", docs = "Resolver-owned output globs of every task, excluded from source inputs.", configurable = False),
        attr("global_inputs", "list<string>", default = "[]", docs = "Workspace-relative files from turbo.json `globalDependencies`.", configurable = False),
        attr("node", "string", default = "\"node\"", docs = "Node.js executable that runs the task wrapper.", configurable = False),
        attr("dependencies", "list<string>", default = "[\"node_modules/**/*\"]", docs = "Installed package files required at execution time. The `node_modules` of each source directory are added.", configurable = False),
        attr("config", "list<string>", default = _json_encode(_TURBO_CONFIG), docs = "Workspace-level configuration inputs.", configurable = False),
        attr("env", "map<string, string>", default = "{}", docs = "User-supplied environment variables applied last.", configurable = True),
        attr("env_inherit", "list<string>", default = "[]", docs = "Host environment variables inherited by name, from turbo.json `env`, `passThroughEnv`, and their global forms.", configurable = False),
    ]

turbo_task = target_kind(
    docs = "One Turborepo package task. Struo runs the package script directly, caches its outputs, and can execute it remotely without `turbo run`.",
    attrs = _turbo_task_attrs(),
    deps = [dep("deps", ["turbo_task"], "Upstream Turborepo tasks whose outputs this task depends on.")],
    providers = ["turbo_task"],
    capabilities = [capability("build", [])],
    tools = [_TURBO_NODE_TOOL],
    examples = [
        example(
            "turbo-task",
            name = "Turborepo task",
            use_when = "Use this to schedule one Turborepo package task through Struo when the workspace is not covered by the `turbo` native project.",
        ),
    ],
    impl = _turbo_task_impl,
)

turbo_test = target_kind(
    docs = "One Turborepo test task, executed as a cacheable target-level suite with normalized results and logs.",
    attrs = _turbo_task_attrs(),
    deps = [dep("deps", ["turbo_task"], "Prerequisite Turborepo tasks built before the test suite.")],
    providers = ["turbo_task", "struo_test_info"],
    capabilities = [capability("build", []), capability("test", ["default", "test_results", "logs"])],
    tools = [_TURBO_NODE_TOOL],
    examples = [example("turbo-test", name = "Turborepo test suite", use_when = "Run one Turborepo test script as a cacheable suite with normalized results.")],
    impl = _turbo_test_impl,
)

def _turbo_native_project(marker):
    return native_project(
        target_kind = "turbo_workspace",
        docs = "Recognizes a native Turborepo workspace from " + marker + " and emits `turbo_task` targets for each package's build, test, and lint tasks.",
        markers = [marker],
        target_name = "turbo",
        inputs = ["package.json", "pnpm-lock.yaml", "yarn.lock", "package-lock.json", "bun.lock", "**/package.json", "**/turbo.json", "**/turbo.jsonc"],
        manifests = ["turbo.json", "turbo.jsonc", "pnpm-lock.yaml", "pnpm-workspace.yaml", "yarn.lock", "package-lock.json", "bun.lock", "**/package.json", "**/turbo.json", "**/turbo.jsonc"],
        exclude = _native_project_generated_dirs() + ["node_modules", "dist", ".turbo", ".next", "tmp"],
        input_exclude = ["node_modules", ".turbo", ".git", "dist", ".next", "tmp"],
        on_match = "stop",
        requires_tools = ["node"],
    )

turbo = _turbo_native_project("turbo.json")

turbo_jsonc = _turbo_native_project("turbo.jsonc")
