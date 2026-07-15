# Model Context Protocol Tools

Every tool the [`once mcp`](/reference/cli/mcp) [Model Context Protocol](https://modelcontextprotocol.io/) server advertises in `tools/list`, with its input schema and a worked return example.

## `once_query_targets`

List every declared target in the workspace, optionally filtered by target kind.

Returns the same record shape as `once query targets --format json`: one entry per declared target with its canonical id, package, name, target kind, dep edges, and the capabilities it exposes. The optional `kind` argument narrows results to one target kind.

**Input schema**

```json
{
  "properties": {
    "kind": {
      "description": "Restrict results to a target kind discovered through `once_list_target_kinds`.",
      "type": "string"
    }
  },
  "type": "object"
}
```

**Example return**

```json
[
  { "id": "packages/core/Core", "package": "packages/core", "name": "Core",
    "kind": "library", "deps": [], "capabilities": ["build"] },
  { "id": "apps/service/Service", "package": "apps/service", "name": "Service",
    "kind": "application", "deps": ["packages/core/Core"],
    "capabilities": ["build", "run"] }
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
      "description": "Canonical target id, such as `apps/service/Service`.",
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
  "id": "apps/service/Service",
  "kind": "application",
  "capabilities": [
    { "name": "build", "output_groups": ["default", "package"],
      "requires_outputs": [] },
    { "name": "run", "output_groups": ["default"],
      "requires_outputs": ["package"] }
  ]
}
```

## `once_query_schema`

Return the typed contract for a target kind: attributes, dep edges, providers, capabilities, and runnable starter examples.

Returns the target kind schema (the typed contract a target of that kind must match) as `once query schema <kind> --format json` would. The record carries the target kind's documentation, attribute list (with types, required flag, and whether the attribute is configurable), expected dep providers, emitted providers, exposed capabilities, and a lightweight list of runnable starter examples. Use `once_query_example` to fetch the full file tree for a chosen example.

**Input schema**

```json
{
  "properties": {
    "kind": {
      "description": "Target kind to introspect. Discover names with `once_list_target_kinds`.",
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
  "kind": "library",
  "docs": "Reusable library target...",
  "attrs": [
    { "name": "visibility", "ty": "string", "required": true, "configurable": false }
  ],
  "capabilities": [ { "name": "build", "output_groups": ["default"], "requires_outputs": [] } ],
  "providers": ["linkable", "module"],
  "examples": [
    {
      "slug": "library-minimal",
      "name": "Minimal library",
      "use_when": "..."
    }
  ]
}
```

## `once_query_example`

Return the complete file bundle for one target kind starter example.

Returns the same record as `once query example <kind> <slug> --format json`: the selected example's slug, name, selection hint, and every text file a caller can copy to create the starter workspace. Example descriptors are discovered through `once_list_target_kinds` or `once_query_schema`; this tool fetches the file payload only after a caller chooses one.

**Input schema**

```json
{
  "properties": {
    "kind": {
      "description": "Target kind that owns the example.",
      "type": "string"
    },
    "slug": {
      "description": "Example slug from the target kind schema.",
      "type": "string"
    }
  },
  "required": [
    "kind",
    "slug"
  ],
  "type": "object"
}
```

**Example return**

```json
{
  "slug": "library-minimal",
  "name": "Minimal library",
  "use_when": "...",
  "files": [
    { "path": "packages/core/once.toml", "contents": "[[target]]\nname = \"Core\"\nkind = \"library\"\n..." }
  ]
}
```

## `once_list_target_kinds`

List every target kind available in the workspace, with its one-line docs and example slugs.

Lightweight discovery entry point. Returns one entry per target kind containing the target kind's documentation and the slugs of its bundled starter examples. Use this to discover what kinds of targets are buildable in the workspace before calling `once_query_schema` for the full contract of a chosen target kind.

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
    "kind": "library",
    "docs": "Reusable library target...",
    "examples": [
      { "slug": "library-minimal", "name": "Minimal library", "use_when": "..." }
    ]
  }
]
```

## `once_get_target`

Return the resolved view of a single target: target kind, srcs, deps, typed attrs, capabilities, providers.

Returns the same `GraphTarget` record `once_query_targets` emits, scoped to one target id. Includes the target's typed attribute values (with the types declared by its target kind schema), the capabilities it exposes, the providers it emits, and any diagnostics emitted while loading the manifest. Use this before editing a target to learn its current shape.

**Input schema**

```json
{
  "properties": {
    "target": {
      "description": "Canonical target id, such as `packages/core/Core`.",
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
  "label": { "package": "packages/core", "name": "Core", "id": "packages/core/Core" },
  "kind": "library",
  "srcs": ["src/**/*.src"],
  "deps": [],
  "attrs": { "visibility": "public" },
  "capabilities": [ { "name": "build", "output_groups": ["default"], "requires_outputs": [] } ],
  "providers": ["linkable", "module"]
}
```

## `once_query_tests`

List targets that expose Once's generic test capability.

Returns every target with a `test` capability, including its target kind, dependencies, runner type when the target kind exposes `once_test_info`, labels, and normalized result path. Use this as the agent test discovery entry point before running or filtering tests.

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
    "id": "tests/unit",
    "kind": "test_suite",
    "deps": [],
    "runner": "unit",
    "labels": ["fast"],
    "results_path": ".once/out/tests/unit/test/test_results.json"
  }
]
```

## `once_query_affected_tests`

Return test targets likely affected by a set of changed workspace paths.

Maps changed paths to test targets using graph relationships and declared inputs. A test is affected when a changed path belongs to the test target itself or to one of its declared dependencies. Selection does not depend on a particular test runner.

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
    "id": "tests/unit",
    "kind": "test_suite",
    "reasons": ["changed test input `tests/unit/spec.src`"]
  }
]
```

## `once_run_tests`

Run test targets by id, or run tests affected by changed workspace paths.

Executes Once's generic `test` capability for either explicit `target` / `targets` or the targets selected by `changed_paths`. To verify an edit, call `once_query_affected_tests` to preview selection, call `once_run_tests` to execute, then read the normalized `once.test_results.v1` results included in each run record. Failed tests are returned as normal tool content with `success: false` rather than a tool protocol error, so agents can inspect failures and iterate.

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
      "description": "Single canonical target id to run, such as `tests/unit`.",
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
      "target": "tests/unit",
      "exit_code": 0,
      "success": true,
      "record": { "target": "tests/unit", "capability": "test" },
      "results": { "schema": "once.test_results.v1", "status": "passed" },
      "stderr": ""
    }
  ]
}
```

