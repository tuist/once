# `once.toml` Manifest

Once uses `once.toml` files for workspace configuration and typed graph
targets. The file at the workspace root owns settings shared by the whole
repository. Additional manifests declare targets for the package directory
that contains them.

Script execution metadata does not belong in a manifest. Annotated scripts
keep their contract in `# once` headers described by
[Scripted Automation](/guide/scripted/).

## Discovery

By default, Once discovers every `once.toml` below the workspace root while
skipping hidden directories. The root manifest can limit that scan:

```toml
[workspace]
include = ["apps/*/once.toml", "packages/*/once.toml"]
exclude = ["packages/experimental/once.toml"]
```

`include` and `exclude` contain workspace-relative glob patterns. An excluded
path is always omitted. When `include` is empty, every discovered manifest is
included unless `exclude` removes it.

## Root-only Tables

These tables are read only from the root manifest:

| Table | Purpose |
| --- | --- |
| `[workspace]` | Limits manifest discovery with `include` and `exclude`. |
| `[workspace.configuration]` | Selects the target operating system, architecture, and additional configuration tokens. |
| `[modules]` | Loads project target-kind modules from `paths`. |
| `[infrastructures.<name>]` | Declares a named infrastructure provider. |
| `[infrastructure.cache]` | Chooses the shared cache provider. |
| `[infrastructure.execution]` | Chooses the remote execution provider. |

Package manifests that declare `[modules]` are rejected. Keep shared module
and infrastructure configuration at the workspace root so every package sees
the same definitions.

See [Modules](/reference/modules/) and [Infrastructure](/guide/infrastructure/)
for the contracts owned by those tables.

The target configuration defaults to the host. Set it explicitly when
analysis and dependency selection should describe another platform:

```toml
[workspace.configuration]
os = "linux"
arch = "arm64"
tokens = ["release"]
```

Once normalizes common operating system and architecture names. For example,
`darwin` becomes `macos`, `arm64` becomes `aarch64`, and `amd64` becomes
`x86_64`. The ordered selection tokens include the most specific
operating-system and architecture combinations, their aliases, the individual
values, the additional `tokens`, and `default`. Use `once query workspace` to
inspect the exact configuration seen by target kinds.

An execution-only provider can select a sandbox adapter and immutable tool
environment:

```toml
[infrastructures.remote_tests]
kind = "microsandbox"
image = "node:22.18.0-alpine"

[infrastructure.execution]
provider = "remote_tests"
```

The provider name is a workspace policy label. Script headers and command-line
options can select it without coupling the script to Microsandbox.

Hosted execution providers select their immutable environment with a template
or image:

```toml
[infrastructures.e2b_tests]
kind = "e2b"
template = "vitest-node-22"

[infrastructures.daytona_tests]
kind = "daytona"
image = "node:22.18.0-bookworm"
```

E2B, Daytona, and Microsandbox provide execution only. Tuist provides shared
caching. Bind each capability independently when a workspace uses both.

## Targets

The root manifest and package manifests can declare one or more targets:

```toml
[[target]]
name = "hello"
kind = "rust_binary"
srcs = ["src/**/*.rs"]
deps = ["../greeting/greeting"]

[target.attrs]
crate_root = "src/main.rs"
edition = "2024"
```

Every target accepts these common fields:

| Field | Required | Meaning |
| --- | --- | --- |
| `name` | yes | Name unique within the package. |
| `kind` | yes | Target kind that defines attributes, dependencies, providers, and capabilities. |
| `srcs` | no | Package-relative source paths or glob patterns. |
| `deps` | no | Target identifiers consumed by this target. |
| `dependencies` | no | Named dependency roles declared by the target kind. Each role contains target identifiers and validates its own provider contract. |
| `visibility` | no | Consumers allowed to depend on this target. An empty list is public. |
| `attrs` | no | Target-kind-specific values validated against the selected schema. |

Unknown manifest fields are rejected. Use
[`once query schema <kind>`](/reference/cli/query/schema) to inspect the
attributes and dependency contracts accepted by a target kind.

Use `deps` for the target kind's default dependency role. Use
`[target.dependencies]` when the schema exposes more precise roles:

```toml
[[target]]
name = "app"
kind = "rust_binary"
deps = ["./library"]

[target.dependencies]
proc_macro_deps = ["./derive"]
link_deps = ["./native"]
```

Validation checks each role against its matching schema entry. Analysis keeps
`deps` in declared order and exposes the other roles separately, so a target
kind can distinguish compiler plug-ins, runtime-only dependencies, native
links, and other semantics without hardcoding them in Once.

## Target Identifiers

A target identifier combines its package path and target name. A target named
`hello` in `apps/hello/once.toml` has the identifier `apps/hello/hello`. A
target in the root manifest uses its name directly.

Dependency references resolve as follows:

- `packages/logging/logging` is relative to the workspace root.
- `./support` is relative to the package containing the current manifest.
- `../shared/shared` moves from the current package to a sibling package.

Use one of these identifiers with `once query target`, `once build`, `once
run`, and `once test`.

Target-valued entries under `[target.attrs]` use the same normalization rules.
`once query validate-workspace` checks that these references resolve to a
declared target.

## Visibility

Targets are public unless `visibility` restricts them:

```toml
[[target]]
name = "Core"
kind = "rust_library"
visibility = ["package:apps/client", "subtree:tools", "tests/CoreTests"]
```

Each entry grants access to one class of consumer:

- `public` grants access to every target.
- `private` grants access to targets in the same package.
- `package:apps/client` grants access to targets in that exact package.
- `subtree:tools` grants access to targets in `tools` and its child packages.
- `tests/CoreTests` grants access to one exact target.

Exact targets use the same workspace-relative and package-relative
normalization as dependencies. Complete-workspace validation returns
`invalid_visibility` for malformed entries and `dependency_not_visible` when a
dependency edge crosses the declared boundary.

## Conditional Values

Target kinds can accept `select` values for configurable attributes. A target
can also select dependencies for the workspace target configuration:

```toml
[target.deps.select]
macos = ["./apple_support"]
linux = ["./linux_support"]
default = ["./portable_support"]
```

The target-kind schema reports both `configurable` and `implemented` for every
attribute. Validation rejects `select` on a non-configurable attribute and
rejects an unavailable compatibility attribute marked `implemented = false`.
Generic target kinds use the tokens returned by `once query workspace`.
Ecosystem target kinds can add their own tokens and restrictions.

## Inspect

Use the graph before running work:

```sh
once query targets
once query target apps/hello/hello
once query schema rust_binary
```

These commands show resolved target identifiers, selected target kinds,
dependencies, capabilities, and accepted attributes.
