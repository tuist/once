# `nx_task`

One task in a native [Nx](https://nx.dev) workspace.

## Description

`nx_task` represents a single project and task pair from Nx's project
graph. Once hashes its declared inputs, schedules it against the resolved
graph, and runs the task's underlying command directly.

Only two executor families are runnable natively today:

- `nx:run-commands` and its aliases (`@nx/run-commands:run-commands`,
  `@nrwl/run-commands:run-commands`, `@nx/workspace:run-commands`,
  `@nrwl/workspace:run-commands`). The resolver lowers `options.command`
  or `options.commands` into a shell command executed via a POSIX shell
  on Linux and macOS, or `cmd.exe` on Windows.
- `nx:run-script`. The resolver reads `metadata.runCommand`, which Nx
  pre-resolves to the package-manager invocation for the requested
  script.

Other executors emit a provider-only target that fails the build
capability with a message naming the executor.

Most attributes on this target are filled in by the `nx_workspace`
resolver. Declaring an `nx_task` by hand is only necessary when the
workspace is not fully covered by the `nx` native project.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `project` | string | yes |  | Nx project name as it appears in the project graph |
| `task` | string | yes |  | Nx target name for the project, such as `build`, `test`, or `lint` |
| `project_root` | string | no |  | Workspace-relative directory that holds the project's sources |
| `outputs` | list&lt;string&gt; | no | `[]` | Workspace-relative output paths declared by the Nx target with `{projectRoot}`, `{workspaceRoot}`, and `{projectName}` tokens already expanded |
| `executor` | string | no |  | Nx executor id, kept for provenance so cache keys segregate between executors |
| `project_type` | string | no |  | Nx project type reported by the graph: `app`, `lib`, or `e2e` |
| `runnable` | bool | no | `false` | Resolver-owned flag: true when Once was able to lower the task to a concrete command it can run |
| `commands` | list&lt;string&gt; | no | `[]` | Resolver-owned shell commands lowered from the Nx executor options |
| `command_cwd` | string | no |  | Resolver-owned working directory for the lowered command |
| `command_env` | map&lt;string, string&gt; | no | `{}` | Resolver-owned environment variables from the executor's `env` option |
| `node` | string | no | `node` | Node.js executable used to identify the toolchain in the cache key |
| `dependencies` | list&lt;string&gt; | no | `["node_modules/**/*"]` | Installed package files required at execution time |
| `config` | list&lt;string&gt; | no | `["nx.json", "package.json", "pnpm-lock.yaml", "yarn.lock", "package-lock.json"]` | Workspace-level configuration inputs |
| `args` | list&lt;string&gt; | no | `[]` | Additional arguments appended to the underlying command |
| `env` | map&lt;string, string&gt; | no | `{}` | User-supplied environment variables applied after `command_env` |
| `env_inherit` | list&lt;string&gt; | no | `[]` | Host environment variables inherited by name |

## Providers

The target emits `nx_task`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | none |

## Sources

- [Nx executors](https://nx.dev/concepts/executors-and-configurations)
  describe the per-task contract Once translates.
- [Nx run-commands](https://nx.dev/nx-api/nx/executors/run-commands)
  defines the shell command executor that Once runs natively.
- [Nx run-script](https://nx.dev/nx-api/nx/executors/run-script) defines
  the package.json script executor that Once runs natively.
