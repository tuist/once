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

- Store and retrieve content-addressed byte payloads.
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

Choose the binding used by the process that owns the integration. All four
bindings use compatible digests and action-result concepts, so separate tools
can share a configured cache without translating records.

## Before You Embed Once

An application using the cache is responsible for defining stable action
payloads and deciding which inputs affect them. The library stores completed
results, but it cannot determine whether an application omitted an input or
whether replaying a result is safe.

Start with the smallest useful operation in the language guide, then add
action-result storage only after the application has a deterministic action
identity.
