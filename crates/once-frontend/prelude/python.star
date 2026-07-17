_PYTHON_TOOL = tool("python", executables = ["python3", "python"])

def _python_attr(ctx, name, default):
    value = ctx["attr"].get(name)
    return default if value == None else value

def _python_executable(ctx):
    requested = _python_attr(ctx, "python", "python3")
    resolved = host_which_optional(requested)
    if not resolved and requested == "python3":
        resolved = host_which_optional("python")
    if not resolved:
        fail(ctx["label"]["id"] + ": Python interpreter `" + requested + "` was not found")
    return resolved

def _python_env(ctx, test_dir):
    env = {"HOME": test_dir + "/home", "PYTHONDONTWRITEBYTECODE": "1"}
    for name in _python_attr(ctx, "env_inherit", []):
        value = host_env(name)
        if value:
            env[name] = value
    for name, value in _python_attr(ctx, "env", {}).items():
        env[name] = value
    return env

def _python_test_inputs(ctx):
    return _unique(glob(ctx["srcs"]) + _file_globs(_python_attr(ctx, "config", [])) + _file_globs(_python_attr(ctx, "data", [])))

def _python_pytest_adapter():
    return '''import argparse
import json
import os
import sys


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--once-results", required=True)
    parser.add_argument("--once-native-results", required=True)
    parser.add_argument("--once-target", required=True)
    parser.add_argument("--once-source", action="append", default=[])
    parser.add_argument("--once-test-unit", action="append", default=[])
    parser.add_argument("runner_args", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.runner_args[:1] == ["--"]:
        args.runner_args = args.runner_args[1:]
    return args


class OncePlugin:
    def __init__(self, target):
        self.target = target
        self.items = {}
        self.statuses = {}
        self.attempts = {}

    def pytest_collection_finish(self, session):
        for item in session.items:
            path = item.nodeid.split("::", 1)[0].replace(os.sep, "/")
            self.items[item.nodeid] = {
                "id": self.target + "::" + item.nodeid,
                "name": item.name,
                "suite": path,
                "file": path,
            }

    def pytest_runtest_logreport(self, report):
        if report.when not in ("setup", "call", "teardown"):
            return
        status = "passed" if report.passed else "skipped" if report.skipped else "failed"
        self.attempts.setdefault(report.nodeid, []).append({
            "status": status,
            "phase": report.when,
            "duration_ms": int(report.duration * 1000),
        })
        current = self.statuses.get(report.nodeid)
        if status == "failed" or current is None or (status == "skipped" and current == "passed"):
            self.statuses[report.nodeid] = status

    def normalized(self, exit_code):
        cases = []
        for nodeid in sorted(self.items):
            item = self.items[nodeid]
            status = self.statuses.get(nodeid, "failed" if exit_code else "skipped")
            cases.append({
                **item,
                "status": status,
                "attempts": self.attempts.get(nodeid, [{"status": status}]),
                "runner_metadata": {"nodeid": nodeid},
            })
        counts = {"passed": 0, "failed": 0, "skipped": 0}
        for case in cases:
            counts[case["status"] if case["status"] in counts else "failed"] += 1
        return {
            "schema": "once.test_results.v1",
            "target": self.target,
            "runner": {"type": "pytest", "metadata": {}},
            "status": "passed" if exit_code == 0 else "failed",
            "summary": {
                "total": len(cases),
                "passed": counts["passed"],
                "failed": counts["failed"],
                "skipped": counts["skipped"],
                "flaky": 0,
            },
            "cases": cases,
        }


def main():
    args = parse_args()
    try:
        import pytest
    except Exception as error:
        print("unable to import pytest: " + str(error), file=sys.stderr)
        return 2
    selected = []
    prefix = args.once_target + "::"
    for unit in args.once_test_unit:
        if not unit.startswith(prefix):
            print("test unit does not belong to target: " + unit, file=sys.stderr)
            return 2
        selected.append(unit[len(prefix):])
    plugin = OncePlugin(args.once_target)
    exit_code = int(pytest.main(args.runner_args + (selected or args.once_source), plugins=[plugin]))
    native = plugin.normalized(exit_code)
    native["exit_code"] = exit_code
    os.makedirs(os.path.dirname(args.once_results), exist_ok=True)
    with open(args.once_native_results, "w", encoding="utf-8") as file:
        json.dump(native, file, indent=2, sort_keys=True)
    normalized = dict(native)
    normalized.pop("exit_code", None)
    normalized["artifacts"] = {
        "logs": [os.path.dirname(args.once_results) + "/pytest.log"],
        "native_results": [args.once_native_results],
    }
    with open(args.once_results, "w", encoding="utf-8") as file:
        json.dump(normalized, file, indent=2, sort_keys=True)
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
'''

