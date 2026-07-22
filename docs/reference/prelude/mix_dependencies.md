# `mix_dependencies`

Locked Mix dependency graph consumed by Elixir targets.

## Description

During graph loading, Once reads a checked-in graph snapshot or asks Mix to
evaluate the dependencies active for the configured environment. Once reads package identity from the committed
`mix.lock` and does not run a second version solver. Every active locked Hex or
Git dependency becomes a synthetic `mix_package` target with its own source
inputs, build action, and dependency edges.

The lockfile is authoritative. Graph loading fails when an active Mix
dependency has no lock entry, when the manifest or lockfile is not included in
`resolver_inputs`, or when a selected source is outside the Once package. When
`resolver_inputs` is empty or omitted, `srcs` supplies those files instead.
Resolution does not fetch sources and never writes `mix.lock`.

Dependency sources must already exist under `vendor_dir`, normally `deps`.
This separates network and registry access from build execution. Package build
actions enable Hex offline mode and use the locked source tree as declared
inputs. Live graph loading uses the vendored source tree and committed lockfile
without requiring a separately installed Hex archive.

The aggregate target depends only on the direct roots selected by Mix. The
synthetic targets carry transitive edges, so Once still sees and schedules the
complete graph. An optional `roots` list can expose a narrower set by Mix
application name or Hex package name.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `manifest` | string | no | `mix.exs` | Package-relative Mix project manifest used to select the active graph |
| `lockfile` | string | no | `mix.lock` | Package-relative authoritative lockfile |
| `resolver_inputs` | list&lt;string&gt; | no | `srcs` | Package-relative text globs supplied to the resolver |
| `graph_file` | string | no |  | Checked-in [JavaScript Object Notation](https://www.json.org/json-en.html) snapshot with exact manifest, lockfile, Mix environment, and resolver input bindings |
| `vendor_dir` | string | no | `deps` | Package-relative directory containing fetched dependency source trees |
| `path_dependencies` | map&lt;string, string&gt; | no | `{}` | Active path dependency application names mapped to first-party Once targets |
| `mix_env` | string | no | `prod` | Mix environment used to evaluate dependency declarations |
| `roots` | list&lt;string&gt; | no | `[]` | Optional application or Hex package names exposed as direct roots |
| `_mix_resolved` | bool | resolver-owned | `false` | Marks a graph expanded from the lockfile |
| `_mix_locked_roots` | list&lt;string&gt; | resolver-owned | `[]` | Records generated root target names |

The underscore-prefixed attributes are produced by the resolver and should not
be set in `once.toml`.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `elixir_app` | Direct locked package roots emitted by the resolver |

Transitive package relationships are ordinary `deps` edges between generated
`mix_package` targets.

## Providers

The target emits `mix_dependency_set` and `elixir_app`. The `elixir_app`
provider aggregates every reachable package so first-party Elixir targets can
consume one dependency edge.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | none |

## Example

```toml
[[target]]
name = "mix_dependencies"
kind = "mix_dependencies"
srcs = ["mix.exs", "mix.lock", "mix-dependencies.json"]

[target.attrs]
manifest = "mix.exs"
lockfile = "mix.lock"
graph_file = "mix-dependencies.json"
resolver_inputs = ["mix.exs", "mix.lock", "mix-dependencies.json"]
vendor_dir = "deps"

[[target]]
name = "greeting"
kind = "elixir_library"
srcs = ["lib/**/*.ex"]
deps = ["./mix_dependencies"]
```

The snapshot contains the exact `manifest` and `lockfile` text, the selected
`mix_env`, a `once_inputs` map, the normalized `lock` object, and a
`dependencies` array. `once_inputs` contains every exact resolver input except
the snapshot itself. Include each path dependency manifest in
`resolver_inputs`. Graph loading rejects any binding that differs from the
target.
Each dependency records `app`, `dependencies`, `destination`, `manager`,
`top_level`, `path_dependency`, and `custom_compile`. Check in this file to keep
ordinary graph queries independent of the Elixir toolchain. Omit it while
refreshing dependencies so Mix evaluates the active graph, then review and
record the normalized result before restoring tool-free queries.

Map every active path dependency to an ordinary first-party target. The mapped
target participates in the imported root and transitive edges instead of being
silently omitted:

```toml
[target.attrs.path_dependencies]
local_helper = "./local_helper"
```

## Supported Sources and Build Managers

Locked Hex and Git entries are supported. A generated package must use Mix as
its build manager. Rebar, Make, and dependency declarations that override the
compile command fail explicitly. Dedicated target kinds are required before
Once can model those tools and their outputs safely.

The live graph reader requires Erlang 27 or newer for its built-in JavaScript
Object Notation encoder. Checked-in snapshots do not require Mix or Erlang
during graph loading.

## Sources

- [Mix dependencies](https://hexdocs.pm/mix/Mix.Tasks.Deps.html) defines the
  project dependency forms, environment selection, and native graph behavior.
- [Hex usage](https://hex.pm/docs/usage) explains repeatable fetching and why
  the lockfile should be committed.
- [Hex package checksums](https://hex.pm/docs/faq) explains the integrity role
  of package checksums preserved by generated targets.
