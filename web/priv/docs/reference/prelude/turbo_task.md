# `turbo_task`

One package task in a native [Turborepo](https://turborepo.com) workspace.

## Description

`turbo_task` represents a single package and task pair from turbo's task
graph. Struo hashes its declared inputs, schedules it against the resolved
graph, and runs the package script directly from the package directory
through a POSIX shell on Linux and macOS, or `cmd.exe` on Windows. The
task's `PATH` holds the `node_modules/.bin` directories from the package
directory through the workspace root, the directories of Node.js and of
the workspace's package manager (from the root `packageManager` field or the
lockfile), and the system directories. It does not include your
shell's `PATH`, so changing it does not invalidate cached tasks. List `PATH`
in turbo.json `passThroughEnv` to pass it through; it then becomes part of the
cache key. `HOME` is likewise only passed when turbo.json lists it.
Package manager lifecycle hooks such as `prebuild` are not run.

Most attributes are filled in by the
[`turbo_workspace`](/docs/reference/prelude/turbo_workspace) resolver.
Declaring a `turbo_task` by hand is only necessary when the workspace is
not covered by the `turbo` native project.

Inputs are the files of every directory in `source_dirs`, excluding every
task's declared outputs, `node_modules`, and `.turbo`; the workspace
configuration files in `config` and `global_inputs`; the installed
packages in `dependencies` and in each source directory's `node_modules`;
and the output snapshots of upstream tasks, transitively. An empty
`source_dirs` entry selects the whole workspace.

Turborepo outputs are cache patterns, not required products. After a
successful run, Struo stores the existing matches in a task-owned snapshot,
honoring `!` negations, and restores them to their original workspace
paths. Absent outputs are valid. With `cache` set to false, the task runs
on every build and its outputs are neither snapshotted nor restored.

The action needs no command filesystem setup or stream redirection, and
unpacks missing upstream outputs from their snapshots before running, so
any execution provider can run it with `struo build --remote`.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `package` | string | yes |  | Workspace package name, or `//` for a root task |
| `task` | string | yes |  | Turborepo task name, such as `build`, `test`, or `lint` |
| `command` | string | yes |  | The package script the task runs, executed through the platform shell from `package_dir` |
| `package_dir` | string | no |  | Workspace-relative package directory; empty for the workspace root |
| `outputs` | list&lt;string&gt; | no | `[]` | Workspace-relative output globs, with `!` negations. Existing matches are cached and restored; absent outputs are valid |
| `cache` | bool | no | `true` | False when turbo.json disables caching; the action always runs and its outputs are not restored |
| `source_dirs` | list&lt;string&gt; | no | `[]` | Workspace-relative directories whose files are inputs: the package and its internal workspace dependencies. An empty string selects the whole workspace |
| `other_packages` | list&lt;string&gt; | no | `[]` | Workspace packages outside `source_dirs`; their `node_modules` links are left out of the inputs |
| `workspace_outputs` | list&lt;string&gt; | no | `[]` | Resolver-owned output globs of every task, excluded from source inputs |
| `global_inputs` | list&lt;string&gt; | no | `[]` | Workspace-relative files from `globalDependencies` |
| `node` | string | no | `node` | Node.js executable that runs the task wrapper |
| `dependencies` | list&lt;string&gt; | no | `["node_modules/**/*"]` | Installed package files required at execution time; each source directory's `node_modules` is added |
| `config` | list&lt;string&gt; | no | root `package.json`, lockfiles, `turbo.json`, `turbo.jsonc`, `pnpm-workspace.yaml`, `.yarnrc.yml`, `.npmrc`, and `tsconfig*.json` | Workspace-level configuration inputs |
| `env` | map&lt;string, string&gt; | no | `{}` | User-supplied environment variables applied last |
| `env_inherit` | list&lt;string&gt; | no | `[]` | Host environment variables inherited by name, from `env`, `passThroughEnv`, and their global forms |

## Providers

The target emits `turbo_task`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | none |

## Example

```sh
struo query example turbo_task turbo-task
struo build greeting__build
```

## Sources

- [Turborepo task configuration](https://turborepo.com/docs/reference/configuration)
  describes the per-task contract Struo translates.