## `once_query_test_results`

Read normalized once.test_results.v1 results for a target.

Reads the normalized result file produced by the target's `test` capability. This is the stable agent-facing interface for pass or fail summaries, case-level failures, attempts, and artifacts. Callers do not need to parse a runner's command output.

**Input schema**

```json
{
  "properties": {
    "target": {
      "description": "Canonical target id, such as `tests/unit`.",
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
  "target": "tests/unit",
  "status": "passed",
  "summary": { "total": 2, "passed": 2, "failed": 0 },
  "cases": []
}
```

## `once_query_evidence`

List durable evidence records, optionally filtered by subject.

Returns the same record shape as `once query evidence --format json`: durable action evidence captured after `once exec`, `once run`, `once build`, or `once test`. Pass `subject` to filter to one command action, target, or target capability, such as `cli` or `cli:test`.

**Input schema**

```json
{
  "properties": {
    "subject": {
      "description": "Optional subject id or subject-capability pair, such as `cli` or `cli:test`.",
      "type": "string"
    }
  },
  "type": "object"
}
```

**Example return**

```json
[
  {
    "schema": "once.evidence.v1",
    "id": "8d65122cd9dcddc8d5d9a8458ff42a40fe3dd7acbd4e0563fd7f9e8fb19b0c44",
    "kind": "action_result",
    "subject": { "kind": "target", "id": "cli", "capability": "test" },
    "status": "passed",
    "action_digest": "0476bde2e7d8d1a64d9bd6f589ef5b443d0f60b71e2ad6f1c5bd7a2c4c41223f",
    "input_digest": "8ed3f6ad685b959ead7022518e1af76cd816f8e8ec7ccd5f5814ccfb820e6a41",
    "cache": "miss",
    "exit_code": 0,
    "stdout": "b439bb065d84034c2e7172c1709eb28797c9bd7f2c64c5d1a1d9c1118f6f9d7e",
    "created_at_unix_ms": 1812345678901
  }
]
```

## `once_build_target`

Build a target by running its generic `build` capability.

This tool is available only when the server starts with `once mcp --allow-run`. It behaves like `once build <target> --format json`, including dependency traversal, target actions, cache policy, and output groups. The result includes the exit status, standard error, and standard output parsed as structured data when possible. A failed build is returned as normal tool content with `success: false` so agents can inspect diagnostics.

**Input schema**

```json
{
  "properties": {
    "target": {
      "description": "Target id to build, such as `apps/service/Service` or `./Service`.",
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
  "target": "apps/service/Service",
  "capability": "build",
  "exit_code": 0,
  "success": true,
  "record": {
    "target": "apps/service/Service",
    "kind": "application",
    "capability": "build",
    "cache": "miss",
    "outputs": [".once/out/apps/service/Service/package"]
  },
  "stderr": ""
}
```

