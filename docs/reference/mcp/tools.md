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
    { "name": "build", "output_groups": ["bundle", "dsyms"],
      "requires_outputs": [] },
    { "name": "run", "output_groups": ["default"],
      "requires_outputs": ["bundle"] }
  ]
}
```

## `once_query_schema`

Return the typed contract for a rule kind: attributes, dep edges, providers, and capabilities.

Returns the rule schema (the typed contract a target of that kind must match) as `once query schema <kind> --format json` would. The record carries the rule's documentation, attribute list (with types, required flag, and whether the attribute is configurable), expected dep providers, emitted providers, and exposed capabilities.

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
    { "name": "platform", "ty": "string", "required": true, "configurable": true },
    { "name": "sdk_frameworks", "ty": "list<string>", "required": false, "configurable": true }
  ],
  "capabilities": [ { "name": "build", "output_groups": ["archive"], "requires_outputs": [] } ],
  "providers": ["SwiftInfo", "CcInfo"]
}
```

