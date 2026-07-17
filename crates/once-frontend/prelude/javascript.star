_JAVASCRIPT_TOOL = tool("node", executables = ["node"])

def _javascript_attr(ctx, name, default):
    value = ctx["attr"].get(name)
    return default if value == None else value

def _javascript_node(ctx):
    requested = _javascript_attr(ctx, "node", "node")
    resolved = host_which_optional(requested)
    if not resolved:
        fail(ctx["label"]["id"] + ": Node.js executable `" + requested + "` was not found")
    return resolved

def _javascript_env(ctx, test_dir):
    env = {"HOME": test_dir + "/home", "CI": "1"}
    for name in _javascript_attr(ctx, "env_inherit", []):
        value = host_env(name)
        if value:
            env[name] = value
    for name, value in _javascript_attr(ctx, "env", {}).items():
        env[name] = value
    return env

def _javascript_runner_path(ctx, default):
    value = _javascript_attr(ctx, "runner", default)
    return _package_relative(ctx, value)

def _javascript_discovery_inputs(ctx):
    return _unique(glob(ctx["srcs"]) + _file_globs(_javascript_attr(ctx, "config", [])) + _file_globs(_javascript_attr(ctx, "data", [])))

def _javascript_test_inputs(ctx, runner, discovery_inputs):
    return _unique(discovery_inputs + _file_globs(_javascript_attr(ctx, "dependencies", [])) + glob([runner]))

def _javascript_test_adapter():
    return '''import { spawnSync } from "node:child_process"
import fs from "node:fs"
import path from "node:path"

function parseArgs(argv) {
  const options = { sources: [], units: [], runnerArgs: [] }
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index]
    if (argument === "--") {
      options.runnerArgs = argv.slice(index + 1)
      break
    }
    const key = argument.replace(/^--once-/, "").replaceAll("-", "_")
    const value = argv[index + 1]
    index += 1
    if (key === "source") options.sources.push(value)
    else if (key === "test_unit") options.units.push(value)
    else options[key] = value
  }
  return options
}

function escapePattern(value) {
  const special = "^$.*+?()[]{}|"
  return [...value].map(character =>
    special.includes(character) || character.charCodeAt(0) === 92
      ? String.fromCharCode(92) + character
      : character
  ).join("")
}

function decodeUnits(options) {
  const prefix = options.target + "::"
  return options.units.map(unit => {
    if (!unit.startsWith(prefix)) throw new Error("test unit does not belong to target: " + unit)
    const decoded = JSON.parse(decodeURIComponent(unit.slice(prefix.length)))
    if (!Array.isArray(decoded) || decoded.length !== 2) throw new Error("invalid test unit: " + unit)
    return { file: decoded[0], name: decoded[1] }
  })
}

function normalizedStatus(status) {
  if (status === "passed") return "passed"
  if (["pending", "todo", "disabled", "skipped"].includes(status)) return "skipped"
  return "failed"
}

function relativeFile(value) {
  const relative = path.isAbsolute(value) ? path.relative(process.cwd(), value) : value
  return relative.split(path.sep).join("/")
}

function main() {
  const options = parseArgs(process.argv.slice(2))
  const units = decodeUnits(options)
  const files = units.length ? [...new Set(units.map(unit => unit.file))] : options.sources
  const names = units.map(unit => unit.name)
  const namePattern = names.length ? "^(?:" + names.map(escapePattern).join("|") + ")$" : null
  let args
  if (options.runner_type === "vitest") {
    args = [options.runner, "run", ...options.runnerArgs, "--reporter=json", "--outputFile=" + options.native_results, ...files]
    if (namePattern) args.push("--testNamePattern", namePattern)
  } else {
    args = [options.runner, ...options.runnerArgs, "--runInBand", "--json", "--outputFile=" + options.native_results, "--runTestsByPath", ...files]
    if (namePattern) args.push("--testNamePattern", namePattern)
  }
  const execution = spawnSync(process.execPath, args, { encoding: "utf8", env: process.env })
  if (execution.stdout) process.stdout.write(execution.stdout)
  if (execution.stderr) process.stderr.write(execution.stderr)
  const exitCode = execution.status ?? 1
  let native = { testResults: [] }
  if (fs.existsSync(options.native_results)) {
    native = JSON.parse(fs.readFileSync(options.native_results, "utf8"))
  } else {
    fs.writeFileSync(options.native_results, JSON.stringify(native, null, 2))
  }
  const cases = []
  const identifiers = new Set()
  let duplicates = false
  for (const suite of native.testResults || []) {
    const file = relativeFile(suite.name || "unknown")
    for (const assertion of suite.assertionResults || []) {
      const fullName = assertion.fullName || [...(assertion.ancestorTitles || []), assertion.title].join(" ")
      const identifier = options.target + "::" + encodeURIComponent(JSON.stringify([file, fullName]))
      if (identifiers.has(identifier)) {
        duplicates = true
        continue
      }
      identifiers.add(identifier)
      const status = normalizedStatus(assertion.status)
      cases.push({
        id: identifier,
        name: assertion.title || fullName,
        suite: (assertion.ancestorTitles || []).join(" > ") || file,
        file,
        status,
        attempts: [{ status }],
        runner_metadata: { full_name: fullName, duration_ms: assertion.duration ?? null },
      })
    }
  }
  if (duplicates) {
    console.error("duplicate full test names cannot be filtered exactly")
    cases.push({
      id: options.target + "::once-duplicate-identifiers",
      name: "duplicate full test names cannot be filtered exactly",
      suite: options.target,
      status: "failed",
      attempts: [{ status: "failed" }],
      runner_metadata: {},
    })
  }
  const passed = cases.filter(item => item.status === "passed").length
  const failed = cases.filter(item => item.status === "failed").length
  const skipped = cases.filter(item => item.status === "skipped").length
  const status = exitCode === 0 && !duplicates ? "passed" : "failed"
  const normalized = {
    schema: "once.test_results.v1",
    target: options.target,
    runner: { type: options.runner_type, metadata: {} },
    status,
    summary: { total: cases.length, passed, failed, skipped, flaky: 0 },
    cases,
    artifacts: {
      logs: [path.posix.join(path.posix.dirname(options.results), options.runner_type + ".log")],
      native_results: [options.native_results],
    },
  }
  fs.writeFileSync(options.results, JSON.stringify(normalized, null, 2))
  return status === "passed" ? 0 : exitCode || 1
}

try {
  process.exitCode = main()
} catch (error) {
  console.error(error instanceof Error ? error.stack : String(error))
  process.exitCode = 2
}
'''

