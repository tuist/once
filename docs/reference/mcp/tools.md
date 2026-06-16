# MCP Tools

Every tool the `once mcp` server advertises in `tools/list`, with the input schema it validates against and a worked example of what the call returns. The catalog is generated from the same record the runtime serves, so the names, descriptions, and schemas can't drift.

## `once_query_targets`

List every declared target in the workspace, optionally filtered by rule kind.

Returns the same record shape as `once query targets --format json`: one entry per declared target with its canonical id, package, name, rule kind, dep edges, and the capabilities it exposes. The optional `kind` argument narrows results to a single rule.

**Input schema**

```json
{
  "properties": {
    "kind": {
      "description": "Restrict results to targets of this rule kind (e.g. `apple_library`).",
      "type": "string"
    }
  },
  "type": "object"
}
```

**Example return**

```json
[
  { "id": "apps/ios/AppCore", "package": "apps/ios", "name": "AppCore",
    "kind": "apple_library", "deps": [], "capabilities": ["build"] },
  { "id": "apps/ios/Greeter", "package": "apps/ios", "name": "Greeter",
    "kind": "apple_library", "deps": ["apps/ios/AppCore"],
    "capabilities": ["build"] }
]
```

## `once_query_capabilities`

Return the capabilities (`build`, `run`, `test`) a target exposes, with their output groups and required inputs.

Returns the same record `once query capabilities <target> --format json` emits: the target's id and kind plus one entry per capability with its output groups (what running the capability produces) and required outputs (what it depends on having built).

**Input schema**

```json
{
  "properties": {
    "target": {
      "description": "Canonical target id, e.g. `apps/ios/App`.",
      "type": "string"
    }
  },
  "required": [
    "target"
  ],
  "type": "object"
}
```

**Example return**

```json
{
  "id": "apps/ios/App",
  "kind": "apple_application",
  "capabilities": [
    { "name": "build", "output_groups": ["default", "bundle", "dsyms"],
      "requires_outputs": [] },
    { "name": "run", "output_groups": ["default"],
      "requires_outputs": ["bundle"] }
  ]
}
```

## `once_query_schema`

Return the typed contract for a rule kind: attributes, dep edges, providers, capabilities, and runnable starter examples.

Returns the rule schema (the typed contract a target of that kind must match) as `once query schema <kind> --format json` would. The record carries the rule's documentation, attribute list (with types, required flag, and whether the attribute is configurable), expected dep providers, emitted providers, exposed capabilities, and a list of runnable starter examples. Each example bundles a slug, a one-line `use_when` hint, and the full file tree (`once.toml` plus source files) a caller would copy to get a working target.

**Input schema**

```json
{
  "properties": {
    "kind": {
      "description": "Rule kind to introspect, e.g. `apple_library`.",
      "type": "string"
    }
  },
  "required": [
    "kind"
  ],
  "type": "object"
}
```

**Example return**

```json
{
  "kind": "apple_library",
  "docs": "Mixed Swift, Objective-C, C, and C++ static library...",
  "attrs": [
    { "name": "platform", "ty": "string", "required": true, "configurable": true }
  ],
  "capabilities": [ { "name": "build", "output_groups": ["archive"], "requires_outputs": [] } ],
  "providers": ["apple_linkable", "apple_module"],
  "examples": [
    {
      "slug": "apple-library-minimal",
      "name": "Minimal Apple library",
      "use_when": "...",
      "files": [
        { "path": "apps/Hello/once.toml", "contents": "[[target]]\nname = \"Hello\"\nkind = \"apple_library\"\n..." }
      ]
    }
  ]
}
```

## `once_list_rules`

List every rule kind the registry knows about, with its one-line docs and example slugs.

Lightweight discovery entry point. Returns one entry per rule kind containing the rule's documentation and the slugs of its bundled starter examples. Use this to discover what kinds of targets are buildable in the workspace before calling `once_query_schema` for the full contract of a chosen rule.

**Input schema**

```json
{
  "properties": {},
  "type": "object"
}
```

**Example return**

```json
[
  {
    "kind": "apple_library",
    "docs": "Mixed Swift, Objective-C, C, and C++ static library...",
    "examples": [
      { "slug": "apple-library-minimal", "name": "Minimal Apple library", "use_when": "..." }
    ]
  }
]
```