def _python_pytest_impl(ctx):
    python = _python_executable(ctx)
    inputs = _python_test_inputs(ctx)
    test_dir = _test_output_dir(ctx)
    results = test_dir + "/test_results.json"
    log = test_dir + "/pytest.log"
    native_results = test_dir + "/pytest-results.json"
    adapter = test_dir + "/once_pytest_adapter.py"
    filters = (ctx.get("test") or {}).get("filters") or []
    info = {
        "schema": "once.test_info.v1",
        "target": ctx["label"]["id"],
        "runner": {"type": "pytest", "display_name": "pytest", "metadata": {}},
        "command": {"argv": [python, adapter], "env": _python_env(ctx, test_dir), "cwd": "."},
        "outputs": {"results": results, "logs": [log], "native_results": [native_results], "coverage": []},
        "listing": {"supported": True, "strategy": "normalized_results"},
        "filtering": {"case_filtering": "runner_args"},
        "sharding": {"supported": True, "granularity": _python_attr(ctx, "batching", "file")},
        "retries": {"supported": False, "default_attempts": 1},
        "execution": {"cacheable": True, "timeout_ms": _python_attr(ctx, "timeout_ms", None), "run_from_workspace_root": True},
        "labels": _python_attr(ctx, "labels", []),
        "metadata": {},
    }
    provider = {"label_id": ctx["label"]["id"], "target_kind": "pytest_test", "affected_inputs": inputs, "test_discovery_inputs": inputs, "test_info": info}
    if ctx["capability"] != "test":
        return provider
    write_path(adapter, _python_pytest_adapter())
    argv = [python, adapter, "--once-results", results, "--once-native-results", native_results, "--once-target", ctx["label"]["id"]]
    for source in glob(ctx["srcs"]):
        argv.extend(["--once-source", source])
    for unit in filters:
        argv.extend(["--once-test-unit", unit])
    argv.append("--")
    argv.extend(_python_attr(ctx, "args", []))
    version = host_command([python, "--version"], merge_stderr = True).strip()
    run_action(
        argv = argv,
        inputs = _unique(inputs + [adapter]),
        outputs = [test_dir, results, log, native_results],
        clean_paths = [results, log, native_results],
        create_dirs = [test_dir, test_dir + "/home"],
        env = _python_env(ctx, test_dir),
        stdout = log,
        stderr = log,
        toolchain_identity = "once.pytest.v1\x00" + python + "\x00" + version,
        identifier = ctx["label"]["id"] + ":pytest",
    )
    return provider

pytest_test = target_kind(
    docs = "Runs pytest suites through Once with stable case discovery, exact filtering, normalized results, and file-oriented automatic batching.",
    attrs = [
        attr("python", "string", default = "\"python3\"", docs = "Python interpreter executable.", configurable = False),
        attr("config", "list<string>", default = "[\"pyproject.toml\", \"pytest.ini\", \"conftest.py\"]", docs = "Configuration and collection inputs.", configurable = False),
        attr("data", "list<string>", default = "[]", docs = "Runtime data inputs read by tests.", configurable = False),
        attr("args", "list<string>", default = "[]", docs = "Additional pytest arguments.", configurable = False),
        attr("env", "map<string,string>", default = "{}", docs = "Environment variables for test execution.", configurable = True),
        attr("env_inherit", "list<string>", default = "[]", docs = "Host environment variables inherited by name.", configurable = False),
        attr("batching", "string", default = "\"file\"", docs = "Automatic batch granularity: `file`, `case`, or `target`.", configurable = False),
        attr("labels", "list<string>", default = "[]", docs = "Labels exposed through test discovery."),
        attr("timeout_ms", "int", docs = "Optional test timeout in milliseconds.", configurable = False),
    ],
    deps = [dep("deps", [], "Targets whose inputs should affect this test suite.")],
    providers = ["once_test_info"],
    capabilities = [capability("test", ["default", "test_results", "logs"])],
    tools = [_PYTHON_TOOL],
    examples = [example("pytest-test-minimal", name = "Minimal pytest suite", use_when = "You want Python tests with exact case execution and automatic file batching.")],
    impl = _python_pytest_impl,
)
