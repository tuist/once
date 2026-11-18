# Model Context Protocol

[`struo mcp`](/reference/cli/mcp) runs a
[Model Context Protocol](https://modelcontextprotocol.io/) server over standard
input and output. A coding agent can inspect the typed build graph, query
project memory, and optionally run graph capabilities through the same
[JavaScript Object Notation (JSON)](https://www.json.org/json-en.html) shapes
emitted by [`struo query`](/reference/cli/query) and the command-line execution
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
struo -C ~/code/MyApp mcp
struo mcp --workspace ~/code/MyApp
```

## Protocol Handshake

Struo prefers protocol version `2025-11-25` and negotiates supported client
versions back through `2024-11-05`. The expected sequence:

1. Client → `initialize` request with its protocol version,
   capabilities, and client info.
2. Server → `initialize` result with the negotiated protocol version,
   the `tools` capability, server info, the configured workspace root, and
   cross-tool workflow instructions.
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
              "serverInfo": { "name": "struo-mcp", "version": "0.0.0" },
              "instructions": "Struo is a self-describing build graph…" } }
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

## Verification Reports

With execution enabled, `struo_build_target` and `struo_exec_script` accept the
optional boolean `verify_reproducible`, defaulting to `false`. When `true`, they
return a `struo.verification.v1` report instead of their normal execution record.
The corresponding commands are `struo build --verify-reproducible <target>`
and `struo exec --verify-reproducible --script -- <runtime> <script>`.

`struo_query_verification` accepts an `id` containing the verification UUID and
returns the retained report without running commands. It is available without
`--allow-run`, matching `struo query verification <ID> --format json`. A failed
check is inspectable as a report with `success: false`; failure to find or read
the report is a tool-level error. Successfully reading a report does not mean
the check passed.

| Field | Meaning |
| --- | --- |
| `schema` | `struo.verification.v1`. |
| `id`, `subject` | Verification UUID and the command or target checked. |
| `success` | Both trials succeeded, and either declared artifacts match or, when no artifacts are declared, captured logs match. This is the check's pass/fail decision. |
| `execution_succeeded` | Both trials returned successful results without execution or artifact-retention errors. Identical failures cannot pass. |
| `artifacts_match` | Artifact equality, or `null` when execution failed, no artifacts were declared, or the detail limit prevents distinguishing artifact drift from log drift. |
| `logs_match` | Equality of captured standard output, standard error, and retained declared action logs, or `null` when complete comparison is unavailable. Log drift is advisory when declared artifacts match. |
| `reproducible` | Stricter comparison: successful execution with no artifact, exit-code, or captured-log differences. It can be `false` while `success` is `true` for log-only drift. It is not a proof claim. |
| `trials`, `results` | Trial count and each trial's action digest, result, and error. |
| `differences` | Result-level differences tagged by `kind`: `exit_code`, `stdout`, `stderr`, or `output`. Output records identify the declared path and both digests, with `null` for an absent value. |
| `artifact_differences` | Bounded file-level details with a path, difference kind, a `log` flag for declared action logs, and, when available, the first differing byte offset and text excerpts from both trials. |
| `differences_truncated` | Whether the file-level inventory or difference listing reached its limit. Full retained artifacts remain available for inspection. |
| `trial_directories` | Workspace-relative paths to retained trial results, decoded declared artifacts, captured logs, and trial-local Struo caches. |
| `scope`, `limitations` | What was repeated and which sources of variation or external state were not isolated. |
| `inspect_command` | Command for reading the same retained report again. |

Both trials start from the same whole-workspace snapshot at the same private
path, using separate, initially empty local Struo caches. Script dependencies
and fingerprints run in each trial. Declared trial outputs do not replace
outputs in the original workspace. Reports and artifacts remain local under
`.struo/verifications/<ID>/`; they are not automatically published as project
evidence. Retained files and excerpts may contain sensitive data.

The comparison covers the selected command or target's declared result, not
artifacts and logs produced only by prerequisites.

This is a repeated-trial observation, not proof of reproducibility. Host
toolchains, host tool caches, absolute paths, network services, and external
side effects are not isolated. Clocks, randomness, and scheduling are not
perturbed or controlled. Authorize repeated execution before starting a check.
See [script verification](/guide/scripted/caching#check-repeated-execution) and
[build verification](/guide/graph/#compare-two-fresh-builds) for the workflow
and snapshot limitations. The [tool catalog](/reference/mcp/tools) provides the
exact tool schemas.

## Example session

A complete client/server exchange:

```text
→ { "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { … } }
← { "jsonrpc": "2.0", "id": 0, "result": { … "capabilities": { "tools": {} } … } }
→ { "jsonrpc": "2.0", "method": "notifications/initialized" }
→ { "jsonrpc": "2.0", "id": 1, "method": "tools/list" }
← { "jsonrpc": "2.0", "id": 1, "result": { "tools": [ … ] } }
→ { "jsonrpc": "2.0", "id": 2, "method": "tools/call",
    "params": { "name": "struo_query_workspace", "arguments": {} } }
← { "jsonrpc": "2.0", "id": 2,
    "result": { "content": [ { "type": "text",
                               "text": "{ … workspace state … }" } ],
                "structuredContent": { "result": { … workspace state … } } } }
```

Every advertised tool includes behavioral annotations, a strict root input
schema, and an output schema. Text content remains available for older clients;
newer clients can consume `structuredContent` directly.

When editing and execution are enabled, `struo_materialize_example` can create a
complete starter without returning every file through the model context. It is
idempotent for identical files and rejects the complete operation before
writing when any destination conflicts.

Some hosts probe optional resource, resource-template, or prompt catalogs even
when a server does not advertise them. Struo returns valid empty catalogs for
those probes and responds to the standard liveness request. Workflow guidance
comes from the initialization instructions and live tool catalog.

## Wiring into Claude Desktop

Add an entry to your `claude_desktop_config.json` under `mcpServers`:

```json
{
  "mcpServers": {
    "struo": {
      "command": "struo",
      "args": ["--workspace", "/absolute/path/to/your/project", "mcp"]
    }
  }
}
```

Restart Claude Desktop. The read-only tools appear under the `struo`
server in the connectors list and Claude can call them directly when
planning against the graph and project memory.
