# `nx_workspace`

Native [Nx](https://nx.dev) workspace seed.

## Description

`nx_workspace` runs `nx graph --view=projects` at graph load time, reads
the resulting project graph, and emits one `nx_task` target per project
and task pair. Dependency edges honor Nx's `dependsOn`, including the
upstream `^` form and the object form.

Nx remains authoritative for project discovery, task configuration, named
inputs, and executor semantics. Once schedules and caches the tasks it
emits without shelling back into `nx run`. Tasks whose executor is
`nx:run-commands` (or its `@nx/` / `@nrwl/` aliases) or `nx:run-script`
run their underlying command directly; tasks with other executors emit
provider-only targets that fail the build capability with a message that
names the executor.

The default task filter emits every `build`, `test`, and `lint` task in
the graph. Set `targets` to the empty list to include every task Nx
reports.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `node` | string | no | `node` | Node.js executable name, absolute path, or workspace-relative path |
| `targets` | list&lt;string&gt; | no | `["build", "test", "lint"]` | Nx task names to emit; empty list includes every task |
| `resolver_inputs` | list&lt;string&gt; | no | `srcs` | Text inputs available while deriving the graph |
| `graph_file` | string | no |  | Optional workspace-relative path to a checked-in `nx graph --view=projects` JSON snapshot; when set, Once reads it directly instead of running `nx graph` |

## Providers

The target emits `nx_workspace`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | none |

## Direct Use

Install the workspace's Node.js dependencies with the project's package
manager (npm, pnpm, yarn, or bun) so that `node_modules/nx` is present:

```sh
npm install
```

Once never installs Node.js dependencies or updates lock files.

Inspect the automatically derived graph:

```sh
once query workspace
once query targets --kind nx_task
```

No `once.toml` is required. To configure the resolver explicitly, author a
target equivalent to:

```toml
[[target]]
name = "nx"
kind = "nx_workspace"
srcs = ["nx.json", "package.json"]

[target.attrs]
resolver_inputs = ["nx.json", "package.json", "**/project.json", "**/package.json"]
```

Keep this seed as the only Once target while Nx's graph describes the
workspace. Add explicit targets beside it only for exceptional boundaries
that Nx does not cover.

## Sources

- [`nx graph`](https://nx.dev/nx-api/nx/documents/graph) defines the
  project graph consumed by the native integration.
- [Nx executors](https://nx.dev/concepts/executors-and-configurations)
  describe the per-task contract Once translates.