## `once_get_target`

Return the resolved view of a single target: rule kind, srcs, deps, typed attrs, capabilities, providers.

Returns the same `GraphTarget` record `once_query_targets` emits, scoped to one target id. Includes the target's typed attribute values (with the types declared by its rule schema), the capabilities it exposes, the providers it emits, and any diagnostics emitted while loading the manifest. Use this before editing a target to learn its current shape.

**Input schema**

```json
{
  "properties": {
    "target": {
      "description": "Canonical target id, e.g. `apps/Hello/Hello`.",
      "type": "string"
    }
  },
  "required": [
    "target"
  ],
  "type": "object"
}
```

**Example return**

```json
{
  "label": { "package": "apps/Hello", "name": "Hello", "id": "apps/Hello/Hello" },
  "kind": "apple_library",
  "srcs": ["Sources/**/*.swift"],
  "deps": [],
  "attrs": { "platform": "ios", "minimum_os": "17.0" },
  "capabilities": [ { "name": "build", "output_groups": ["default", "binary"], "requires_outputs": [] } ],
  "providers": ["apple_linkable", "apple_module"]
}
```

## `once_query_tests`

List targets that expose Once's generic test capability.

Returns every target with a `test` capability, including its rule kind, dependencies, runner type when the rule exposes `once_test_info`, labels, and normalized result path. Use this as the agent-native test discovery entry point before running or filtering tests.

**Input schema**

```json
{
  "properties": {},
  "type": "object"
}
```

**Example return**

```json
[
  {
    "id": "spec/cli_e2e",
    "kind": "shellspec_test",
    "deps": [],
    "runner": "shellspec",
    "labels": ["e2e"],
    "results_path": ".once/out/spec/cli_e2e/test/test_results.json"
  }
]
```

## `once_query_affected_tests`

Return test targets likely affected by a set of changed workspace paths.

Maps changed paths to test targets using generic graph relationships and declared inputs. A test is affected when a changed path belongs to the test target itself or to one of its declared dependencies. The query does not know about ShellSpec, Python, Android, or any native runner.

**Input schema**

```json
{
  "properties": {
    "changed_paths": {
      "description": "Workspace-relative changed paths. An empty list returns every test target.",
      "items": {
        "type": "string"
      },
      "type": "array"
    }
  },
  "type": "object"
}
```

**Example return**

```json
[
  {
    "id": "spec/cli_e2e",
    "kind": "shellspec_test",
    "reasons": ["changed test input `spec/cli_spec.sh`"]
  }
]
```

## `once_run_tests`

Run test targets by id, or run tests affected by changed workspace paths.

Executes Once's generic `test` capability for either explicit `target` / `targets` or the targets selected by `changed_paths`. This is the MCP-native edit verification loop for coding harnesses: call `once_query_affected_tests` to preview selection, call `once_run_tests` to execute, then read the normalized `once.test_results.v1` results included in each run record. Failed tests are returned as normal tool content with `success: false` rather than a tool protocol error, so agents can inspect failures and iterate.

**Input schema**

```json
{
  "properties": {
    "changed_paths": {
      "description": "Workspace-relative changed paths. Used only when no explicit target is supplied; an empty list runs every discovered test target.",
      "items": {
        "type": "string"
      },
      "type": "array"
    },
    "target": {
      "description": "Single canonical target id to run, e.g. `spec/cli_e2e`.",
      "type": "string"
    },
    "targets": {
      "description": "Canonical target ids to run. Used with `target`, this is deduplicated before execution.",
      "items": {
        "type": "string"
      },
      "type": "array"
    }
  },
  "type": "object"
}
```

**Example return**

```json
{
  "runs": [
    {
      "target": "spec/cli_e2e",
      "exit_code": 0,
      "success": true,
      "record": { "target": "spec/cli_e2e", "capability": "test" },
      "results": { "schema": "once.test_results.v1", "status": "passed" },
      "stderr": ""
    }
  ]
}
```

## `once_query_test_results`

Read normalized once.test_results.v1 results for a target.

