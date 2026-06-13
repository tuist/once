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
      "requires_outputs": [] }
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

## `once_run_target`

Run a target through the same action path as `once run`.

Opt-in tool exposed only when the MCP server starts with `once mcp --allow-run`. Executes `once run --format json` for a target and returns the structured run record. The tool has the same side effects as the CLI: it may build dependencies, write `.once/out` outputs, install software, or launch a process.

**Input schema**

```json
{
  "properties": {
    "target": {
      "description": "Canonical target id to run.",
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
  "target": "tools/demo/LaunchApp",
  "kind": "script",
  "capability": "run",
  "status": "completed",
  "cache": "miss",
  "outputs": [".once/out/tools/demo/LaunchApp/run"]
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

