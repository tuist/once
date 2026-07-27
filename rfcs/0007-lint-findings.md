# Request for Comments 0007: Lint Findings

## Status

Accepted

## Motivation

Once can cache commands and expose typed build and test capabilities, but a
static analyzer has a different success model. Many analyzers return a nonzero
exit code when they found valid, actionable issues. Treating that code as an
execution failure discards the report and prevents caching.

Ecosystems also use different native report formats. A coding agent should not
need a separate parser and repair loop for each analyzer.

## Decision

Once adds a generic `lint` capability and a `once_lint_info` provider. Target
kinds keep native analyzer configuration authoritative and produce a
[Static Analysis Results Interchange Format report](https://docs.oasis-open.org/sarif/sarif/v2.1.0/sarif-v2.1.0.html).
The command line projects that report into `once.lint_results.v1`.

`run_action` accepts `success_exit_codes`, which defaults to `[0]`. A listed
code means the process completed and its declared outputs are valid. Once
captures and caches those outputs, then records the action as successful.
Unexpected codes remain execution failures.

`once lint <target>` prints the same normalized findings on fresh execution
and cache hits. `--fail-on` controls the lowest severity that makes the command
return a failing status. The default is `warning`.

Every analyzer integration lives in a Starlark target kind. Rust code knows the
portable report schema and normalized finding schema, but never recognizes an
ecosystem or analyzer by name.

## Provider contract

The reserved `once_lint_info` provider contains:

```python
{
    "schema": "once.lint_info.v1",
    "target": ctx["label"]["id"],
    "analyzer": {
        "type": "example",
        "display_name": "Example",
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
}
```

The normalized result records target, completion state, counts by severity,
stable findings, and native report artifacts. Finding locations use
workspace-relative paths.

## Initial analyzers

The built-in prelude includes target kinds for Ruff, ESLint, golangci-lint,
SwiftLint, detekt, Credo, and RuboCop. Integrations use native portable reports
when available. ESLint and RuboCop use small target-kind-owned adapters from
their public machine-readable reports.

Clippy, clang-tidy, ShellCheck, and Zig formatting require dedicated adapters
or a stronger compilation graph contract. They should reuse this provider and
result shape instead of adding analyzer-specific command surfaces.

## Non-goals

- A central lint scheduler across every target
- Automatic source rewriting
- Replacing native analyzer configuration
- Baseline storage or changed-finding comparison

The normalized finding and fingerprint fields leave room for a later
regression gate without changing analyzer target kinds.
