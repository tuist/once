# Model Context Protocol

[`once mcp`](/reference/cli/mcp) runs a
[Model Context Protocol](https://modelcontextprotocol.io/) server over standard
input and output. A coding agent can inspect the typed build graph, query
project memory, and optionally run graph capabilities through the same
[JavaScript Object Notation (JSON)](https://www.json.org/json-en.html) shapes
emitted by [`once query`](/reference/cli/query) and the command-line execution
commands.

This page covers transport, handshake, error model, an example
session, and Claude Desktop configuration. The
[Tools](/reference/mcp/tools) page documents each tool's input schema and
return shape.

## Transport

The server exchanges newline-delimited JSON over standard input and output.
Each request is one line in and each response is one line out, following
[JSON Remote Procedure Call (JSON-RPC) 2.0](https://www.jsonrpc.org/specification).
Notifications, which have no `id`, get no reply.

Spawn the server with the workspace as its working directory, or
pass an explicit `--workspace <DIR>`:

```sh
once -C ~/code/MyApp mcp
once mcp --workspace ~/code/MyApp
```

## Protocol Handshake

Once prefers protocol version `2025-11-25` and negotiates supported client
versions back through `2024-11-05`. The expected sequence:

1. Client → `initialize` request with its protocol version,
   capabilities, and client info.
2. Server → `initialize` result with the negotiated protocol version,
   the `tools` capability, server info, and cross-tool workflow instructions.
3. Client → `notifications/initialized` (no reply).
4. Client → `tools/list` to discover tools, then `tools/call` for
   each invocation.

```json
{ "jsonrpc": "2.0", "id": 0, "method": "initialize",
  "params": { "protocolVersion": "2025-11-25",
              "capabilities": {},
              "clientInfo": { "name": "claude-desktop", "version": "1.0" } } }
```

Server reply:

```json
{ "jsonrpc": "2.0", "id": 0,
  "result": { "protocolVersion": "2025-11-25",
              "capabilities": { "tools": {} },
              "serverInfo": { "name": "once-mcp", "version": "0.0.0" },
              "instructions": "Once is a self-describing build graph…" } }
```

## Error model

| Surface                         | Where it lands                              |
| ------------------------------- | ------------------------------------------- |
| Parse error                     | JSON-RPC error, code `-32700`, id `null`    |
| Unknown method                  | JSON-RPC error, code `-32601`               |
| Malformed `tools/call` params   | JSON-RPC error, code `-32602`               |
| Unknown tool                    | Tool result with `isError: true`            |
| Missing argument                | Tool result with `isError: true`            |
| No matching target or schema    | Tool result with `isError: true`            |

Tool-level failures stay inside the JSON-RPC `result` envelope so the
agent surfaces the message instead of treating the channel as broken.

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
                               "text": "[ … target list … ]" } ],
                "structuredContent": { "result": [ … target list … ] } } }
```

Every advertised tool includes behavioral annotations, a strict root input
schema, and an output schema. Text content remains available for older clients;
newer clients can consume `structuredContent` directly.

Some hosts probe optional resource, resource-template, or prompt catalogs even
when a server does not advertise them. Once returns valid empty catalogs for
those probes and responds to the standard liveness request. Workflow guidance
comes from the initialization instructions and live tool catalog.

## Wiring into Claude Desktop

Add an entry to your `claude_desktop_config.json` under `mcpServers`:

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

Restart Claude Desktop. The read-only tools appear under the `once`
server in the connectors list and Claude can call them directly when
planning against the graph and project memory.
