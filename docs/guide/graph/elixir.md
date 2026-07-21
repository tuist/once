---
prev: false
next: false
---

# Elixir

Once can compile Elixir libraries into cacheable bytecode and run ExUnit tests
against that compiled application. This guide declares one library and one test
target for the same code.

## Prerequisites

Install the repository's pinned Erlang and Elixir toolchain through mise:

```sh
mise install
mise exec -- elixir --version
```

Library builds use `elixir`, `elixirc`, and `erl`. The direct ExUnit path in
this guide does not require a Mix project. If a test target sets `mix_config`,
`mix` must also be available.

## Declare the Library and Test

Create `apps/greeting/once.toml`:

```toml
[[target]]
name = "greeting"
kind = "elixir_library"
srcs = ["lib/**/*.ex"]

[target.attrs]
app_name = "greeting"
mix_env = "test"

[[target]]
name = "greeting_test"
kind = "elixir_test"
srcs = ["test/**/*.exs"]
deps = ["./greeting"]

[target.attrs]
labels = ["unit"]
```

Use this source layout:

```text
apps/greeting/
├── once.toml
├── lib/
│   └── greeting.ex
└── test/
    ├── test_helper.exs
    └── greeting_test.exs
```

The library uses `mix_env = "test"` because `elixir_test` requires exactly
one `elixir_library` dependency compiled for the test environment. The test
target consumes the existing bytecode instead of compiling the application a
second time.

## Query Before Building

Inspect the declared targets and their contracts:

```sh
once query targets --kind elixir_library
once query capabilities apps/greeting/greeting
once query capabilities apps/greeting/greeting_test
once query schema elixir_library
once query schema elixir_test
```

The library exposes `build`. The test target exposes `test`. Neither target
exposes `run`.

## Build and Test

Build the application bytecode:

```sh
once build apps/greeting/greeting
```

For this target, the staged bytecode and application metadata appear under
`.once/out/apps/greeting/greeting/ebin`. Private application files appear
under `.once/out/apps/greeting/greeting/priv`. Compile metadata and persisted
warnings live beside them as `compile-metadata.tsv` and
`compile-warnings.log`.

Run the tests against that compiled library:

```sh
once test apps/greeting/greeting_test
```

Test results and logs appear under
`.once/out/apps/greeting/greeting_test/test/`, including
`test_results.json`, `elixir-test.log`, and `native_results.txt`.

Changing only a test file reruns the test without recompiling the library.
Changing a library source invalidates the library build and the dependent
test. Configuration files, private files, data, and supported dynamic compile
inputs can also participate in cache invalidation when declared by the target.

## Add Dependencies Deliberately

Compile-time dependencies such as macros or structs used from another target
should be separate `elixir_library` targets connected through `deps`. This
keeps each compiled application queryable and lets Once rebuild the affected
part of the graph.

The library target does not require `mix.exs`. Set `mix_config` only when the
project file should affect the build or when tests must run through `mix test`.
In that mode, Once still uses bytecode compiled by the library target and runs
Mix with dependency checks and compilation disabled.

For third-party packages managed by Mix and Hex, declare one
`mix_dependencies` target. Mix evaluates the active dependency graph from
`mix.exs`, while the committed `mix.lock` supplies the exact version, source,
and checksums. Once emits a synthetic `mix_package` target for each active
locked package and preserves Mix's dependency edges.

```toml
[[target]]
name = "mix_dependencies"
kind = "mix_dependencies"
srcs = ["mix.exs", "mix.lock", "mix-dependencies.json"]

[target.attrs]
graph_file = "mix-dependencies.json"
resolver_inputs = [
  "mix.exs",
  "mix.lock",
  "mix-dependencies.json",
  "local_helper/mix.exs",
]
vendor_dir = "deps"

[target.attrs.path_dependencies]
local_helper = "./local_helper"

[[target]]
name = "local_helper"
kind = "elixir_library"
srcs = ["local_helper/lib/**/*.ex"]

[target.attrs]
app_name = "local_helper"

[[target]]
name = "greeting"
kind = "elixir_library"
srcs = ["lib/**/*.ex"]
deps = ["./mix_dependencies"]
```

Fetch dependency sources before the Once build and keep them under the
configured vendor directory. Graph loading never runs dependency fetching or
updates the lockfile. With `graph_file`, ordinary graph loading also avoids
executing Mix. Without it, Once evaluates the project with isolated Mix and Hex
homes and Hex offline mode. The live graph path requires Erlang 27 or newer for
its built-in JavaScript Object Notation encoder. The checked-in snapshot binds
the exact `mix.exs` and `mix.lock` text together with the selected `mix_env`, so
a manifest, lockfile, or environment change makes graph loading fail until the
snapshot is regenerated. Its `once_inputs` map also binds every exact resolver
input except the snapshot itself. Include path dependency manifests so their
edges cannot become stale. Package compilation sets Hex offline mode and uses
the registered Mix compiler pipeline, so Elixir and Erlang compilers declared
by a Mix-managed package keep their native behavior.

Map every active Mix path dependency through `path_dependencies`. Each value is
an ordinary first-party Once target reference. Graph loading fails instead of
silently dropping a path dependency when its application name is not mapped.

The importer currently accepts locked Hex and Git sources whose build manager
is Mix. Rebar, Make, and dependency declarations with a custom compile command
fail with an explicit error. They need dedicated target kinds so their tools,
inputs, and outputs remain visible to caching and scheduling.

Inspect the imported packages before building the local application:

```sh
once query targets --kind mix_package
once build greeting
elixir \
  -pa .once/out/mix-locked-greeting/mix/prod/lib/locked_greeting/ebin \
  -pa .once/out/local_helper/ebin \
  -pa .once/out/greeting/ebin \
  -e 'IO.write(Greeting.message("Once"))'
```

The bundled dependency starter compiles a locked `locked_greeting` package and
a mapped local helper, then compiles the first-party `Greeting` module against
both. Evaluating `Greeting.message("Once")` returns `Hello, Once!`, which
verifies both provider edges as well as the imported graph.

## Target References and Limitations

- [`elixir_library`](/reference/prelude/elixir_library) documents application
  metadata, configuration, private files, environment, dependencies, and the
  `bytecode` output group.
- [`mix_dependencies`](/reference/prelude/mix_dependencies) documents locked
  Mix and Hex graph expansion, vendored sources, and root selection.
- [`mix_package`](/reference/prelude/mix_package) documents the generated
  package target and its locked identity provider fields.
- [`elixir_test`](/reference/prelude/elixir_test) documents direct ExUnit and
  Mix test modes, test arguments, labels, timeouts, results, and logs.

An `elixir_test` must depend on exactly one `elixir_library` built with
`mix_env = "test"`. Direct ExUnit options cannot be combined with Mix mode.
Reserved compatibility attributes listed in the references fail validation when
set to non-empty values.

## Next

Add a second `elixir_library` dependency when the application has a real
compile-time boundary, then query and test the resulting graph again. Once the
graph is stable, continue with [Memory](/guide/memory/) to inspect the durable
context recorded for builds and tests.
