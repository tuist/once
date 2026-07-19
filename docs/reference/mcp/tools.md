# Model Context Protocol Tools

Every tool the [`once mcp`](/reference/cli/mcp) [Model Context Protocol](https://modelcontextprotocol.io/) server advertises in `tools/list`, with its input schema and a worked return example.

## `once_query_targets`

List every declared target in the workspace, optionally filtered by target kind.

Returns the same record shape as `once query targets --format json`: one entry per declared target with its canonical id, package, name, target kind, default dependencies, named dependency roles, and exposed capabilities. The optional `kind` argument narrows results to one target kind.

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
    "kind": "library", "deps": [], "dependency_edges": {}, "capabilities": ["build"] },
  { "id": "apps/service/Service", "package": "apps/service", "name": "Service",
    "kind": "application", "deps": ["packages/core/Core"],
    "dependency_edges": { "plugins": ["tools/compiler/Plugin"] },
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

Return the typed contract for a target kind: attributes, dep edges, providers, capabilities, source references, and runnable starters.

Returns the target kind schema (the typed contract a target of that kind must match) as `once query schema <kind> --format json` would. The record carries the target kind's documentation, attribute list (with types, required flag, and whether the attribute is configurable), expected dep providers, emitted providers, exposed capabilities, external source concepts that can guide partial adoption, and a lightweight list of runnable starter examples. Use `once_query_example` to fetch the full file tree for a chosen example.

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
  "source_references": [
    { "system": "Example Build", "symbol": "example_library",
      "url": "https://example.com/example_library", "use_when": "...",
      "content_digest": "..." }
  ],
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

List target kinds with their docs, external source references, and example slugs, optionally filtered by ecosystem or intent.

Lightweight discovery entry point. Returns matching target kinds with documentation, external build-system concepts they can partially replace, and bundled starter examples. When the request names one or more ecosystems or runner families, include all of their names in the short `query` copied from the request. Once combines those specific matches while ignoring generic intent words, which lets a harness discover every native test integration in a mixed repository with one call. Omit the query when the intent is unknown. Call `once_query_schema` for the full contract of each chosen target kind. The matching command-line operation is `once query target-kinds --query <text> --format json`.

**Input schema**

```json
{
  "properties": {
    "query": {
      "description": "Short ecosystem, target-kind family, or intent text copied from the user's request.",
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
    "kind": "library",
    "docs": "Reusable library target...",
    "source_references": [
      { "system": "Example Build", "symbol": "example_library",
        "url": "https://example.com/example_library", "use_when": "...",
        "content_digest": "..." }
    ],
    "examples": [
      { "slug": "library-minimal", "name": "Minimal library", "use_when": "..." }
    ]
  }
]
```

## `once_query_module_contract`

Return the complete project-module authoring contract, generic analysis and action primitives, maintenance invariants, and a starter module.

Use this when no discovered target kind covers an external rule or plugin. The result contains the exact Starlark declaration helpers, schema invariants, implementation context fields, generic host-analysis and action primitives, module registration snippet, maintenance loop, and a runnable starter. A coding harness can use it to author and maintain a project-local target kind without waiting for a built-in integration. The matching command-line operation is `once query module-contract --format json`.

**Input schema**

```json
{
  "properties": {},
  "type": "object"
}
```

**Example return**

```json
{
  "language": "Starlark",
  "registration": "[modules]\npaths = [\"modules/*.star\"]\n",
  "schema_invariants": ["attr.default is optional schema documentation and must be a string..."],
  "context_fields": [
    { "signature": "ctx[\"attr\"]", "purpose": "Typed target attributes." }
  ],
  "action_primitives": [
    { "signature": "write_path(path, content)", "purpose": "Declare a portable file-writing action." },
    { "signature": "materialize_host_file(source, destination)", "purpose": "Snapshot a content-verified absolute host toolchain file into a workspace output." }
  ],
  "starter": "def _generated_text_impl(ctx): ..."
}
```

## `once_fetch_external_source`

Fetch bounded UTF-8 source code, metadata, or documentation from a public HTTPS address.

Fetches an authoritative external rule, plugin, registry record, or build-system reference for a coding harness to inspect before generating a local Once target kind. Only public HTTPS addresses are accepted, redirects are not followed, and response content is bounded to one mebibyte. The result includes the content, media type, digest, byte count, and truncation state. The matching command-line operation is `once query external-source <url> --format json`.

**Input schema**

```json
{
  "properties": {
    "max_bytes": {
      "default": 262144,
      "description": "Maximum response bytes to return.",
      "maximum": 1048576,
      "minimum": 1,
      "type": "integer"
    },
    "url": {
      "description": "Public HTTPS address for external source code, metadata, or documentation.",
      "type": "string"
    }
  },
  "required": [
    "url"
  ],
  "type": "object"
}
```

**Example return**

```json
{
  "url": "https://example.com/rules/example.rule",
  "content_type": "text/plain",
  "content_digest": "...",
  "byte_count": 4120,
  "truncated": false,
  "content": "rule implementation..."
}
```

## `once_validate_module`

Validate a project-local Starlark module and return its target kind contracts before registration or execution.

Reads one workspace-relative module file, evaluates it with the public Once declarations and generic primitives, and returns either its discovered target kind schemas or a structured repair diagnostic. Use it after a harness writes or updates an external-rule adaptation and before registering targets that depend on it. The matching command-line operation is `once query validate-module <path> --format json`.

**Input schema**

```json
{
  "properties": {
    "path": {
      "description": "Workspace-relative path to a Starlark module file.",
      "type": "string"
    }
  },
  "required": [
    "path"
  ],
  "type": "object"
}
```

**Example return**

```json
{
  "valid": true,
  "path": "modules/generated_text.star",
  "target_kinds": [
    { "kind": "generated_text", "providers": ["generated_file"], "capabilities": [ { "name": "build", "output_groups": ["default"] } ] }
  ],
  "diagnostics": []
}
```

## `once_get_target`

Return one resolved target with its sources, dependency roles, typed attributes, capabilities, and providers.

Returns the same `GraphTarget` record `once_query_targets` emits, scoped to one target id. Includes default dependencies in `deps`, named roles in `dependency_edges`, typed attribute values, exposed capabilities, emitted providers, and manifest diagnostics. Use this before editing a target to learn its current shape.

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
  "dependency_edges": { "plugins": ["tools/compiler/Plugin"] },
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

Maps changed paths to test targets using graph relationships and declared inputs. A test is affected when a changed path belongs to the test target itself or to one of its declared dependencies. Declared source patterns are matched without requiring the changed file to still exist. Changes to manifests, configured graph modules, and paths without a declared owner conservatively select every test. Selection does not depend on a particular test runner.

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

## `once_query_test_plan`

Create an immutable test plan without assigning work to runners.

Returns the selection policy, normalized changed paths, unmatched paths, selected tests, and stable execution batches. The plan deliberately contains no local worker, remote provider, or fixed-job assignment, so scheduling can change without changing test identity or invalidating reusable results. Before a target's first complete run, or when its discovery inputs change, the plan intentionally contains one whole-target batch. Run that target once with `once_run_tests`, inspect `once_query_test_manifest`, then query the plan again to see automatic file or case batches. Pass `target` with a `test_unit` from the manifest to create an exact unit-filtered plan. Planning rejects targets that do not declare exact filtering and units absent from the persisted whole-target manifest. The matching command-line operations are `once query test-plan --changed-path <path> --format json` and `once query test-plan --target <target> --test-unit <unit> --format json`.

**Input schema**

```json
{
  "properties": {
    "changed_paths": {
      "description": "Workspace-relative changed paths. An empty list creates a full test plan.",
      "items": {
        "type": "string"
      },
      "type": "array"
    },
    "target": {
      "description": "Explicit canonical test target. When set, changed paths are ignored.",
      "type": "string"
    },
    "test_unit": {
      "description": "One stable unit identifier from once_query_test_manifest. Requires target.",
      "type": "string"
    }
  },
  "type": "object"
}
```

**Example return**

```json
{
  "schema": "once.test_plan.v1",
  "id": "<stable digest>",
  "selection": {
    "schema": "once.test_selection.v1",
    "policy": { "mode": "affected", "safety": "conservative", "evidence": "declared_graph" },
    "changed_paths": ["src/lib.src"],
    "unmatched_paths": [],
    "tests": [{ "id": "tests/unit", "kind": "test_suite", "reasons": ["changed dependency `lib` input `src/lib.src`"] }]
  },
  "batches": [{ "id": "<stable digest>", "target": "tests/unit", "test_filters": [] }]
}
```

## `once_run_tests`

Run test targets by id, or run tests affected by changed workspace paths.

Creates the same immutable plan as `once_query_test_plan`, then pulls stable batches from a shared local queue. Batches with longer historical uncached durations are queued first, and idle workers dynamically take the next batch. Explicit `target` or `targets` produce an exact plan; otherwise `changed_paths` drive conservative graph selection. With exactly one target, `test_unit` runs a stable unit returned by `once_query_test_manifest` when the target kind declares filtering support. `jobs` caps workers without changing plan or batch identity. The result's `plan` is the work that just executed. Its `next_plan` is recomputed after complete runs refresh discovery, so use that field to assess file or case batching for the next run. The result also includes actual schedule attempts and normalized test results. Failed tests are returned as normal tool content with `success: false` rather than a tool protocol error, so agents can inspect failures and iterate. The matching command-line operations are `once test --changed-path <path> --jobs <count> --format json` and `once test <target> --test-unit <unit> --format json`.

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
    "jobs": {
      "description": "Maximum local workers. Defaults to available host parallelism and never changes plan or batch identity.",
      "maximum": 256,
      "minimum": 1,
      "type": "integer"
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
    },
    "test_unit": {
      "description": "Run one stable unit identifier returned by once_query_test_manifest. Requires exactly one explicit target.",
      "type": "string"
    }
  },
  "type": "object"
}
```

**Example return**

```json
{
  "plan": { "schema": "once.test_plan.v1", "id": "<executed plan digest>", "selection": {}, "batches": [] },
  "next_plan": { "schema": "once.test_plan.v1", "id": "<next plan digest>", "selection": {}, "batches": [] },
  "schedule": {
    "schema": "once.test_schedule.v1",
    "id": "<attempt-specific digest>",
    "plan_id": "<executed plan digest>",
    "strategy": "longest_estimated_duration_first_dynamic",
    "workers": 2,
    "attempts": [{ "batch_id": "<stable batch digest>", "placement": "local", "worker": "local-1", "status": "passed" }]
  },
  "runs": [
    {
      "batch_id": "<stable batch digest>",
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
  "runner": { "type": "native", "metadata": {} },
  "status": "passed",
  "summary": { "total": 1, "passed": 1, "failed": 0, "skipped": 0, "flaky": 0 },
  "cases": [{ "id": "tests/unit::case_name", "name": "case_name", "suite": "tests/unit", "status": "passed", "attempts": [{ "status": "passed" }], "runner_metadata": {} }],
  "artifacts": { "logs": [], "native_results": [] }
}
```

## `once_query_test_manifest`

List stable test units discovered in a target's normalized results.

Returns an immutable `once.test_manifest.v1` projection of the target's current normalized test results and its target-kind-declared listing and filtering support. When no results exist, the manifest reports `whole_target_fallback` with no units; run the whole target once to refresh discovery. Unit identifiers can be passed to `once_query_test_plan` or `once_run_tests` only when `case_filtering` is `runner_args`. The matching command-line operation is `once query test-manifest <target> --format json`.

**Input schema**

```json
{
  "properties": {
    "target": {
      "description": "Canonical target identifier, such as `tests/unit`.",
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
  "schema": "once.test_manifest.v1",
  "id": "<stable digest>",
  "target": "tests/unit",
  "runner": "native",
  "source": "normalized_results",
  "listing_supported": true,
  "case_filtering": "runner_args",
  "units": [{ "id": "tests/unit::case_name", "name": "case_name", "suite": "tests/unit" }]
}
```

## `once_query_test_attempts`

List persisted test batch attempts and measured durations.

Returns actual schedule attempts recorded by `once_run_tests` or `once test --changed-path`. Each record connects a stable plan and batch to its local worker, status, cache state, timestamps, measured duration, and the estimate used for ordering. Use `target` to inspect one test target and `limit` to bound history. The matching command-line operation is `once query test-attempts --target <target> --limit <count> --format json`.

**Input schema**

```json
{
  "properties": {
    "limit": {
      "default": 20,
      "description": "Newest matching attempts to return.",
      "maximum": 100,
      "minimum": 1,
      "type": "integer"
    },
    "target": {
      "description": "Optional canonical test target id.",
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
    "schema": "once.test_batch_attempt.v1",
    "plan_id": "<stable plan digest>",
    "batch_id": "<stable batch digest>",
    "target": "tests/unit",
    "placement": "local",
    "worker": "local-1",
    "duration_ms": 842,
    "status": "passed",
    "cache": "miss"
  }
]
```

## `once_query_evidence`

List durable evidence records, optionally filtered by subject.

Returns the same record shape as `once query evidence --format json`: durable action evidence captured after `once exec`, `once run`, `once build`, or `once test`. Pass `subject` to filter to one command action, target, or target capability, such as `cli` or `cli:test`. The tool returns the newest five matching records by default; set `limit` from 1 through 100 when more or fewer are useful. The matching command-line option is `once query evidence --limit <count>`. Evidence is historical provenance, not proof that inputs remain unchanged; run the relevant capability when a current result is required.

**Input schema**

```json
{
  "properties": {
    "limit": {
      "default": 5,
      "description": "Maximum number of newest matching records to return.",
      "maximum": 100,
      "minimum": 1,
      "type": "integer"
    },
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

## `once_validate_script`

Parse and validate an annotated script's cache contract.

Reads a workspace-relative script, validates its shebang and `once` directives, and returns the parsed runtime, inputs, outputs, dependency scripts, fingerprints, environment names, working directory, remote policy, and output symlink policy. Put singular directives with quoted values directly after the shebang, for example `# once input "input.txt"` and `# once output "output.txt"`. Plural names, colon syntax, and unquoted paths are invalid. Invalid contracts return `{ valid: false, diagnostics: [...] }`, so callers can repair directive typos before execution. The matching command-line operation is `once query script <path>`.

**Input schema**

```json
{
  "properties": {
    "path": {
      "description": "Workspace-relative annotated script path.",
      "type": "string"
    }
  },
  "required": [
    "path"
  ],
  "type": "object"
}
```

**Example return**

```json
{
  "valid": true,
  "path": "scripts/build.sh",
  "contract": {
    "path": "scripts/build.sh",
    "runtime": "sh",
    "runtime_args": [],
    "inputs": ["src/**"],
    "outputs": ["dist/**"],
    "needs": [],
    "fingerprints": [],
    "env_vars": []
  }
}
```

## `once_exec_script`

Execute a validated annotated script through Once's action cache.

Opt-in tool exposed only when the Model Context Protocol server starts with `once mcp --allow-run`. Validates the script contract before running the same path as `once exec --script`, materializes declared outputs, and returns captured streams, exit status, action digest, cache hit or miss state, and matching evidence. Invoke it twice with unchanged declared inputs to verify cache reuse.

**Input schema**

```json
{
  "properties": {
    "args": {
      "description": "Arguments passed to the script after its path.",
      "items": {
        "type": "string"
      },
      "type": "array"
    },
    "path": {
      "description": "Workspace-relative annotated script path.",
      "type": "string"
    }
  },
  "required": [
    "path"
  ],
  "type": "object"
}
```

**Example return**

```json
{
  "path": "scripts/build.sh",
  "success": true,
  "exit_code": 0,
  "stdout": "built\n",
  "stderr": "",
  "record": {
    "action_digest": "0476bde2e7d8d1a64d9bd6f589ef5b443d0f60b71e2ad6f1c5bd7a2c4c41223f",
    "cache": "hit",
    "exit_code": 0
  },
  "evidence_subject": "0476bde2e7d8d1a64d9bd6f589ef5b443d0f60b71e2ad6f1c5bd7a2c4c41223f",
  "evidence": []
}
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

## `once_validate_workspace`

Validate the complete workspace graph before execution.

Loads every manifest and target kind schema, then checks target attributes, duplicate target ids, missing dependencies, dependency provider compatibility, source patterns, and dependency cycles. Returns stable diagnostics with target and attribute scope plus suggested repairs. Call this after materializing a starter or applying edits and before build, run, or test. The matching command-line operation is `once query validate-workspace`.

**Input schema**

```json
{
  "properties": {},
  "type": "object"
}
```

**Example return**

```json
{
  "valid": false,
  "target_count": 1,
  "diagnostics": [
    {
      "code": "missing_dependency",
      "message": "target `apps/service/Service` depends on missing target `packages/core/Core`",
      "target": "apps/service/Service",
      "attribute": "deps",
      "repairs": ["Declare target `packages/core/Core` or remove it from `deps`"]
    }
  ]
}
```

## `once_validate_target`

Validate a proposed `[[target]]` table against its target kind schema. Returns structured diagnostics instead of prose.

Schema-only validation: checks that the target declares a known target kind, every named dependency role is declared by that kind, every required attribute is present, every declared attribute is known and has the declared type, and the target name is well-formed. The check is local; it does not resolve dependency references or read other manifests. Returns `{ valid: true }` on success or `{ valid: false, diagnostics: [...] }` where each diagnostic carries a stable `code`, the offending `target` id, the offending `attribute` when applicable, and `repairs` an agent can apply.

**Input schema**

```json
{
  "properties": {
    "target": {
      "description": "Raw `[[target]]` table shape with `name`, `kind`, optional `deps`, `dependencies`, `srcs`, and `attrs`.",
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
