---
prev: false
next: false
---

# Turborepo

Struo can read an existing [Turborepo](https://turborepo.com) workspace,
derive a typed build graph from its packages and `turbo.json` tasks, and
schedule each task through Struo's cache and remote execution. You can
query, build, and test the workspace without translating anything into
`struo.toml`, and without running `turbo run`.

## Start With an Existing Turborepo Workspace

### Check the Toolchain

Struo invokes Node.js and the workspace's turbo binary while loading the
graph. Confirm both are available and that your dependencies are installed:

```sh
node --version
npx turbo --version
```

Struo requires Turborepo 2. It prefers the workspace copy of turbo (under
`node_modules/turbo`) over one on `PATH`, so a plain `npm install`,
`pnpm install`, `yarn install`, or `bun install` is normally enough.

### Preview the Derived Graph

From the directory that contains `turbo.json` or `turbo.jsonc`, inspect
the match and the emitted targets:

```sh
struo query workspace
struo query targets --kind turbo_task
```

No `struo.toml` is required, and these commands do not write one. The
`turbo_workspace` seed asks turbo for its package graph and for the dry run
of the requested tasks, which carries turbo's own resolution of
`dependsOn`, package configurations, `outputs`, `cache`, and environment
variables. It emits one target per package and task pair, named
`<package>__<task>`: `turbo_test` for `test` tasks and `turbo_task` for the
rest. Characters other than letters, digits, and underscores become
underscores, so `@acme/ui#build` becomes `_acme_ui__build`, and root tasks
such as `//#format` become `root__format`.

By default, Struo emits the `build`, `test`, and `lint` tasks together with
every task they depend on. Task names that no package defines are skipped.
Set the seed's `targets` attribute to the empty list to include every task.
Tasks without a script, such as transit nodes, do not become targets; their
dependents depend directly on the runnable tasks behind them. Persistent
and interactive tasks, such as `dev`, are left out.

### Build a Target

Struo schedules and caches each task itself. It runs the package script
from the package directory through the platform shell, with every
`node_modules/.bin` directory from the package up to the workspace root on
`PATH`, the same way package managers run scripts:

```sh
struo build web__build
```

The first run is a cache miss and executes the script. A second run with
unchanged inputs is a cache hit, restores the task's outputs, and completes
without rerunning it. Building the `turbo` seed builds every `build` task.

Tasks do not inherit your shell's `PATH`, so it stays out of their cache
keys. Their `PATH` holds those `node_modules/.bin` directories, the
directories of Node.js and of the workspace's package manager, so scripts such
as `pnpm with-env next build` still work, and the system directories. A tool
installed anywhere else must come from `node_modules`, or you can list `PATH`
in `passThroughEnv` to pass your shell's value through as a keyed input.

Turborepo `outputs` are cache patterns, not required products. Struo stores
the existing matches, honoring `!` negations and `$TURBO_ROOT$`, in a
task-owned snapshot and restores them to their original paths. A task with
`"cache": false` always runs and its outputs are not restored.

### Run Tests

```sh
struo test
```

Tasks named `test` become `turbo_test` targets, one normalized, cacheable
suite per package. Struo records the script's exit status, duration, and
log. Individual assertions are not imported, and suites do not offer case
listing, case filtering, or sharding.

## Inputs and Environment

Turbo hashes files to decide when to rerun a task, but the task itself
reads whatever is on disk. Struo instead declares exactly what each task
receives, so the same task can run in a fresh remote machine. Each task's
inputs are:

- every file in its package and in the workspace packages it depends on,
  except declared outputs of any task, `node_modules`, and `.turbo`
- the root `package.json`, lockfiles, `turbo.json`, `.npmrc`, root
  `tsconfig*.json` files, and the files from `globalDependencies`
- the installed `node_modules` of the workspace root and of those packages,
  without the links to workspace packages the task does not depend on
- the output snapshots of the tasks it depends on, transitively

A root task receives the whole workspace instead. Turbo's task `inputs`
setting does not narrow this set, because a remote worker can only read
files it receives.

Environment variables follow turbo's strict mode: a task receives the
variables listed in its `env` and `passThroughEnv`, the global forms of
both, and the variables turbo matched through wildcards and framework
inference. Their values are part of the action's cache key. `PATH` is built
as described above. `HOME` is not passed, so tools fall back to the account's
home directory and a different `HOME` does not invalidate cached tasks; list it
in `passThroughEnv` when a task needs your value.

## Remote Execution

Each task is one command action with declared inputs and outputs, so
`struo build --remote` can send uncached tasks to any configured execution
provider:

```sh
struo build --remote --compute workers web__build
```

Struo keeps scheduling and caching on your machine and ships each task's
inputs, including installed dependencies, to a fresh worker. The worker
unpacks the output snapshots of upstream tasks before running the script,
and only the task's own snapshot comes back. Tasks perform their own
cleanup and log capture, so they need no provider-specific filesystem
setup. As with other generic toolchains, the worker image must provide
Node.js, and the package manager when scripts call it, at the same paths as
your machine. See
[Remote Execution](/guide/infrastructure/remote-execution) to configure a
provider.

## Configuration

`turbo_workspace` accepts a small number of attributes:

- `targets` selects which task names to emit. Defaults to `build`, `test`,
  and `lint`. Set it to the empty list to include every task.
- `graph_file` points at a checked-in graph snapshot. Struo reads it
  instead of invoking turbo, which is useful where Node.js and
  `node_modules/turbo` are not installed. The snapshot is a JSON object with
  `packages`, the package items from `turbo query`, and `dry`, the output of
  `turbo run <tasks> --dry=json`.

`turbo_task` carries what Struo needs to run one task: `package`, `task`,
the resolved `command`, `package_dir`, `outputs`, `cache`, and the
environment variables to inherit. The seed's resolver fills these from
turbo, so you rarely edit them by hand.

## Update Workflow

Turborepo remains authoritative for its own configuration. When you edit
`turbo.json`, add a package, or change a script, Struo picks up the change
on the next graph load. The workspace's package manager still owns
installing dependencies; Struo neither installs them nor updates lock
files. Lifecycle hooks such as `prebuild` are not run, because Struo runs
the script itself rather than through the package manager.
