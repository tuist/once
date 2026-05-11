# Runtime Targets and Headless Sessions

Runtime targets are for side-effecting software that an agent may need to
observe or control after launch: simulator apps, dev servers, desktop
apps, emulators, and test harnesses with live state.

## TOML Shape

Fabrik attaches runtime metadata to targets:

- `[[target]]`: target declaration.
- `rule = "command"`: generic command rule, cacheable by default.
- `[target.runtime]`: optional runtime inspection metadata for the
  enclosing target. A command target with runtime metadata is never
  cacheable.
- `rule = "apple.simulator_app"`: Apple simulator app bundle. It uses
  `platform = "ios"` today, with room for `watchos` and `tvos` later.

```toml
[[target]]
name = "dev"
rule = "command"

[target.attrs]
argv = ["npm", "run", "dev"]

[target.runtime]
kind = "web_server"
capabilities = ["logs", "http", "screenshot"]

[[target.runtime.interface]]
name = "logs"
kind = "stream"
argv = ["tail", "-f", ".fabrik/runtime/dev/stdout.log"]
```

Structured `fabrik run` output includes a `runtime` descriptor:

```json
{
  "target": "app/dev",
  "kind": "runtime_task",
  "runtime": {
    "kind": "web_server",
    "subject": "app/dev",
    "capabilities": ["logs", "http"],
    "interfaces": []
  }
}
```

## Headless Sessions

Headless mode should make a running target inspectable without a GUI.
The session should be local-first and file-backed so agents can recover
after interruptions.

```text
.fabrik/runtime/<session-id>/
  session.json
  events.ndjson
  stdout.log
  stderr.log
  artifacts/
  control.sock
```

`session.json` records the target, runtime kind, pid or simulator UDID,
start time, capabilities, and interface schemas. `events.ndjson`
contains append-only lifecycle, log, artifact, UI, and diagnostic
events. `control.sock` accepts local commands such as stop, restart,
send input, capture screenshot, snapshot UI, tap, type text, and stream
logs.

## Transport

Use JSON-RPC over a Unix domain socket for the first headless control
plane, plus NDJSON for event replay and streaming. This keeps the
protocol easy for coding agents, shell tools, and editors to consume
without generated clients.

For the normal agent workflow, ask `fabrik run` to create and serve the
session:

```sh
fabrik run --runtime-rpc app/dev
```

Structured output includes the session path and socket:

```json
{
  "runtime": {
    "kind": "web_server",
    "session": ".fabrik/runtime/app-dev-1778520000000",
    "rpc": {
      "transport": "jsonrpc",
      "socket": ".fabrik/runtime/app-dev-1778520000000/control.sock"
    }
  }
}
```

Serve the local RPC endpoint for an existing session directory when
replaying or debugging a saved session:

```sh
fabrik runtime rpc .fabrik/runtime/<session-id>
```

The default socket is `.fabrik/runtime/<session-id>/control.sock`
when the path fits platform limits. On platforms with short Unix
socket path limits, Fabrik uses a temporary socket path and reports it
in the run record.
Requests are newline-delimited JSON-RPC 2.0 messages:

```json
{"jsonrpc":"2.0","id":1,"method":"runtime.describe","params":{}}
```

gRPC can be added later for remote runners or long-lived IDE
integrations, but it should not be the only local protocol. Local
agents need inspectable files, stable JSON, and simple streams more
than they need a binary RPC dependency.

## RPC Contract

The runtime RPC contract should be platform-agnostic. A web server, an
iOS simulator app, a macOS app, and an emulator should all expose the
same core domains where the concepts overlap. Platform-specific
features are extensions advertised through capabilities.

Core domains:

- `session`: identity, status, capabilities, and declared interfaces.
- `events`: ordered lifecycle, log, diagnostic, artifact, and UI
  events.
- `logs`: queryable and streamable text records.
- `artifacts`: screenshots, captures, traces, result bundles, and other
  files.
- `process`: generic lifecycle control.
- `input`: generic text, signal, and stdin-style input where supported.

Extension domains use runtime-specific prefixes:

