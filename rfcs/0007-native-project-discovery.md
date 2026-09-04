# Request for Comments 0007: Native Project Discovery

## Summary

Once can recognize an ecosystem-native project, such as `mix.exs` or
`Cargo.toml`, and expose a typed graph without requiring a hand-written
`once.toml`.

A Starlark `native_project` declaration describes how to find a project and
which ordinary target kind seeds its graph. The target kind's existing
resolver reads authoritative native metadata and emits the detailed Once
targets. This keeps one graph expansion mechanism and keeps Rust independent
of individual ecosystems.

The discovered seed is ephemeral and remains an internal graph-loading detail.
Projects use the resulting graph through the ordinary query and execution
commands without an initialization step.

Discovered targets may accept invocation-specific arguments through the
generic run capability. `once run <target> -- <arguments>` exposes those
arguments as `ctx["run"]["args"]` to the target kind. This keeps project
discovery independent of framework conventions while allowing an ecosystem
module to interpret its native command shape.

## Motivation

Native project descriptions already contain application names, sources,
dependencies, products, tests, features, environments, and releases.
Transcribing that information before trying Once creates duplicate authority
and makes adoption unnecessarily difficult.

Native project discovery must preserve these invariants:

- target kinds remain typed and discoverable;
- native manifests and lockfiles remain authoritative;
- actions declare inputs and outputs for caching and scheduling;
- diagnostics remain structured;
- discovery never writes a native manifest or `once.toml`;
- generic Rust code does not recognize Mix, Cargo, or another ecosystem by
  name.

## Declaration

Starlark modules export declarations beside target kinds:

```starlark
mix = native_project(
    name = "mix",
    target_kind = "mix_workspace",
    target_name = "mix",
    docs = "Recognizes a native Mix project from mix.exs.",
    markers = ["mix.exs"],
    inputs = [
        "mix.lock",
        ".formatter.exs",
        "config/**/*.exs",
    ],
    exclude = ["deps", "_build"],
    on_match = "descend",
    max_depth = 16,
    requires_tools = ["elixir"],
)
```

The declaration is data. It cannot execute project code or declare actions.
It owns only discovery:

- `markers` are normalized relative files that must all exist in one
  directory. The first marker drives the scan.
- `inputs` are optional text globs added to the seed's resolver inputs.
  Markers are always included.
- `target_kind` and `target_name` identify the ordinary seed target.
- `exclude` and `max_depth` bound discovery.
- `on_match` is `stop` or `descend`.
- `requires_tools` documents executables needed when resolution runs.

Rust validates that `target_kind` names an exported target kind. Native
metadata interpretation belongs to that kind's resolver.

For the declaration above, discovery synthesizes this target:

```toml
[[target]]
name = "mix"
kind = "mix_workspace"
srcs = ["mix.exs"]

[target.attrs]
resolver_inputs = [
  "mix.exs",
  "mix.lock",
  ".formatter.exs",
  "config/**/*.exs",
]
```

The resolver supplies detailed targets through the same typed graph path used
by authored resolver targets. There is no second proposal evaluator.

## Discovery

Discovery is generic and deterministic:

1. Load `native_project` declarations from built-in and configured Starlark
   modules.
2. Walk the configured workspace boundary without following symbolic links.
3. Skip hidden directories and names in `exclude`.
4. Stop at `max_depth`.
5. Match a directory only when every marker exists there.
6. For `on_match = "stop"`, keep the shallowest match and suppress matching
   descendants. For `"descend"`, retain nested matches.
7. Sort matches by package and declaration name.

Detection reads names only. It does not evaluate executable manifests, invoke
a package manager, access the network, or write files.

Normal graph loading may invoke a native tool through trusted analysis. An
ecosystem resolver may ask its package manager to create a missing lockfile
before importing pinned dependencies. Missing tools, unresolved dependencies,
or unavailable sources surface as structured diagnostics.

## Precedence

Discovered seeds fill uncovered ecosystem entry points:

- An authored target with the same target kind in the same package suppresses
  the discovered seed.
