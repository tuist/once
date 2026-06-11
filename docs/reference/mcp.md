# MCP

[`once mcp`](/reference/cli/mcp) runs a Model Context Protocol server
over stdio so a coding agent (Claude Desktop, an IDE plug-in, an
Anthropic SDK script) can inspect the typed build graph through the
same JSON shapes [`once query`](/reference/cli/query) emits.

This page is the protocol reference. Transport, handshake, tool
catalog, and an example session.

## Transport

Newline-delimited JSON over stdio. Each request is one line of
JSON-RPC 2.0 in, each response is one line out. Notifications
(messages with no `id`) get no reply.

Spawn the server with the workspace as its working directory, or
pass an explicit `--workspace <DIR>`:

```sh
once -C ~/code/MyApp mcp
once mcp --workspace ~/code/MyApp
```

## Handshake

MCP 2024-11-05. The expected sequence:

1. Client → `initialize` request with its protocol version,
   capabilities, and client info.
2. Server → `initialize` result with protocol version `2024-11-05`,
   the `tools` capability, and server info.
3. Client → `notifications/initialized` (no reply).
4. Client → `tools/list` to discover tools, then `tools/call` for
   each invocation.

```json
{ "jsonrpc": "2.0", "id": 0, "method": "initialize",
  "params": { "protocolVersion": "2024-11-05",
              "capabilities": {},
              "clientInfo": { "name": "claude-desktop", "version": "1.0" } } }
```

Server reply:

```json
{ "jsonrpc": "2.0", "id": 0,
  "result": { "protocolVersion": "2024-11-05",
              "capabilities": { "tools": {} },
              "serverInfo": { "name": "once-mcp", "version": "0.0.0" } } }
```

## Tools

Three read-only tools, each wrapping the matching `once query` verb.
Every successful call returns one `content` block of `type: "text"`
whose `text` is the pretty-printed JSON payload. Failed calls return
`isError: true` with the message in the same `content` shape.

### `once_query_targets`

List every declared target, optionally filtered by rule kind.

**Input schema**

```json
{
  "type": "object",
  "properties": {
    "kind": {
      "type": "string",
      "description": "Restrict results to targets of this rule kind."
    }
  }
}
```

**Returns** an array of target records:

```json
[
  { "id": "apps/ios/AppCore", "package": "apps/ios", "name": "AppCore",
    "kind": "apple_library", "deps": [], "capabilities": ["build"] },
  { "id": "apps/ios/Greeter", "package": "apps/ios", "name": "Greeter",
    "kind": "apple_library", "deps": ["apps/ios/AppCore"],
    "capabilities": ["build"] }
]
```

### `once_query_capabilities`

Return the capabilities a single target exposes, with the output
groups each capability emits and the inputs it requires.

**Input schema**

```json
{
  "type": "object",
  "properties": {
    "target": {
      "type": "string",
      "description": "Canonical target id, e.g. `apps/ios/App`."
    }
  },
  "required": ["target"]
}
```

**Returns** a capability record:

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

### `once_query_schema`

Return the typed contract for a rule kind: attributes (with types
and whether they are required), expected dep providers, emitted
providers, and capabilities.

**Input schema**

```json
{
  "type": "object",
  "properties": {
    "kind": {
      "type": "string",
      "description": "Rule kind to introspect, e.g. `apple_library`."
    }
  },
  "required": ["kind"]
}
```

**Returns** a schema record with `kind`, `docs`, `attrs`,
`capabilities`, and `providers`. Field shapes mirror
`once query schema <kind> --format json`.

## Error model

| Surface              | Where it lands                                |
| -------------------- | --------------------------------------------- |
| Parse error          | JSON-RPC error, code `-32700`, id `null`      |
| Unknown method       | JSON-RPC error, code `-32601`                 |
| Malformed `tools/call` params | JSON-RPC error, code `-32602`         |
| Unknown tool         | Tool result with `isError: true`              |
| Missing argument     | Tool result with `isError: true`              |
| No matching target / schema | Tool result with `isError: true`       |

Tool-level failures stay inside the JSON-RPC `result` envelope so
the agent surfaces the message instead of treating the channel as
broken.

## Example session

A complete client/server exchange:

```text
→ { "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { … } }
← { "jsonrpc": "2.0", "id": 0, "result": { … "capabilities": { "tools": {} } … } }
→ { "jsonrpc": "2.0", "method": "notifications/initialized" }
→ { "jsonrpc": "2.0", "id": 1, "method": "tools/list" }
← { "jsonrpc": "2.0", "id": 1, "result": { "tools": [ … ] } }
→ { "jsonrpc": "2.0", "id": 2, "method": "tools/call",
    "params": { "name": "once_query_targets", "arguments": {} } }
← { "jsonrpc": "2.0", "id": 2,
    "result": { "content": [ { "type": "text",
                               "text": "[ … target list … ]" } ] } }
```

## Wiring into Claude Desktop

Add an entry to your `claude_desktop_config.json` under
`mcpServers`:

```json
{
  "mcpServers": {
    "once": {
      "command": "once",
      "args": ["--workspace", "/absolute/path/to/your/project", "mcp"]
    }
  }
}
```

Restart Claude Desktop. The three tools appear under the `once`
server in the connectors list and Claude can call them directly
when planning against the graph.

## Not yet on the wire

Running a graph action through MCP, returning the action digest,
and exposing the cached outputs / logs / provider record by that
digest as MCP resources. The action cache and content-addressed
storage already key everything by stable digest, so the surface is
ready for this capability when it lands.
