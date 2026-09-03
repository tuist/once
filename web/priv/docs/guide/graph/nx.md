---
prev: false
next: false
---

# Nx

Once can read an existing [Nx](https://nx.dev) workspace, derive a typed
build graph from its projects and tasks, and schedule each task through
Once's cache and remote execution. You can query, build, and test the
workspace without translating anything into `once.toml`.

## Start With an Existing Nx Workspace

### Check the Toolchain

Once invokes Node.js and the local Nx binary. Confirm both are available and
that your workspace's dependencies are installed:

```sh
node --version
nx --version
```

If the repository pins Node with [mise](https://mise.jdx.dev/) or similar,
install and activate that configuration first. Once prefers the workspace
copy of Nx (under `node_modules/nx`) over one on `PATH`, so a plain
`npm install`, `pnpm install`, or `yarn install` is normally enough.

### Preview the Derived Graph

From the directory that contains `nx.json`, inspect the match and the
emitted targets:

```sh
once native list
once query targets --kind nx_task
```

No `once.toml` is required, and these commands do not write one. The
`nx_workspace` seed runs `nx graph` once at load time, reads the resulting
project graph, and emits one `nx_task` target per project and task pair.
Dependency edges honor Nx's `dependsOn`, including the upstream `^` form
and the object form.

By default, Once emits the `build`, `test`, and `lint` tasks for every
project. Adjust the filter through the seed's `targets` attribute, or set
it to the empty list to include every task in the graph.

### Build a Target

Once schedules and caches each task itself instead of routing back through
`nx run`. For a task whose Nx executor is `nx:run-commands` or
`nx:run-script`, Once reads the resolved command from the graph and runs
it directly, so cache keys, remote execution, and insights all live in
Once:

```sh
once build my_app__build
```

The first run is a cache miss and executes the underlying command. A
second run with unchanged inputs is a cache hit and completes without
running Node.js.

## Executor Coverage

Nx tasks describe how their work runs through an `executor` string.
Coverage falls into three groups today:

- `nx:run-commands` and its aliases (`@nx/run-commands:run-commands`,
  `@nrwl/run-commands:run-commands`, `@nx/workspace:run-commands`,
  `@nrwl/workspace:run-commands`) run natively. Once extracts the shell
  command from the executor's `options.command` or `options.commands` and
  runs it under a POSIX shell on Linux and macOS, or `cmd.exe` on Windows.
- `nx:run-script` runs natively when the graph carries a resolved
  `metadata.runCommand`. Nx populates that field with the exact
  package-manager invocation (for example `pnpm run build`), which Once
  runs as a single command.
- Other executors (`@nx/webpack:webpack`, `@nx/jest:jest`,
  `@angular-devkit/build-angular:*`, plugin executors) surface as
  informational `nx_task` targets that fail the build capability with a
  message that names the executor. The graph still loads and the
  dependency relationships are visible to `once query`.

The distribution varies by workspace. In practice, most workspaces built
around `run-commands` or `run-script` tasks run natively in full. A
workspace that leans on plugin executors will need adapters for those
executors before Once can run them.

## Configuration

`nx_workspace` accepts a small number of attributes:

- `targets` selects which task names to emit. Defaults to `build`, `test`,
  and `lint`. Set it to the empty list to include every task in the graph.
- `graph_file` points at a checked-in `nx graph --view=projects` JSON
  snapshot. When set, Once reads the snapshot directly instead of running
  Nx. This is useful in environments that do not install Node.js and
  `node_modules/nx`, such as documentation builds and reproducibility
  tests.

`nx_task` carries the per-task attributes Once needs to run the underlying
command: `project`, `task`, `project_root`, resolved `commands`, and their
`command_cwd`, `command_env`, and `outputs`. The seed's resolver fills
these from the Nx graph, so you rarely edit them by hand.

## Update Workflow

Nx remains authoritative for its own graph. When you update `nx.json`,
add or remove a project, or bump `dependsOn`, Once picks up the change on
the next graph load. The workspace's package manager still owns installing
Node.js dependencies. Once neither installs them nor updates lock files.
