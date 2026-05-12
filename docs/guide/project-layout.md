# Project layout

Fabrik projects are regular source trees with `fabrik.toml` files placed
next to the code they describe. The CLI starts at the project root, walks
the tree for build files, and loads them into one graph.

## Targets

A target is a named unit in the build graph. It can compile code, run a
checked-in script, produce an artifact, or represent any other operation
provided by a target type.

The target type controls the fields available in `fabrik.toml` and how
the target turns into one or more cacheable actions. The target name
identifies that target inside its build file directory.

## Project root

The project root is the directory Fabrik evaluates from. By default this
is the current directory. This command lists the targets in that project:

```sh
fabrik targets
```

Use `-C` when you want to run Fabrik from somewhere else:

```sh
fabrik -C path/to/project targets
```

The root also owns `.fabrik/`, the local cache and output directory.
Do not commit `.fabrik/`.

## Build files

Each build file is named `fabrik.toml`. A project can have one at the
root and more in subdirectories:

```txt
my-project/
  fabrik.toml
  apps/
    cli/
      fabrik.toml
      src/
  shared/
    fabrik.toml
    src/
  scripts/
    fabrik.toml
  .fabrik/
```

A `fabrik.toml` file is data, not code. Each table declares one target:

```toml
[[task]]
name = "generate"
argv = ["./scripts/generate.sh"]
srcs = ["scripts/generate.sh"]
outputs = ["generated/schema.json"]
```

## Build file scope

A `fabrik.toml` file creates a local scope for the targets declared in
that directory.

That directory scope controls three things:

- target names are local to the build file directory
- `srcs` and `src_globs` are resolved from that directory
- target IDs use the directory path as their prefix

See [Target IDs](./target-ids.md) for the exact CLI and dependency
resolution rules.

## Dependencies

Targets form a graph by listing dependencies on other targets. Inside a
`fabrik.toml` file, a bare dependency name points at another target in
the same build file directory:

```toml
deps = ["schema"]
```

Use relative paths for nearby build file directories:

```toml
deps = ["../shared/schema"]
```

Use project-root paths when the dependency should be unambiguous from
any directory:

```toml
deps = ["tools/generate"]
```

## Inputs and outputs

Targets declare the files that matter to their cache key. Common fields
include:

- `srcs`: source files relative to the build file directory.
- `src_globs`: glob inputs relative to the build file directory.
- `outputs`: project-root-relative outputs that Fabrik should restore
  from the cache when supported by the target type.
- `deps`: other targets that must be built first.

This is the contract that lets Fabrik answer why something ran, what
changed, and what can be restored from cache.

## Actions

Targets are the author-facing model. Actions are the execution model.

When you run `fabrik build`, `fabrik run`, or `fabrik test`, Fabrik loads
the targets, asks the relevant planner to expand the requested target,
and executes the resulting actions through the cache.

Most users author targets. Actions are what show up in cache diagnostics,
structured output, and lower-level debugging.

## Recommended shape

Prefer small `fabrik.toml` files near the code they describe:

- Put repo-wide entry points in the root build file.
- Put directory-specific targets next to the code they describe.
- Keep target names short and local, such as `app`, `test`, `lint`, or
  `generate`.
- Use dependencies to connect directories rather than centralizing every
  target in one file.

That layout keeps target IDs predictable for humans while giving agents
small, local files to edit.
