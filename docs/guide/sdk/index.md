---
prev: false
next: false
---

# Software Development Kit

Use a Once language library when an application needs to store and retrieve
cache data directly. Use the [command line](/reference/cli/) instead when the
goal is to execute scripts, build targets, run tests, or manage runtime
sessions.

The language libraries intentionally expose the same small cache surface:

- Open the default local cache, an isolated local root, or the effective
  provider configured for a workspace.
- Store and retrieve content-addressed byte payloads.
- Store and materialize files without loading their complete contents into the
  host language.
- Build versioned action keys from labeled values and content digests.
- Associate an action digest with a completed action result.
- Remove an action result without deleting its referenced payloads.
- Inspect local cache statistics.

They do not execute commands or load the repository graph.

## Choose A Language

| Language | Package | Call style | Guide |
| --- | --- | --- | --- |
| Rust | `once` crate | Asynchronous | [Rust](/guide/sdk/rust) |
| Swift | `Once.xcframework` and the matching Swift wrapper | Asynchronous | [Swift](/guide/sdk/swift) |
| Ruby | `buildonce` gem | Synchronous | [Ruby](/guide/sdk/ruby) |
| JavaScript | `buildonce` package for Node.js | Promise-based | [JavaScript](/guide/sdk/javascript) |
| C | `libonce` and `once.h` | Synchronous | [C](/guide/sdk/c) |

Choose the binding used by the process that owns the integration. All
bindings use compatible digests and action-result concepts, so separate tools
can share a configured cache without translating records.

## Choose A Cache

The default constructor preserves the lightweight local behavior of earlier
releases. Use an explicit local root for test isolation or caller-owned cache
lifetimes. Use a workspace constructor when the integration must share the
same effective provider as the command line. Workspace selection considers
the process override, workspace infrastructure, compatible Tuist project
configuration, and the user default in that order.

Statistics describe the local tier. Remote uploads are best effort: a
configured remote provider writes through the local tier and keeps the local
write usable when the remote service is unavailable. Streamed inputs larger
than eight mebibytes remain in the local tier to preserve bounded memory use.

## Before You Embed Once

Use the action-key builder to partition each integration by namespace and add
every input that can affect the result. Input order is significant and must be
deterministic. The library cannot determine whether an application omitted an
input or whether replaying a result is safe.

Start with the smallest useful operation in the language guide, then add
action-result storage only after the application has a deterministic action
identity.