- An authored target whose target kind has a resolver also suppresses a nested
  match when its declared `srcs` or `resolver_inputs` cover that match's
  primary marker.
- Non-resolver targets do not claim nested discovery merely because they have
  a broad source glob.
- Resolver-emitted targets never participate in discovery precedence.

Workspace include and exclude patterns remain the outer boundary for both
`once.toml` files and native projects.

Traversal and resolver behavior compose. A Cargo workspace uses
`on_match = "stop"` because its root resolver owns member manifests. Mix uses
`"descend"` because an umbrella resolver can point to separately discovered
child seeds while independent nested Mix projects remain valid.

## Query And Execution

The command line and
[Model Context Protocol](https://modelcontextprotocol.io/) expose discovered
targets through their ordinary graph operations:

| Command line | Model Context Protocol tool | Result |
| --- | --- | --- |
| `once query workspace` | `once_query_workspace` | Describe the loaded workspace |
| `once query targets` | `once_query_targets` | List the expanded typed graph |
| `once build <target>` | `once_build_target` | Build a discovered target |
| `once test <target>` | `once_run_tests` | Test a discovered target |

Discovery never writes `once.toml` or modifies the native manifest. A future
conversion process may produce an owned Once graph, but conversion is outside
the discovery contract.

## Compatibility

Direct discovery is part of the behavior of the Once executable and its loaded
Starlark modules. Repositories that need reproducible behavior across machines
must pin that tool and module version through their normal toolchain setup.

A separate graph pin under `.once/` would not protect fresh clones or fresh
continuous integration runners. A checked-in module lock may be introduced if
Once later supports independently versioned module packages, but it is not
part of native project discovery.

## Initial Ecosystems

### Elixir

The Mix declaration recognizes `mix.exs`. `mix_workspace` evaluates project
metadata for development, test, and production, then emits locked dependency,
application, lint, test, and release targets.

The implementation is project-neutral. It does not encode Phoenix, Ecto,
Credo, or application-specific conventions in Rust. A dependency-free Mix
project legitimately has no `mix.lock`; a project with external dependencies
receives a repair when the lockfile is absent.

### Rust

The Cargo declaration recognizes `Cargo.toml`. `cargo_workspace` uses locked,
offline Cargo metadata to emit first-party libraries, binaries, tests,
procedural macros, build scripts, and external packages.

`on_match = "stop"` lets a workspace root own its members. Cargo remains
authoritative for membership, features, resolved versions, build scripts, and
target metadata. Targets gated by unselected required features are omitted,
test targets include development dependencies, and Cargo target names are
normalized to valid Rust crate identifiers. Locked external sources are
snapshotted from Cargo's local cache into target-owned outputs, without assuming
that the project owns a `vendor` directory. Cargo's explicit workspace member
set distinguishes first-party packages from local path dependencies, and
multi-output libraries emit one target per declared Rust library crate type.

### Future Ecosystems

A third-party module can export another `native_project` that points to an
existing target kind. This supports specialized recognizers without forking
the target kind. A new ecosystem can instead introduce its own resolver seed
kind. Neither case adds an ecosystem branch to Rust.

## Caching And Determinism

Marker files and `inputs` become declared resolver inputs. Resolver module
source and toolchain identity also participate in normal graph and action
identity. Detailed targets must declare every source, dependency output, and
generated output they consume.

Resolvers must not use filesystem enumeration order, current time, temporary
absolute paths, secrets, or undeclared environment values as graph inputs.

## Alternatives Rejected

### Ecosystem detection in Rust

This couples the generic loader to filenames and behavior from every
ecosystem.

### A content-aware discovery callback

This duplicates the existing resolver evaluator. Mechanical discovery plus
ordinary resolver preview keeps the boundary clear.

### Generate the complete `once.toml`

Materializing resolver-emitted products duplicates native manifest and
lockfile state. Only the stable seed belongs in `once.toml`.

### Always delegate to the native build system

That loses fine-grained Once actions and caching when native metadata can be
lowered correctly. External build-system bridges remain available for opaque
behavior.
