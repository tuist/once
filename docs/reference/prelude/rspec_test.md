# `rspec_test`

[Ruby Specification (RSpec)](https://rspec.info/) tests run through Once.

## Description

`rspec_test` invokes the Ruby Specification core runner with its structured
formatter, converts examples into stable target-qualified identifiers, and
writes normalized Once results. Exact execution passes the native example
identifier back to the runner.

Automatic batching uses one batch per specification file by default. Set
`batching` to `case` for individual examples, or `target` for one target batch.
A complete discovery run is required before smaller batches are planned.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `ruby` | string | no | `ruby` | Ruby interpreter that can load the Ruby Specification library |
| `config` | list&lt;string&gt; | no | `Gemfile`, `Gemfile.lock` | Dependency and runner configuration inputs |
| `data` | list&lt;string&gt; | no | `[]` | Runtime data and support inputs |
| `args` | list&lt;string&gt; | no | `[]` | Additional runner arguments |
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
kind = "rspec_test"
srcs = ["spec/**/*_spec.rb"]

[target.attrs]
env_inherit = ["GEM_HOME", "GEM_PATH"]
```

The Ruby Specification library must be available to the selected interpreter.
List helper files and other runtime inputs in `data` when they are outside the
source patterns.