- `ui.*`: visual and accessibility inspection or interaction.
- `apple.simulator.*`: simulator-only controls such as status bar,
  location, appearance, hardware keyboard, and device buttons.
- `browser.*`: browser-specific controls such as navigation, DOM
  inspection, console logs, and network events.

The extension rule is strict: if a method is useful outside one
runtime, promote it to a core or shared domain. If it depends on a
runtime-specific substrate, keep it behind a namespaced capability.

### Session

```json
{
  "id": "runtime.describe",
  "params": {}
}
```

Returns:

```json
{
  "sessionId": "01HX...",
  "target": "examples/apple/ios/simulator-app/Demo",
  "runtime": { "kind": "ios_simulator", "platform": "ios" },
  "state": "running",
  "capabilities": [
    "logs.query",
    "logs.stream",
    "artifacts.read",
    "ui.snapshot",
    "ui.action",
    "apple.simulator.location"
  ]
}
```

### Events

`events.subscribe` streams new events after an optional cursor.
`events.query` replays events from `events.ndjson`.

Every event has the same envelope:

```json
{
  "cursor": "00000000042",
  "time": "2026-05-11T16:20:00Z",
  "domain": "logs",
  "kind": "record",
  "severity": "info",
  "source": "stdout",
  "payload": {}
}
```

### Logs

Logs are a core domain because every runtime can expose them, even if
the backend source differs.

```json
{
  "id": "logs.query",
  "params": {
    "source": "app",
    "since": "2026-05-11T16:00:00Z",
    "levels": ["error", "warn"],
    "text": "database",
    "limit": 200
  }
}
```

```json
{
  "id": "logs.stream",
  "params": {
    "source": "app",
    "cursor": "00000000100",
    "levels": ["info", "warn", "error"]
  }
}
```

The normalized log record shape:

```json
{
  "time": "2026-05-11T16:20:00Z",
  "source": "app",
  "level": "info",
  "message": "Started",
  "fields": {
    "process": "Demo",
    "pid": 1234,
    "subsystem": "dev.fabrik"
  }
}
```

For Apple simulators, `source = "app"` might come from `simctl spawn
log stream`; for a web server, it might come from stdout. Agents should
not need to care.

### Artifacts

Artifacts are files with metadata and stable paths:

```json
{
  "id": "artifacts.list",
  "params": { "kind": "screenshot" }
}
```

```json
{
  "id": "artifacts.read",
  "params": { "artifactId": "screenshot/latest" }
}
```

The response returns metadata plus a path or bytes depending on the
transport:

```json
{
  "artifactId": "screenshot/latest",
  "kind": "screenshot",
  "mediaType": "image/png",
  "path": ".fabrik/runtime/01HX/artifacts/screenshot.png"
}
```

### UI

UI is shared by many runtimes but optional. A runtime advertises
`ui.snapshot` when it can inspect UI state and `ui.action` when it can
interact with it.

```json
{
  "id": "ui.snapshot",
  "params": { "format": "accessibility-tree" }
}
```

```json
{
  "id": "ui.perform",
  "params": {
    "action": "tap",
    "target": { "label": "Continue" }
  }
}
```

Coordinates are a fallback. Stable element ids or accessibility labels
are preferred because they survive screen size and density differences.

### Platform Extensions

Apple simulator-specific actions stay in an extension domain:

```json
{
  "id": "apple.simulator.setLocation",
  "params": { "latitude": 52.52, "longitude": 13.405 }
}
```

```json
{
  "id": "apple.simulator.setAppearance",
  "params": { "appearance": "dark" }
}
```

An agent discovers these through `runtime.describe`. It should never
guess that a session supports an extension just because the runtime kind
looks familiar.

## Agent Model

A coding harness usually separates:

- process execution, captured as stdout, stderr, exit status, and
  lifecycle events
- artifacts, written to stable paths
- control actions, exposed as small typed commands
- observation streams, replayable from disk and subscribable live

Fabrik runtime sessions should follow that model. A human can still run
`fabrik run`; an agent can run it, read the returned session id, tail
events, and send control messages through the socket.
