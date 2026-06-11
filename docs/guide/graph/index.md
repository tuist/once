# Graph

The Once graph is a typed build model that sits above the cacheable
script ramp. Once a team has shipped enough scripts to know what their
real cacheable units of work look like, the same work moves into typed
graph targets that carry richer relationships, schemas, capabilities,
diagnostics, and agent-friendly edits.

This page is the conceptual orientation. The platform pages under it go
into rule schemas, attributes, and end-to-end command behavior.

## Where the Graph Fits

Once has three layers:

1. **Script actions** — annotated shell or interpreter scripts that
   `once exec` runs through the action cache. Documented under
   [Scripts](/guide/scripts/).
2. **Script targets** — declared graph targets that wrap a script
   action so it participates in the graph alongside typed rules.
3. **Build graph targets** — typed targets with rule schemas, typed
   attributes, dep edges, capabilities, providers, declared outputs,
   structured diagnostics, and queryable metadata.

Scripts are deliberately the migration ramp. Teams keep their existing
scripts cacheable while moving the same work into graph targets when
they need stronger relationships, multiple capabilities (build / run /
test), or richer diagnostics.

## What a Target Looks Like

Targets live in `once.toml` files at the package level. Every target
carries a `kind`, a `name`, and a typed attribute bag:

```toml
[[target]]
name = "AppCore"
kind = "apple_library"
srcs = ["Sources/**/*.swift"]
deps = ["./Logging"]

[target.attrs]
platform = "ios"
minimum_os = "17.0"
sdk_frameworks = ["Foundation"]
```

Dependency references are root-relative by default; `./` and `../` are
resolved against the package that owns the manifest.

Internally every target has a structured label:

```
package = "apps/ios"
name    = "AppCore"
id      = "apps/ios/AppCore"
```

The id is what providers, queries, and the action cache key against.

## Built-In Rules

Rule schemas and impl callables live in the Starlark prelude. The Rust
side exposes only generic primitives (`host_arch`, `host_which`,
`host_command`, `glob`, `declare_output`, `run_action`, `write_file`);
every rule's domain logic — SDK names, triple format, file-extension
routing, toolchain discovery — is written in Starlark on top of those.

Today the prelude ships Apple rules:

- [`apple_library`](/guide/graph/apple) — mixed Swift + Objective-C /
  C / C++ static library with module emission, bridging headers,
  modulemap generation, `exported_deps`, transitive provider
  propagation, and configurable Xcode / SDK selection.
- `apple_framework` — framework bundle metadata (placeholder action
  pending its toolchain plumbing).
- `apple_application` — application bundle with resources, Info.plist
  metadata, entitlements, and signing inputs (placeholder).
- `apple_test_bundle` — XCTest bundle with optional host application
  (placeholder).

Per-platform deep dives live on the pages nested under this one.

## Capabilities

Each rule declares which capabilities its targets expose: `build`,
`run`, `test`. The CLI dispatches on capability, and every capability
runs as a cacheable action through the same `run_with_cache` substrate
script targets use.

```sh
once query targets
once query capabilities apps/ios/App
once query schema apple_library
once build apps/ios/AppCore
once run  apps/ios/App
once test apps/ios/AppTests
```

`query` commands are deliberately broad — agents and humans can both
ask what a target can do before any execution happens.

## Rule Implementations and Providers

Rule impls receive a `ctx` dict carrying the label, typed attribute
map, raw glob source list, dep provider records, and the workspace-
relative build directory. The impl composes its toolchain command
lines through the generic primitives and emits actions via
`run_action(...)`.

Each impl returns a provider record describing its outputs. Consumer
rules read those records via `ctx["deps"]` to compose their own
commands — `SwiftInfo`-style transitive views of archives,
swiftmodules, linkopts, defines, frameworks, modulemaps, etc.

## Action Cache and Execution

Every action — script or graph — goes through `once_core::run_with_cache`,
which keys on a deterministic action digest composed from:

- The action's argv, env, cwd, and declared outputs.
- An optional input digest that hashes the action's source inputs.
- An optional toolchain identity (so a different Xcode partitions the
  cache cleanly).

A graph command opens a single `BuildSession` per invocation; that
session walks the impl-reachable dependency closure, schedules sibling
targets through a `tokio` `JoinSet` so they compile in parallel, and
moves the last reader's provider record straight into the consumer's
dep list (instead of cloning).

## Configurability

Many of the rule attrs are marked `configurable = true` in the
prelude, anticipating a `select()` / platform-transitions story.
That epic is not in the codebase yet, so the flag is currently
aspirational; single-platform, single-arch builds work today, and
multi-arch fan-out plus configurable attribute resolution land
together once the platform model arrives.

## Authoring and Agent Workflows

The graph is intentionally inspectable first: `query` commands return
structured JSON, rule schemas document every attribute, and providers
expose their shape. Agents can ask what a target can do — and what
exists at the rule level — without running anything.

The action cache plus content-addressed storage gives every action a
stable digest and addressable outputs. A planned MCP interface will
expose this surface so coding agents can call `run_action` with a
returned id and later query the cached outputs, logs, and provider
records by that id without re-running anything.
