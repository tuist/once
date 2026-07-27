# Custom Lint Target Kinds

Use a project-local lint target kind when no built-in integration covers an
analyzer. The module owns tool-specific command behavior and report
conversion. Once provides generic action execution, caching, normalized
findings, failure policy, and evidence.

Start by checking whether a built-in integration already exists:

```sh
once query target-kinds
```

The [Linting guide](/guide/graph/linting) introduces the built-in analyzers and
the `once lint` workflow.

## Choose an integration shape

Every lint target must finish with a valid
[Static Analysis Results Interchange Format report](https://docs.oasis-open.org/sarif/sarif/v2.1.0/sarif-v2.1.0.html).
There are two useful shapes:

1. Invoke the analyzer directly when it can write that report.
2. Capture a native machine-readable report, then run a second declared action
   that converts it.

Keep conversion in the target kind. Do not add analyzer names or native report
parsers to Once's Rust execution layer.

## Get the current authoring contract

Ask the installed Once version for its exact Starlark contract:

```sh
once query module-contract --format json
```

The result contains `lint_starter`, `lint_target_starter`,
`lint_adapter_starter`, and `normalized_lint_result_example`. A coding agent
can use these fields without relying on remembered function signatures.

## Define a complete lint target kind

The following module runs a project-owned Python adapter. The adapter receives
the report destination and declared sources, writes the portable report, and
uses process code `1` when it found issues.

```python
def _package_path(ctx, path):
    package = ctx["label"]["package"]
    if package:
        return package + "/" + path
    return path

def _scripted_lint_impl(ctx):
    lint_dir = ctx["build_dir"] + "/lint"
    report = lint_dir + "/report.sarif"
    results = lint_dir + "/lint_results.json"
    sources = glob(ctx["srcs"])
    config = glob(ctx["attr"].get("config") or [])
    data = glob(ctx["attr"].get("data") or [])
    program = _package_path(ctx, ctx["attr"]["program"])
    python = host_which(ctx["attr"].get("python") or "python3")
    identity = (
        "scripted_lint.v1\x00" +
        python + "\x00" +
        host_command([python, "--version"], merge_stderr = True).strip()
    )
    argv = [
        python,
        program,
        "--report", execution_path(report),
        "--analyzer", ctx["attr"]["analyzer"],
    ] + (ctx["attr"].get("args") or []) + sources
    findings_exit_code = ctx["attr"].get("findings_exit_code")
    if findings_exit_code == None:
        findings_exit_code = 1
    success_codes = [0]
    if findings_exit_code != 0:
        success_codes.append(findings_exit_code)
    inputs = sources + config + data + [program]

    if ctx["capability"] == "lint":
        run_action(
            argv = argv,
            inputs = inputs,
            outputs = [report],
            clean_paths = [report],
            create_dirs = [lint_dir],
            success_exit_codes = success_codes,
            toolchain_identity = identity,
            identifier = "scripted_lint:" + ctx["label"]["id"],
        )

    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "scripted_lint",
        "affected_inputs": inputs,
        "lint_info": {
            "schema": "once.lint_info.v1",
            "target": ctx["label"]["id"],
            "analyzer": {
                "type": ctx["attr"]["analyzer"],
                "display_name": ctx["attr"]["analyzer"],
                "metadata": {},
            },
            "command": {"argv": argv, "env": {}, "cwd": "."},
            "outputs": {
                "sarif": report,
                "results": results,
                "native_results": [],
                "logs": [],
            },
            "scope": {"requested": sources},
            "execution": {
                "cacheable": True,
                "run_from_workspace_root": True,
            },
            "metadata": {},
        },
    }

scripted_lint = target_kind(
    docs = "Runs a project-owned lint adapter and exposes normalized findings.",
    attrs = [
        attr("analyzer", "string", required = True, configurable = False),
        attr("program", "string", required = True, configurable = False),
        attr("python", "string", default = "\"python3\"", configurable = False),
        attr("config", "list<string>", default = "[]", configurable = False),
        attr("data", "list<string>", default = "[]", configurable = False),
        attr("args", "list<string>", default = "[]", configurable = False),
        attr("findings_exit_code", "int", default = "1", configurable = False),
    ],
    providers = ["once_lint_info"],
    capabilities = [capability("lint", ["default", "lint_results"])],
    tools = [tool("python", executables = ["python3", "python"])],
    impl = _scripted_lint_impl,
)
```

Important parts of this contract:

- `inputs` includes sources, native configuration, additional data, and the
  adapter itself.
- `toolchain_identity` includes the executable and version, so analyzer
  upgrades cannot reuse an older report.
- `execution_path(report)` gives the process an absolute path for local,
  sandboxed, or remote execution without embedding a host path in the action
  identity.
- `success_exit_codes` separates valid findings from tool crashes.
- `lint_info.outputs.sarif` names the portable report produced by the action.
- `lint_info.outputs.results` reserves the normalized result written by
  `once lint` after it reads the portable report.
- Both output fields must contain non-empty workspace-relative paths. Once
  validates them before running the target actions and returns a structured
  repair diagnostic when the provider contract is incomplete.

## Register and declare the target

Register the module and target in the root `once.toml`:

```toml
[modules]
paths = ["modules/*.star"]

[[target]]
name = "lint"
kind = "scripted_lint"
srcs = ["src/**/*.txt"]

[target.attrs]
analyzer = "TODO checker"
program = "tools/lint_adapter.py"
findings_exit_code = 1
```

Pin the adapter runtime in the repository:

```sh
mise use python@latest
```

The `lint_adapter_starter` returned by `once query module-contract` is a
complete adapter that reports lines containing `TODO`. Save it as
`tools/lint_adapter.py`, or replace it with an adapter for the analyzer being
integrated.

## Invoke a native portable reporter directly

When the analyzer already produces the portable report, the implementation can
invoke it directly:

```python
argv = [
    analyzer,
    "--format", "sarif",
    "--output", execution_path(report),
] + sources

run_action(
    argv = argv,
    inputs = sources + config,
    outputs = [report],
    clean_paths = [report],
    create_dirs = [lint_dir],
    success_exit_codes = [0, findings_exit_code],
    toolchain_identity = analyzer_identity,
    identifier = "lint:" + ctx["label"]["id"],
)
```

Use only process codes documented by the analyzer. A configuration error,
missing plugin, crash, or incomplete report must remain an action failure.

## Convert a native report

When the analyzer only emits its own machine-readable format, declare two
actions:

```python
native = lint_dir + "/native-results.json"
adapter = lint_dir + "/native-to-sarif.py"

write_path(adapter, adapter_source)

run_action(
    argv = analyzer_argv,
    inputs = sources + config,
    outputs = [native],
    clean_paths = [native],
    create_dirs = [lint_dir],
    success_exit_codes = [0, findings_exit_code],
    toolchain_identity = analyzer_identity,
    identifier = "lint-native:" + ctx["label"]["id"],
)

run_action(
    argv = [python, adapter, native, report],
    inputs = [adapter, native],
    outputs = [report],
    clean_paths = [report],
    toolchain_identity = adapter_identity,
    identifier = "lint-convert:" + ctx["label"]["id"],
)
```

The native report remains inspectable through
`lint_info.outputs.native_results`. The converter is an action input, so a
converter change invalidates only the conversion and its downstream result.

## Validate the integration

Validate the module before registering or executing it:

```sh
once query validate-module modules/scripted_lint.star --format json
once query validate-workspace
once query schema scripted_lint
```

Probe its filesystem contract in a private validation sandbox:

```sh
once query validate-actions lint --capability lint
```

Run it twice and inspect the current normalized result and durable evidence:

```sh
once lint lint
once --format json lint lint
once query evidence lint --limit 2
```

The second unchanged run should reuse the report. Verify that findings contain
workspace-relative paths, stable rule identifiers, appropriate severities,
and enough message detail for a human or coding agent to repair them.

## Checklist

Before publishing or sharing a lint target kind, confirm that:

- the analyzer and adapter are invoked directly without a shell wrapper;
- every source, configuration file, adapter, plugin, and data file is an
  action input;
- the portable and native reports are declared outputs;
- cleanup and output directories are explicit action setup;
- the analyzer version and adapter behavior participate in action identity;
- only documented finding codes appear in `success_exit_codes`;
- invalid or missing reports fail instead of becoming empty successful runs;
- the target emits `once_lint_info` and the `lint` capability;
- a real starter can be materialized, validated, and executed;
- repeated unchanged execution demonstrates cache reuse.