def _javascript_test_info(ctx, runner_type, display_name, node, runner, adapter, results, log, native_results, test_dir):
    return {
        "schema": "once.test_info.v1",
        "target": ctx["label"]["id"],
        "runner": {"type": runner_type, "display_name": display_name, "metadata": {}},
        "command": {"argv": [node, adapter], "env": _javascript_env(ctx, test_dir), "cwd": "."},
        "outputs": {"results": results, "logs": [log], "native_results": [native_results], "coverage": []},
        "listing": {"supported": True, "strategy": "normalized_results"},
        "filtering": {"case_filtering": "runner_args"},
        "sharding": {"supported": True, "granularity": _javascript_attr(ctx, "batching", "file")},
        "retries": {"supported": False, "default_attempts": 1},
        "execution": {"cacheable": True, "timeout_ms": _javascript_attr(ctx, "timeout_ms", None), "run_from_workspace_root": True},
        "labels": _javascript_attr(ctx, "labels", []),
        "metadata": {},
    }

def _javascript_test_impl(ctx, runner_type, display_name, default_runner):
    node = _javascript_node(ctx)
    runner = _javascript_runner_path(ctx, default_runner)
    discovery_inputs = _javascript_discovery_inputs(ctx)
    inputs = _javascript_test_inputs(ctx, runner, discovery_inputs)
    test_dir = _test_output_dir(ctx)
    results = test_dir + "/test_results.json"
    log = test_dir + "/" + runner_type + ".log"
    native_results = test_dir + "/" + runner_type + "-results.json"
    adapter = test_dir + "/once_javascript_test_adapter.mjs"
    info = _javascript_test_info(ctx, runner_type, display_name, node, runner, adapter, results, log, native_results, test_dir)
    provider = {"label_id": ctx["label"]["id"], "target_kind": runner_type + "_test", "affected_inputs": inputs, "test_discovery_inputs": discovery_inputs, "test_info": info}
    if ctx["capability"] != "test":
        return provider
    write_path(adapter, _javascript_test_adapter())
    argv = [node, adapter, "--once-runner-type", runner_type, "--once-runner", runner, "--once-results", results, "--once-native-results", native_results, "--once-target", ctx["label"]["id"]]
    for source in glob(ctx["srcs"]):
        argv.extend(["--once-source", source])
    for unit in (ctx.get("test") or {}).get("filters") or []:
        argv.extend(["--once-test-unit", unit])
    argv.append("--")
    argv.extend(_javascript_attr(ctx, "args", []))
    version = host_command([node, "--version"]).strip()
    run_action(
        argv = argv,
        inputs = _unique(inputs + [adapter]),
        outputs = [test_dir, results, log, native_results],
        clean_paths = [results, log, native_results],
        create_dirs = [test_dir, test_dir + "/home"],
        env = _javascript_env(ctx, test_dir),
        stdout = log,
        stderr = log,
        toolchain_identity = "once." + runner_type + ".v1\x00" + node + "\x00" + version,
        identifier = ctx["label"]["id"] + ":" + runner_type,
    )
    return provider

