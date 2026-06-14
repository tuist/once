# Graph

The Once graph is a typed build model that sits above the cacheable
script ramp. When teams need richer relationships than scripts can
express, the same work moves into typed graph targets that carry
schemas, dep edges, capabilities, and structured diagnostics.

## Where the Graph Fits

Once has three layers:

1. **Script actions**: annotated scripts that `once exec` runs through
   the action cache. See [Scripts](/guide/scripts/).
2. **Script targets**: declared graph targets that wrap a script
   action so it participates alongside typed rules.
3. **Build graph targets**: typed targets validated against a *rule
   schema* (the contract for a kind: which attributes it accepts,
   which providers each dep edge expects, which providers it emits,
   and which capabilities it exposes), carrying typed attributes,
   declared outputs, and structured diagnostics.

Scripts are the migration ramp. Teams move into graph targets when
they need stronger relationships, multiple capabilities, or richer
diagnostics.

## Targets

Targets are the named units in the graph. They live in `once.toml`
files at the package level and declare what they are, what source files
belong to them, and which other targets they depend on:

```toml
[[target]]
name = "AppCore"
kind = "apple_library"
srcs = ["Sources/**/*.swift"]
deps = ["./Logging"]

[target.attrs]
platform = "ios"
minimum_os = "17.0"
```

Dep references are root-relative by default; `./` and `../` resolve
against the package that owns the manifest.

## Rules and Preludes

A target's `kind` points at a rule. The rule defines the target's schema:
which attributes it accepts, which providers it expects from deps, which
providers it emits, and which capabilities it exposes.

Rules are grouped into preludes. A prelude is a domain-specific rule set
that teaches Once how to describe one ecosystem without baking that
ecosystem into the core graph model.

Today the built-in prelude covers one platform family:

- [Apple Prelude](/guide/graph/apple): Swift, Objective-C, C, and C++
  libraries, frameworks, applications, and test bundles for Apple
  platforms.

## Capabilities

Each rule declares which capabilities its targets expose:
[`build`](/reference/cli/build), [`run`](/reference/cli/run), and
[`test`](/reference/cli/test). A library might expose `build`; an
application artifact might expose `build`; a runner rule might consume
that artifact and expose `run`; and a test runner rule might expose
`test`.

The CLI dispatches on capability, and every capability runs through the
same action substrate scripts use. Build actions can replay from cache
when their inputs match. Run and test actions may still produce cached
outputs, but rules can declare side-effectful work that must happen for
the requested invocation.

```sh
once query targets
once query schema apple_library
once build apps/ios/AppCore
once run  tools/demo/LaunchApp
once test tools/tests/RunAppTests
```

[`query`](/reference/cli/query) commands return structured JSON so
agents and humans can ask what a target can do before any execution
happens.

## Runner Targets

Some ecosystems need an external runtime: a simulator, a device, a local
service, or a remote environment. Once should not bake those runtime
types into the core CLI. They belong in rules, where the ecosystem
knowledge already lives.

Model that as a runner target. A runner target depends on the artifact it
knows how to run, carries the runtime-specific attributes its prelude
understands, and exposes the generic `run` capability:

```toml
[[target]]
name = "LaunchApp"
kind = "some_runtime_runner"
deps = ["../apps/App"]

[target.attrs]
runtime = "local"
```

The CLI remains generic:

```sh
once run tools/demo/LaunchApp
```

That keeps the bridge explicit: the producer target emits providers and
output groups; the runner rule declares which providers it accepts and
what command it runs against them. The prelude owns any runtime-specific
probing, validation, installation, launch, and diagnostics.

## Agent Access

The MCP server starts as an inspection surface. An agent can discover
targets, query schemas, and inspect rule contracts without being allowed
to execute anything:

```sh
once mcp
```

Running is side-effectful. It can build code, write outputs, install
software, or launch a process, so Once only advertises runtime session
tools when the server is started with an explicit opt-in:

```sh
once mcp --allow-run
```

With that opt-in, agents can call `once_start_target` with the same
target id the CLI accepts. The call returns a runtime session id
immediately, then the agent can use `once_runtime_status`,
`once_runtime_logs`, and `once_stop_runtime` to follow or stop the run.
Without it, the MCP surface remains read-oriented and the runtime tools
are not listed.
