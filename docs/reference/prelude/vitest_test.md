# `vitest_test`

JavaScript and TypeScript tests run with Vitest.

## Description

`vitest_test` invokes the package-local Vitest entry point with its structured
reporter, records stable file and full-name identifiers, and writes normalized
Once results. Exact execution selects the discovered file and an anchored test
name pattern.

Automatic batching uses one batch per test file by default. Set `batching` to
`case` for individual cases, or `target` for one target batch. A complete run
must establish a current manifest before smaller batches are planned.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `node` | string | no | `node` | Node.js executable |
| `runner` | string | no | `node_modules/vitest/vitest.mjs` | Package-relative Vitest entry point |
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

Install Vitest in the package before running the target. Include setup files,
transforms, snapshots, and other runtime files in `data`.