def _vitest_impl(ctx):
    return _javascript_test_impl(ctx, "vitest", "Vitest", "node_modules/vitest/vitest.mjs")

def _jest_impl(ctx):
    return _javascript_test_impl(ctx, "jest", "Jest", "node_modules/jest/bin/jest.js")

_JAVASCRIPT_TEST_ATTRS = [
    attr("node", "string", default = "\"node\"", docs = "Node.js executable.", configurable = False),
    attr("runner", "string", docs = "Package-relative runner entry point.", configurable = False),
    attr("config", "list<string>", default = "[\"package.json\", \"package-lock.json\", \"pnpm-lock.yaml\", \"yarn.lock\"]", docs = "Configuration and dependency inputs.", configurable = False),
    attr("dependencies", "list<string>", default = "[\"node_modules/**/*\"]", docs = "Installed runner and package files required during execution.", configurable = False),
    attr("data", "list<string>", default = "[]", docs = "Runtime data, setup, transform, snapshot, and runner package inputs.", configurable = False),
    attr("args", "list<string>", default = "[]", docs = "Additional runner arguments.", configurable = False),
    attr("env", "map<string,string>", default = "{}", docs = "Environment variables for test execution.", configurable = True),
    attr("env_inherit", "list<string>", default = "[]", docs = "Host environment variables inherited by name.", configurable = False),
    attr("batching", "string", default = "\"file\"", docs = "Automatic batch granularity: `file`, `case`, or `target`.", configurable = False),
    attr("labels", "list<string>", default = "[]", docs = "Labels exposed through test discovery."),
    attr("timeout_ms", "int", docs = "Optional test timeout in milliseconds.", configurable = False),
]

vitest_test = target_kind(
    docs = "Runs Vitest suites through Once with exact file and case filtering, normalized results, and automatic batching.",
    attrs = _JAVASCRIPT_TEST_ATTRS,
    deps = [dep("deps", [], "Targets whose inputs should affect this test suite.")],
    providers = ["once_test_info"],
    capabilities = [capability("test", ["default", "test_results", "logs"])],
    tools = [_JAVASCRIPT_TOOL],
    examples = [example("vitest-test-minimal", name = "Minimal Vitest suite", use_when = "You want JavaScript or TypeScript tests scheduled by file through Once.")],
    impl = _vitest_impl,
)

jest_test = target_kind(
    docs = "Runs Jest suites through Once with exact file and case filtering, normalized results, and automatic batching.",
    attrs = _JAVASCRIPT_TEST_ATTRS,
    deps = [dep("deps", [], "Targets whose inputs should affect this test suite.")],
    providers = ["once_test_info"],
    capabilities = [capability("test", ["default", "test_results", "logs"])],
    tools = [_JAVASCRIPT_TOOL],
    examples = [example("jest-test-minimal", name = "Minimal Jest suite", use_when = "You want JavaScript or TypeScript Jest tests scheduled by file through Once.")],
    impl = _jest_impl,
)
