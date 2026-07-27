# `minitest_test`

Ruby Minitest files run through Once.

## Start Here

The selected `ruby` interpreter must be able to load `minitest/autorun`. Many
Ruby installations include Minitest. When the project needs the standalone
gem, install it in the Ruby environment used by Once. The interpreter may be
a name on the executable search path, an absolute path, or a
workspace-relative path:

```sh
gem install minitest
```

Retrieve the runnable starter when you want a complete declaration and sample
tests:

```sh
once query example minitest_test minitest-test-minimal --format json
```

Copy the declaration below into `once.toml`, adjust `srcs` and `ruby` for the
project, then establish discovery and run the file batches:

```sh
once test tests --format json
once query test-manifest tests --format json
once test tests --jobs 4 --format json
```

The first command runs the complete target. Later runs schedule each
discovered test file independently. Minitest methods are not separate Once
units. See [Testing and Scheduling](/guide/graph/testing) for affected plans
and file-level exact execution.

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
| `ruby` | string | no | `ruby` | Ruby interpreter name, absolute path, or workspace-relative path that can load Minitest |
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

Shared helpers and runtime files must be included in `srcs` or `data`.
