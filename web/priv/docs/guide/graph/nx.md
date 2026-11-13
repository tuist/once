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
once query workspace
once query targets --kind nx_task
```

No `once.toml` is required, and these commands do not write one. The
`nx_workspace` seed runs `nx graph` once at load time, reads the resulting
project graph, and emits one target per project and task pair: `nx_test` for
tests and `nx_task` for other tasks.
Dependency edges honor Nx's `dependsOn`, including the upstream `^` form
and the object form.

By default, Once emits the `build`, `test`, and `lint` tasks for every
project. Adjust the filter through the seed's `targets` attribute, or set
it to the empty list to include every task in the graph. Tasks required
by `dependsOn` are retained transitively even when their names are outside
the filter, so a build that depends on `compile` keeps its prerequisite.
Upstream dependencies traverse projects without the requested task until
they reach a project that defines it.

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
rerunning that command.

Nx declares potential outputs, including coverage directories that may not
exist unless coverage is enabled. Once caches the existing matches in a
task-owned snapshot and restores them to their original workspace paths.
Missing matches are valid for Nx tasks; other Once action outputs remain
required. Glob patterns use the workspace's installed Nx matcher.
Generated Vite caches under `node_modules/.vite` are excluded from inputs,
so a test runner's timing records do not invalidate its next run.

## Executor Coverage

Nx tasks describe how their work runs through an `executor` string.
Coverage falls into three groups today:

- `nx:run-commands` and its aliases (`@nx/run-commands:run-commands`,
  `@nrwl/run-commands:run-commands`, `@nx/workspace:run-commands`,
  `@nrwl/workspace:run-commands`) run natively. Once extracts the shell
  command from the executor's `options.command` or `options.commands` and
  runs it under a POSIX shell on Linux and macOS, or `cmd.exe` on Windows.
  `options.args` are forwarded unless `forwardAllArgs` disables forwarding
  for the task or an individual command.
- `nx:run-script` runs natively when the graph carries a resolved
  `metadata.runCommand`. Nx populates that field with the exact
  package-manager invocation (for example `pnpm run build`), which Once
  runs as a single command from the project directory.
- Plugin executors run through the installed Nx `runExecutor` API, using
  the resolved project graph and default configuration. Once owns the
  dependency schedule, action hashing, output snapshots, and cache. It does
  not invoke `nx build`, `nx test`, or Nx's task runner.

Commands retain the host's activated `PATH` after the project-local tool
directories, so package-manager shims and additional toolchains remain
available. Install and activate those tools before invoking Once. Once does
not install SDKs, restore NuGet packages, or download browser binaries.

Once conservatively hashes workspace sources and installed dependencies,
excluding declared generated outputs and runtime caches. This covers shared
configuration and cross-project sources, but can invalidate more tasks than
Nx's fine-grained named inputs. Output tokens such as `{options.outputPath.base}`
resolve against the task's default configuration before snapshotting.

### Run Tests

```sh
once test
```

Tasks named `test` become `nx_test` targets, exposing one normalized, cacheable
suite batch per project. Both application and library suites run by default.
Once runs the resolved command or plugin executor, records its exit status
and duration, and retains its log. A failed command produces a failed suite.
Suite units do not offer case listing, case filtering, or within-suite
sharding. Native runner output remains in the log; individual assertions are
not imported into the normalized suite result.

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