Reads the normalized result file produced by the target's `test` capability. This is the stable agent-facing interface for pass/fail summaries, case-level failures, attempts, and artifacts; callers should not scrape native runner stdout or stderr.

**Input schema**

```json
{
  "properties": {
    "target": {
      "description": "Canonical target id, e.g. `spec/cli_e2e`.",
      "type": "string"
    }
  },
  "required": [
    "target"
  ],
  "type": "object"
}
```

**Example return**

```json
{
  "schema": "once.test_results.v1",
  "target": "spec/cli_e2e",
  "status": "passed",
  "summary": { "total": 2, "passed": 2, "failed": 0 },
  "cases": []
}
```

## `once_build_target`

Build a target by running its generic `build` capability.

Opt-in tool exposed only when the MCP server starts with `once mcp --allow-run`. Executes the same path as `once build <target> --format json`, so dependency traversal, rule-declared actions, cache policy, and output groups stay owned by the CLI and rule graph. The tool returns stdout parsed as JSON when possible, along with exit status and stderr. A failed build is returned as normal tool content with `success: false` so agents can inspect diagnostics.

**Input schema**

```json
{
  "properties": {
    "target": {
      "description": "Target id to build, e.g. `apps/ios/App` or `./App`.",
      "type": "string"
    }
  },
  "required": [
    "target"
  ],
  "type": "object"
}
```

**Example return**

```json
{
  "target": "apps/ios/App",
  "capability": "build",
  "exit_code": 0,
  "success": true,
  "record": {
    "target": "apps/ios/App",
    "kind": "apple_application",
    "capability": "build",
    "cache": "miss",
    "outputs": [".once/out/apps/ios/App/App.app"]
  },
  "stderr": ""
}
```

## `once_run_target`

Run a target by executing its generic `run` capability.

Opt-in tool exposed only when the MCP server starts with `once mcp --allow-run`. Executes the same path as `once run <target> --format json`, including any prerequisite build outputs declared by the target's `run` capability. Rule-declared execution policy is preserved, so uncacheable actions are executed instead of replayed from the action cache. The tool returns stdout parsed as JSON when possible, plus exit status and stderr.

**Input schema**

```json
{
  "properties": {
    "target": {
      "description": "Target id to run, e.g. `apps/ios/App` or `./App`.",
      "type": "string"
    }
  },
  "required": [
    "target"
  ],
  "type": "object"
}
```

**Example return**

```json
{
  "target": "apps/ios/App",
  "capability": "run",
  "exit_code": 0,
  "success": true,
  "record": {
    "target": "apps/ios/App",
    "kind": "apple_application",
    "capability": "run",
    "cache": "bypass",
    "outputs": [".once/out/apps/ios/App/run/run.json"]
  },
  "stderr": ""
}
```

## `once_start_target`

Start a target in a persisted runtime session and return its session id.

Opt-in tool exposed only when the MCP server starts with `once mcp --allow-run`. Starts `once run` under a runtime supervisor, persists stdout and stderr under `.once/runtime/<session_id>/`, and returns immediately with the session record. Use the runtime status, logs, and stop tools to follow the process.

**Input schema**

```json
{
  "properties": {
    "target": {
      "description": "Target id to start, e.g. `tools/demo/LaunchApp` or `./LaunchApp`.",
      "type": "string"
    }
  },
  "required": [
    "target"
  ],
  "type": "object"
}
```

**Example return**

```json
{
  "session_id": "tools-demo-LaunchApp-123-1812345678901",
  "target": "tools/demo/LaunchApp",
  "status": "starting",
  "session_dir": ".once/runtime/tools-demo-LaunchApp-123-1812345678901",
  "stdout": ".once/runtime/tools-demo-LaunchApp-123-1812345678901/stdout.log",
  "stderr": ".once/runtime/tools-demo-LaunchApp-123-1812345678901/stderr.log"
}
```

## `once_runtime_status`

Return the latest persisted status for a runtime session.

Reads `.once/runtime/<session_id>/session.json` and returns the supervisor's latest status. Status values include `starting`, `running`, `stopping`, `stopped`, `exited`, and `failed`.

**Input schema**

```json
{
  "properties": {
    "session_id": {
      "description": "Session id returned by `once_start_target`.",
      "type": "string"
    }
  },
  "required": [
    "session_id"
  ],
  "type": "object"
}
```

