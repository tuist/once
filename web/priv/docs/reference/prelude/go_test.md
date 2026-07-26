# `go_test`

Compiled Go test package with exact filtering, normalized results, native event
records, and optional coverage.

The target accepts every [common Go attribute](/reference/prelude/go_source#common-go-attributes),
the link attributes from [`go_binary`](/reference/prelude/go_binary#attributes),
and these test-specific attributes:

| Attribute | Type | Default | Description |
| --- | --- | --- | --- |
| `args` | list&lt;string&gt; | `[]` | Arguments passed to the compiled test binary |
| `test_env` | map&lt;string, string&gt; | `{}` | Test environment values |
| `env_inherit` | list&lt;string&gt; | `[]` | Host environment names inherited by the runner |
| `rundir` | string | empty | Package-relative working directory for the test binary |
| `labels` | list&lt;string&gt; | `[]` | Labels exposed through generic test discovery |
| `timeout_ms` | int | empty | Optional test timeout in milliseconds |
| `cover_packages` | list&lt;string&gt; | `[]` | Import path patterns included in coverage instrumentation |
| `fail_fast` | bool | `false` | Stop after the first failing test |
| `short` | bool | `false` | Request Go short test mode |
| `count` | int | `0` | Run selected tests this many times when greater than zero |
| `parallel` | int | `0` | Maximum tests run concurrently inside the test binary |
| `shuffle` | string | empty | `on`, `off`, or a shuffle seed |

## Dependency Edges

The target accepts the normal `deps`, `embed`, and `cdeps` roles. Its
`target_under_test` role accepts `go_package` and composes the Buck2-compatible
library or binary sources into the test package.

## Providers, Capabilities, and Outputs

The target emits `go_package` and `once_test_info`.

| Capability | Output groups | Requires |
| --- | --- | --- |
| `build` | `binary` | none |
| `test` | `default`, `test_results`, `logs`, `coverage` | `binary` |

The test action writes `test_results.json`, `go-test.log`, and
`native_results.jsonl` under `.once/out/<target>/test/`. With coverage enabled,
it also writes `coverage.out` and records it in the normalized artifacts map.
Benchmarks are discovered as skipped units during a normal test run and can be
selected exactly later. Test execution requires the host operating system and
architecture.
