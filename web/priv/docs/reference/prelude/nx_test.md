# `nx_test`

A cacheable test suite from an [Nx](https://nx.dev) workspace.

## Description

The native Nx resolver emits `nx_test` for tasks named `test`. Other tasks
remain [`nx_task`](/docs/reference/prelude/nx_task) targets and do not advertise
the test capability. The workspace seed reports both application and library
suites as first-party test roots, so `once test` runs all of them.

Commands and plugin executors use the same action contract as `nx_task`.
Once owns scheduling, input hashing, caching, output snapshots, and restoration.
Plugin actions invoke the installed Nx `runExecutor` API, not Nx's task runner.

Each task is one suite unit in `once.test_results.v1`. Its exit status determines
success or failure, and its duration and log are retained. Individual assertions
are not imported. Case listing, case filtering, and within-suite sharding are
unsupported. Failed commands cannot produce a passing suite.

## Attributes

All attributes are shared with [`nx_task`](/docs/reference/prelude/nx_task),
including `project`, `task`, `commands`, `command_cwd`, `command_env`, `outputs`,
`executor_graph`, and `workspace_outputs`.

## Providers

The target emits `nx_task` and `once_test_info`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | none, executes when used as a prerequisite |
| `test` | `default`, `test_results`, `logs` |

## Example

```sh
once query example nx_test nx-test
once test hello__test
```
