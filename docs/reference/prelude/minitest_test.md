# `minitest_test`

Ruby Minitest files run through Once.

## Description

`minitest_test` treats each declared test file as one stable unit. It invokes
each selected file with Ruby, captures its output, and writes normalized Once
results. This makes file selection exact without depending on Minitest class or
method naming conventions.

After a complete run, Once schedules the files as independent batches and
balances them using historical durations. Individual Minitest methods are not
separate Once units.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `ruby` | string | no | `ruby` | Ruby interpreter that can load Minitest |
| `config` | list&lt;string&gt; | no | `Gemfile`, `Gemfile.lock` | Dependency and runner configuration inputs |
| `data` | list&lt;string&gt; | no | `[]` | Runtime data and support inputs |
| `args` | list&lt;string&gt; | no | `[]` | Arguments passed to each test file |
| `env` | map&lt;string, string&gt; | no | `{}` | Explicit test environment variables |
| `env_inherit` | list&lt;string&gt; | no | `[]` | Host environment variables inherited by name |
| `batching` | string | no | `file` | Accepted for parity; file units remain the scheduling boundary |
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
kind = "minitest_test"
srcs = ["test/**/*_test.rb"]
```

Minitest must be available to the selected interpreter. Shared helpers and
runtime files must be included in `srcs` or `data`.