**Example return**

```json
{
  "session_id": "tools-demo-LaunchApp-123-1812345678901",
  "target": "tools/demo/LaunchApp",
  "status": "running",
  "pid": 4242
}
```

## `once_runtime_logs`

Read stdout or stderr records for a runtime session.

Reads persisted line-oriented stdout and stderr records from a runtime session. Pass `source` to restrict to `stdout` or `stderr`, and pass a previous `cursor` to read only newer records.

**Input schema**

```json
{
  "properties": {
    "cursor": {
      "description": "Cursor returned by a previous log record.",
      "type": "string"
    },
    "limit": {
      "description": "Maximum number of records to return.",
      "type": "integer"
    },
    "session_id": {
      "description": "Session id returned by `once_start_target`.",
      "type": "string"
    },
    "source": {
      "description": "`stdout` or `stderr`.",
      "enum": [
        "stdout",
        "stderr"
      ],
      "type": "string"
    }
  },
  "required": [
    "session_id"
  ],
  "type": "object"
}
```

**Example return**

```json
{
  "session_id": "tools-demo-LaunchApp-123-1812345678901",
  "records": [
    { "cursor": "stdout:000000000000", "source": "stdout", "level": "info", "message": "ready" }
  ]
}
```

## `once_stop_runtime`

Request that a runtime session stop.

Writes a stop request into the runtime session directory. The supervisor observes the request, kills the child process, and updates `session.json` to `stopped`.

**Input schema**

```json
{
  "properties": {
    "session_id": {
      "description": "Session id returned by `once_start_target`.",
      "type": "string"
    }
  },
  "required": [
    "session_id"
  ],
  "type": "object"
}
```

**Example return**

```json
{
  "session_id": "tools-demo-LaunchApp-123-1812345678901",
  "target": "tools/demo/LaunchApp",
  "status": "stopping"
}
```

## `once_validate_target`

Validate a proposed `[[target]]` table against its rule schema. Returns structured diagnostics instead of prose.

Schema-only validation: checks that the target declares a known rule kind, every required attribute is present, every declared attribute is known to the rule and matches the rule's declared type, and the target name is well-formed. The check is local; it does not resolve dep references or read other manifests. Returns `{ valid: true }` on success or `{ valid: false, diagnostics: [...] }` where each diagnostic carries a stable `code`, the offending `target` id, the offending `attribute` when applicable, and `repairs` an agent can apply.

**Input schema**

```json
{
  "properties": {
    "target": {
      "description": "Raw `[[target]]` table shape with `name`, `kind`, optional `deps`, `srcs`, and `attrs`.",
      "type": "object"
    }
  },
  "required": [
    "target"
  ],
  "type": "object"
}
```

**Example return**

```json
{
  "valid": false,
  "diagnostics": [
    {
      "code": "missing_required_attr",
      "message": "rule `apple_library` requires attribute `platform`",
      "target": "Hello",
      "attribute": "platform"
    }
  ]
}
```

## `once_apply_edit`

Apply a batch of `create` / `update` / `delete` operations to one `once.toml` atomically.

Reads the manifest at `<workspace>/<package>/once.toml` (creating it if missing), applies the batch of operations against the in-memory document, and writes the result back only if every operation succeeds. Returns `{ applied: true, path: <manifest path> }` on success or `{ applied: false, diagnostics: [...] }` with the structured diagnostic shape used by `once_validate_target`. The original file is never partially modified.

**Input schema**

```json
{
  "properties": {
    "operations": {
      "description": "Ordered list of operations. Each is `{ op: \"create\", target: {...} }`, `{ op: \"update\", target_name: \"...\", set: {...} }`, or `{ op: \"delete\", target_name: \"...\" }`.",
      "items": {
        "type": "object"
      },
      "type": "array"
    },
    "package": {
      "description": "Package directory relative to the workspace root, e.g. `apps/Hello`. Use `\"\"` for the root manifest.",
      "type": "string"
    }
  },
  "required": [
    "package",
    "operations"
  ],
  "type": "object"
}
```

**Example return**

```json
{
  "applied": true,
  "path": "apps/Hello/once.toml"
}
```

