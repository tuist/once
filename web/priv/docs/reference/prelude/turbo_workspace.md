# `turbo_workspace`

Native [Turborepo](https://turborepo.com) workspace seed.

## Description

`turbo_workspace` asks turbo for its package graph (`turbo query`) and for
the dry run of the requested tasks (`turbo run --dry=json`) at graph load
time, and emits one task target per package and task pair. Dependency edges
are the ones turbo resolves from `dependsOn`, including the upstream `^`
form, `package#task` references, and package configurations that extend
the root `turbo.json`.

Turborepo remains authoritative for package discovery and task
configuration. Struo schedules, caches, and optionally executes remotely
the tasks it emits without shelling back into `turbo run`. Tasks named
`test` become [`turbo_test`](/docs/reference/prelude/turbo_test) targets;
the rest become [`turbo_task`](/docs/reference/prelude/turbo_task) targets.

The default task filter emits every `build`, `test`, and `lint` task,
together with the tasks they depend on. Names that no package defines are
skipped. Set `targets` to the empty list to include every task. Tasks
without a script are bridged: their dependents depend on the runnable tasks
behind them. Persistent and interactive tasks are not emitted.

Wildcard environment patterns are expanded by turbo against the caller's
environment, so a changed environment reloads the graph.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `node` | string | no | `node` | Node.js executable name, absolute path, or workspace-relative path |
| `targets` | list&lt;string&gt; | no | `["build", "test", "lint"]` | Task names to emit with the tasks they depend on; empty list includes every task |
| `resolver_inputs` | list&lt;string&gt; | no | `srcs` | Text inputs available while deriving the graph |
| `graph_file` | string | no |  | Optional workspace-relative graph snapshot with `packages` (the package items of `turbo query`) and `dry` (the output of `turbo run --dry=json`). Struo reads it directly instead of invoking turbo |
| `_default_test_roots` | list&lt;string&gt; | no | `[]` | Resolver-owned test target names selected by targetless `struo test` |

## Providers

The target emits `turbo_workspace`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | none |

Building the seed builds every emitted `build` task.

## Direct Use

Install the workspace's Node.js dependencies with its package manager so
that `node_modules/turbo` is present:

```sh
npm install
```

Struo never installs Node.js dependencies or updates lock files.

Inspect the automatically derived graph:

```sh
struo query workspace
struo query targets --kind turbo_task
```

No `struo.toml` is required. To configure the resolver explicitly, author a
target equivalent to:

```toml
[[target]]
name = "turbo"
kind = "turbo_workspace"
srcs = ["turbo.json", "package.json"]

[target.attrs]
resolver_inputs = ["turbo.json", "package.json", "**/package.json"]
targets = ["build", "test", "lint", "typecheck"]
```

## Example

```sh
struo query example turbo_workspace turbo-workspace-native-project
```

## Sources

- [Turborepo task configuration](https://turborepo.com/docs/reference/configuration)
  defines `dependsOn`, `outputs`, `cache`, `env`, and `passThroughEnv`.
- [`turbo run --dry`](https://turborepo.com/docs/reference/run#--dry----dry-run)
  reports the resolved task graph Struo reads.