## `once_run_target`

Run a target by executing its generic `run` capability.

This tool is available only when the server starts with `once mcp --allow-run`. It behaves like `once run <target> --format json`, including prerequisite build outputs declared by the target's `run` capability. Set `visible` to request a visible interface when the target kind supports one. Uncacheable actions run again instead of replaying an action-cache hit. The result includes the exit status, standard error, and standard output parsed as structured data when possible.

**Input schema**

```json
{
  "properties": {
    "target": {
      "description": "Target id to run, such as `apps/service/Service` or `./Service`.",
      "type": "string"
    },
    "visible": {
      "description": "Request a visible runtime interface when the target kind supports one.",
      "type": "boolean"
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
  "target": "apps/service/Service",
  "capability": "run",
  "exit_code": 0,
  "success": true,
  "record": {
    "target": "apps/service/Service",
    "kind": "application",
    "capability": "run",
    "cache": "bypass",
    "outputs": [".once/out/apps/service/Service/run.json"]
  },
  "stderr": ""
}
```

## `once_start_target`

Start a target in a persisted runtime session and return its session id.

This tool is available only when the server starts with `once mcp --allow-run`. It starts the target, saves its standard output and standard error under `.once/runtime/<session_id>/`, and returns immediately with the session record. Use the status, logs, and stop tools to follow the process.

**Input schema**

```json
{
  "properties": {
    "target": {
      "description": "Target id to start, such as `tools/demo/LaunchService` or `./LaunchService`.",
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
  "session_id": "tools-demo-LaunchService-123-1812345678901",
  "target": "tools/demo/LaunchService",
  "status": "starting",
  "session_dir": ".once/runtime/tools-demo-LaunchService-123-1812345678901",
  "stdout": ".once/runtime/tools-demo-LaunchService-123-1812345678901/stdout.log",
  "stderr": ".once/runtime/tools-demo-LaunchService-123-1812345678901/stderr.log"
}
```

## `once_runtime_status`

Return the latest persisted status for a runtime session.

Reads `.once/runtime/<session_id>/session.json` and returns the latest status. Status values include `starting`, `running`, `stopping`, `stopped`, `exited`, and `failed`.

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
  "session_id": "tools-demo-LaunchService-123-1812345678901",
  "target": "tools/demo/LaunchService",
  "status": "running",
  "pid": 4242
}
```

## `once_runtime_logs`

Read standard output or standard error records for a runtime session.

Reads persisted line-oriented standard output and standard error records from a runtime session. Pass `source` to restrict the result to `stdout` or `stderr`, and pass a previous `cursor` to read only newer records.

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
  "session_id": "tools-demo-LaunchService-123-1812345678901",
  "records": [
    { "cursor": "stdout:000000000000", "source": "stdout", "level": "info", "message": "ready" }
  ]
}
```

## `once_stop_runtime`

Request that a runtime session stop.

Requests that the process stop and updates the session status as the request is handled.

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
  "session_id": "tools-demo-LaunchService-123-1812345678901",
  "target": "tools/demo/LaunchService",
  "status": "stopping"
}
```

## `once_validate_target`

Validate a proposed `[[target]]` table against its target kind schema. Returns structured diagnostics instead of prose.

Schema-only validation: checks that the target declares a known target kind, every required attribute is present, every declared attribute is known to the target kind and matches the target kind's declared type, and the target name is well-formed. The check is local; it does not resolve dep references or read other manifests. Returns `{ valid: true }` on success or `{ valid: false, diagnostics: [...] }` where each diagnostic carries a stable `code`, the offending `target` id, the offending `attribute` when applicable, and `repairs` an agent can apply.

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
      "message": "target kind `library` requires attribute `visibility`",
      "target": "Core",
      "attribute": "visibility"
    }
  ]
}
```

## `once_apply_edit`

Apply a batch of `create` / `update` / `delete` operations to one `once.toml` atomically.

Reads the manifest at `<workspace>/<package>/once.toml`, creating it if needed, and applies the operations only if every operation succeeds. Returns `{ applied: true, path: <manifest path> }` on success or `{ applied: false, diagnostics: [...] }` with the structured diagnostic shape used by `once_validate_target`. A failed batch leaves the original file unchanged.

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
      "description": "Package directory relative to the workspace root, such as `packages/core`. Use `\"\"` for the root manifest.",
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
  "path": "packages/core/once.toml"
}
```

