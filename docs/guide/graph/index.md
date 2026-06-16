# Graph

The Once graph is the product model for repository automation. Targets
declare what exists in the workspace, capabilities describe what can be
done with those targets, and rules lower each capability into
content-addressed actions that can run locally, replay from cache, or move
to a compute provider.

## Where the Graph Fits

Once has a small set of durable concepts:

1. **Targets**: named units in the workspace.
2. **Capabilities**: operations a target exposes, such as `build`, `run`,
   and `test`.
3. **Actions**: concrete executable work with inputs, outputs,
   environment, platform requirements, and cache identity.
4. **Rules**: typed logic that validates targets and lowers capabilities
   into actions.
5. **Scripts**: the least typed rule-backed adapter for existing
   executable files.

Scripts are not outside the graph. They are the easiest way to enter it.
Teams move from script targets into richer typed rules when they need
stronger relationships, multiple capabilities, or structured diagnostics.

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

Projects can add checked-in Starlark rule files from the root
`once.toml`:

```toml
[rules]
paths = ["rules/*.star"]
```

Each rule file assigns `RULES = [...]` using the same `rule`, `attr`,
`dep`, and `capability` helpers as the built-in prelude. Rule paths are
resolved relative to the project root, loaded in sorted order, and
included in `once query rules`, `once query schema`, validation, MCP
schema tools, and graph analysis.

The `[rules]` table is only loaded from the root manifest. Package
manifests that declare `[rules]` are rejected.

## Capabilities

Each rule declares which capabilities its targets expose:
[`build`](/reference/cli/build), [`run`](/reference/cli/run), and
[`test`](/reference/cli/test). A library might expose `build`; an
application artifact might expose `build`; a runner rule might consume
that artifact and expose `run`; and a test runner rule might expose
`test`.

The CLI dispatches on capability, and every capability runs through the
same action substrate. Build actions can replay from cache when their
inputs match. Run and test actions may still produce cached outputs, but
rules can declare side-effectful work that must happen for the requested
invocation.

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
software, or launch a process, so Once only advertises execution
tools when the server is started with an explicit opt-in:

```sh
once mcp --allow-run
```

With that opt-in, agents can call `once_build_target`,
`once_run_target`, or `once_start_target` with the same target id the
CLI accepts. The start call returns a runtime session id immediately,
then the agent can use `once_runtime_status`, `once_runtime_logs`, and
`once_stop_runtime` to follow or stop the run. Without it, the MCP
surface remains read-oriented and the execution tools are not listed.
