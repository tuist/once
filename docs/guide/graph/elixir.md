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

Third-party packages need to be represented as Elixir library dependencies if
Once should cache and rebuild them at target granularity. Passing a
precompiled build directory through compiler arguments leaves that work
outside the graph.

## Target References and Limitations

- [`elixir_library`](/reference/prelude/elixir_library) documents application
  metadata, configuration, private files, environment, dependencies, and the
  `bytecode` output group.
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
