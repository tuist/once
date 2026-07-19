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
| `[modules]` | Loads project target-kind modules from `paths`. |
| `[infrastructures.<name>]` | Declares a named infrastructure provider. |
| `[infrastructure.cache]` | Chooses the shared cache provider. |
| `[infrastructure.execution]` | Chooses the remote execution provider. |

Package manifests that declare `[modules]` are rejected. Keep shared module
and infrastructure configuration at the workspace root so every package sees
the same definitions.

See [Modules](/reference/modules/) and [Infrastructure](/guide/infrastructure/)
for the contracts owned by those tables.

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

## Conditional Values

Target kinds can accept `select` values for configurable attributes. A target
can also select dependencies for the current host:

```toml
[target.deps.select]
macos = ["./apple_support"]
linux = ["./linux_support"]
default = ["./portable_support"]
```

The target-kind schema determines which attribute values are configurable.
Each ecosystem guide documents its selection tokens and restrictions.

## Inspect

Use the graph before running work:

```sh
once query targets
once query target apps/hello/hello
once query schema rust_binary
```

These commands show resolved target identifiers, selected target kinds,
dependencies, capabilities, and accepted attributes.
