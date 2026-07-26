# `vitest_test`

JavaScript and TypeScript tests run with Vitest.

## Start Here

Install Vitest in the package with the project's JavaScript package manager,
or make the `vitest` command available on the executable search path. For
example:

```sh
npm install --save-dev vitest
```

Retrieve the runnable starter when you want a complete declaration, package
manifest, and sample tests:

```sh
once query example vitest_test vitest-test-minimal --format json
```

Copy the declaration below into `once.toml`, adjust `srcs` and any setup files
for the project, then establish discovery and run the file batches:

```sh
once test tests --format json
once query test-manifest tests --format json
once test tests --jobs 4 --format json
```

The first command runs the complete target. Later runs can use the resulting
manifest for automatic batching and exact selection. See
[Testing and Scheduling](/guide/graph/testing) for affected plans, case-level
batching, and exact unit commands.

## Description

`vitest_test` prefers the package-local Vitest entry point, then falls back to
the installed `vitest` command. It uses Vitest's structured reporter, records
stable file and full-name identifiers, and writes normalized Once results.
Exact execution selects the discovered file and an anchored test name pattern.

The target kind declares Node.js and Vitest as tool requirements. A
workspace-relative `runner` is treated as an input, while an installed runner
is identified as part of the toolchain.

Automatic batching uses one batch per test file by default. Set `batching` to
`case` for individual cases, or `target` for one target batch. A complete run
must establish a current manifest before smaller batches are planned.
Once disables Vitest's mutable local cache inside the action because the
installed packages are declared inputs. Once's action cache reuses successful
test results when those inputs have not changed.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `node` | string | no | `node` | Node.js executable name, absolute path, or workspace-relative path |
| `runner` | string | no | automatic | Package-relative Vitest entry point or installed command. Once prefers `node_modules/vitest/vitest.mjs`, then searches for `vitest` |
| `config` | list&lt;string&gt; | no | package and lock files | Dependency and runner configuration inputs |
| `dependencies` | list&lt;string&gt; | no | `node_modules/**/*` | Installed runner and package files required during execution |
| `data` | list&lt;string&gt; | no | `[]` | Setup, transform, snapshot, and runtime inputs |
| `args` | list&lt;string&gt; | no | `[]` | Additional Vitest arguments |
| `env` | map&lt;string, string&gt; | no | `{}` | Explicit test environment variables |
| `env_inherit` | list&lt;string&gt; | no | `[]` | Host environment variables inherited by name |
| `batching` | string | no | `file` | Automatic granularity: `file`, `case`, or `target` |
| `labels` | list&lt;string&gt; | no | `[]` | Labels exposed through test discovery |
| `timeout_ms` | int | no | empty | Optional timeout in milliseconds |

## Dependency Edges

`deps` names targets whose changes should select this test target.

## Providers and Capabilities

The target emits `once_test_info` and exposes `test` with the `default`,
`test_results`, and `logs` output groups.

## Example

```toml
[[target]]
name = "tests"
kind = "vitest_test"
srcs = ["tests/**/*.test.js"]
```

Include setup files, transforms, snapshots, and other runtime files in `data`.
