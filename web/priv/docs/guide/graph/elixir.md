---
prev: false
next: false
---

# Elixir

Once can read an existing `mix.exs`, derive a typed build graph, and cache the
compiled project and each locked dependency separately. You can build, lint,
test, run Mix tasks, and assemble a release without first translating the
project into `once.toml`.

## Start With an Existing Mix Project

### Check the Toolchain

Once invokes `elixir`, `erl`, and `mix` from the selected toolchain. Confirm
that the versions expected by the project are available:

```sh
elixir --version
mix --version
```

If the repository uses [mise](https://mise.jdx.dev/) to pin them, install and
activate that configuration first:

```sh
mise install
mise exec -- elixir --version
mise exec -- mix --version
```

Fetching packages from Hex requires a local Hex installation. A dependency
whose build manager is Rebar also requires [Rebar 3](https://rebar3.org/):

```sh
mix local.hex --force
mix local.rebar --force
```

### Materialize Locked Dependencies

Once does not download dependencies or change `mix.lock`. If the project has a
current lockfile, materialize its exact sources before asking Once to load the
graph:

```sh
mix deps.get --check-locked
```

Run `mix deps.get` without `--check-locked` once when the lockfile needs to be
created or deliberately updated. Review that change before continuing. A
dependency-free project does not need `mix.lock`.

Once currently requires a concrete lockfile when a project declares external
dependencies. Library maintainers who do not commit `mix.lock` can generate
one locally before using Once, but repeatable builds on another machine require
the same lockfile.

### Preview the Derived Graph

From the directory that contains `mix.exs`, inspect the match and the graph:

```sh
once native list
once native show mix
once query targets
```

No `once.toml` is required, and these commands do not write one. If the
workspace contains several Mix projects, select one explicitly:

```sh
once native show mix --path apps/accounts
```

The identifiers printed by `once query targets` are the source of truth. For a
Mix project at the workspace root, discovery normally derives:

- `mix_application_dev`, `mix_application_test`, and
  `mix_application_prod` compile the first-party project in each environment;
- one dependency target per environment when `mix.lock` is present;
- `mix_lint` checks unused dependencies and formatting;
- `mix_tests` runs the project tests when the project has files under `test/`;
- `mix_release` assembles a production release;
- `mix` builds the default development application.

Nested projects use package-qualified identifiers such as
`apps/accounts/mix_tests`.

### Build, Lint, Test, Run, and Release

Use the derived targets through the same commands as every other Once graph:

```sh
once build mix
once run mix_lint
once test mix_tests
once run mix_application_dev -- phx.server
once build mix_release
```

The argument after `--` is a Mix task selected at invocation time.
`phx.server` is only an example; native discovery does not assume that the
project uses Phoenix or another framework.

`mix_lint` runs `mix deps.unlock --check-unused` and
`mix format --check-formatted`. It does not infer project-specific tools such
as Credo. The run target is intentionally uncached for servers and interactive
tasks. Builds, the derived lint target, tests, dependency compilation, and
release assembly are cacheable.

`mix_release` is useful for applications that support `mix release`. A library
can still build, lint, and test even when producing a release is not meaningful.
Framework-specific preparation such as asset compilation is not inferred. Add
an explicit release target with `pre_tasks` when the release requires it.

### Confirm Caching

Run the same build twice without changing an input:

```sh
once build mix_application_dev
once build mix_application_dev
```

The second invocation should restore unchanged actions from the configured
cache. Outputs are materialized under `.once/out/`. Changing one dependency
invalidates that package and its consumers, changing application source
invalidates the application, and changing only a test file reruns the tests
without recompiling the application.

Dependency fetching is outside the Once graph. Once caches compilation after
`deps/` contains the sources selected by `mix.lock`. Continue with
[Caching](/guide/scripted/caching) to configure a shared remote cache.

### Record the Project Selection

Initialization is optional. Use it when the repository should make its native
project selection explicit:

```sh
once native init mix
```

This writes only the `mix_workspace` seed. The detailed targets remain derived
from `mix.exs` and `mix.lock`, so they do not become duplicated configuration.
Commit the seed if reviewers and continuous integration should see the
selection. Pin the same Once version in continuous integration, materialize
locked dependencies, and run the same target commands used locally.

### Umbrellas, Nested Projects, and Path Dependencies

Discovery preserves workspace-relative source roots for path dependencies and
recognizes nested Mix projects. An umbrella root builds its detected child
workspaces. Use `once query targets` to find each package-qualified target
rather than assuming that every target lives at the root.

External Hex and Git dependencies must be present under `deps/`. Mix and Rebar
3 packages are supported. Packages that require Make or a custom dependency
compile command fail with an explicit diagnostic instead of silently falling
back to an opaque build.

### Troubleshooting

- If graph loading reports a missing lockfile or dependency source, run
  `mix deps.get`, then retry with `mix deps.get --check-locked`.
- If `mix_tests` is absent, check whether the project has files under `test/`.
  The `mix_application_test` target is still available for a test-environment
  compile.
- If release assembly needs `assets.deploy` or another preparation task,
  declare a `mix_release` target with `pre_tasks`. Once does not infer
  framework conventions.
- Native project evaluation requires Erlang 27 or newer.
- Use `once native show mix --path <path>` when more than one
  `mix.exs` matches the workspace.

## Author a Graph When You Need More Control

Direct `mix.exs` discovery is the default for an existing Mix project.
Hand-authored targets are useful for a standalone library without Mix, custom
lint or release tasks, database setup, or a boundary that should be cached
independently.

## Declare a Standalone Library and Test

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

The library uses `mix_env = "test"` because `elixir_test` requires exactly one
dependency that provides a complete `elixir_app` compiled for the test
environment. The test target consumes the existing bytecode instead of
compiling the application a second time.

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

For a first-party project that needs its registered Mix compiler pipeline,
declare `mix_project` instead of `elixir_library`:

```toml
[[target]]
name = "dependencies_test"
kind = "mix_dependencies"
srcs = ["mix.exs", "mix.lock"]

[target.attrs]
resolver_inputs = [
  "mix.exs",
  "mix.lock",
  "deps/*/mix.exs",
  "deps/*/rebar.config",
]
mix_env = "test"
target_prefix = "mix-test"

[[target]]
name = "application_test"
kind = "mix_project"
srcs = ["lib/**/*", "test/support/**/*"]
deps = ["./dependencies_test"]

[target.attrs]
app_name = "greeting"
mix_env = "test"
manifest = "mix.exs"
compile_args = ["--warnings-as-errors"]

[[target]]
name = "tests"
kind = "elixir_test"
srcs = ["test/**/*.exs"]
deps = ["./application_test"]

[target.attrs]
cacheable = false
setup_tasks = [
  ["ecto.create", "--quiet", "--no-compile", "--no-deps-check"],
  ["ecto.migrate", "--quiet", "--no-compile", "--no-deps-check"],
]
data = ["priv/repo/migrations/**/*"]
```

Fetch the exact dependency sources before loading the graph:

```sh
mix deps.get --check-locked
once build application_test
once test tests
```

`mix_project` runs the first-party compiler pipeline with dependency checking
and dependency compilation disabled. Each dependency is already represented by
an `elixir_app` provider. A manifest can set `run_task`, or the caller can
supply a task without coupling the discovered graph to a framework convention:

```sh
once run mix_application_dev -- phx.server
```

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
homes and Hex offline mode. Vendored locked packages do not require a Hex
archive to be installed in that isolated home. The live graph path requires
Erlang 27 or newer for its built-in JavaScript Object Notation encoder. The
checked-in snapshot binds the exact `mix.exs` and `mix.lock` text together with
the selected `mix_env`, so a manifest, lockfile, or environment change makes
graph loading fail until the snapshot is regenerated. Its `once_inputs` map
also binds every exact resolver input except the snapshot itself. Include path
dependency manifests so their edges cannot become stale. Package compilation
sets Hex offline mode. Mix packages use their registered compiler pipeline,
while Rebar packages use Rebar 3 directly. Each dependency uses the compilation
environment reported by Mix, with `dependency_mix_env = "prod"` as the
fallback. The root configuration is read with the dependency set's `mix_env`,
so use separate dependency targets and distinct `target_prefix` values for
development, test, and production graphs.

Projects whose dependencies read compile-time application configuration
should declare the full root configuration set:

```toml
[target.attrs]
config = ["config/**/*.exs"]
config_entry = "config/config.exs"
```

The `config` globs must include every file imported by `config_entry`.

Map every active Mix path dependency through `path_dependencies`. Each value is
an ordinary first-party Once target reference. Graph loading fails instead of
silently dropping a path dependency when its application name is not mapped.
Once preserves each mapped target's workspace-relative source root in the
isolated build workspace, so relative paths in `mix.exs` keep their original
meaning.

The importer accepts locked Hex and Git sources whose build manager is Mix or
Rebar 3. Make-only packages and dependency declarations with a custom compile
command fail with an explicit error.

## Lint and Release

A `mix_project` can expose deterministic checks through `once run`:

```toml
[target.attrs]
run_tasks = [
  ["deps.unlock", "--check-unused"],
  ["format", "--check-formatted"],
  ["credo"],
]
run_cacheable = true
```

Assemble a release from a compiled project with `mix_release`:

```toml
[[target]]
name = "application_prod"
kind = "mix_project"
srcs = ["lib/**/*"]
deps = ["./dependencies_prod"]

[target.attrs]
app_name = "greeting"
mix_env = "prod"

[[target]]
name = "release"
kind = "mix_release"
srcs = ["mix.exs"]
deps = ["./application_prod"]

[target.attrs]
manifest = "mix.exs"
config = ["config/**/*.exs"]
```

Projects that need preparation can add the ordered `pre_tasks` documented by
[`mix_release`](/reference/prelude/mix_release). Declare any files and
executables used by those tasks through `data` and `tools` so they participate
in caching.

Build the release twice to confirm that the second invocation restores the
release action from the configured cache:

```sh
once build release
once build release
```

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
- [`mix_project`](/reference/prelude/mix_project) documents first-party Mix
  compilation and run tasks.
- [`mix_release`](/reference/prelude/mix_release) documents isolated,
  cacheable Mix release assembly.
- [`elixir_test`](/reference/prelude/elixir_test) documents direct ExUnit and
  Mix test modes, test arguments, labels, timeouts, results, and logs.

An `elixir_test` must depend on exactly one complete `elixir_app` provider built
with `mix_env = "test"`. Set `cacheable = false` for tests that must contact an
external service on every run. Direct ExUnit options cannot be combined with
Mix mode.
Reserved compatibility attributes listed in the references fail validation when
set to non-empty values.

## Next

Build the discovered development target twice and confirm the second build
comes from cache. Then run the derived lint and test targets before adding
hand-authored targets. Continue with [Testing and Scheduling](/guide/graph/testing)
for larger test graphs or [Memory](/guide/memory/) to inspect the durable
context recorded for builds and tests.
