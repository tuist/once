# `elixir_test`

Runs ExUnit tests against an `elixir_library` already compiled with
`mix_env = "test"`.

## Description

`elixir_test` depends on exactly one `elixir_library` target. The library must
compile with `mix_env = "test"`. During the test action, Once creates a test
layout with symlinks to the compiled library and dependency bytecode, then runs
the tests without recompiling the application.

That split lets a changed test file rerun tests without recompiling the
application. A changed library source invalidates both the compiled library and
the dependent test result.
By default, tests run through direct ExUnit with `elixir`, so a package does not
need `mix.exs`. Set `mix_config` when a target should run through `mix test
--no-compile --no-deps-check` instead. Library bytecode must already be built by
Once in both modes.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `mix_config` | string | no | empty | Optional package-relative Mix project file. When omitted, tests run through direct ExUnit without requiring a Mix project |
| `config` | list&lt;string&gt; | no | `["config/**/*.exs"]` | Config file globs included in the test action key |
| `config_files` | list&lt;string&gt; | no | `[]` | Buck-compatible alias for additional config file globs |
| `data` | list&lt;string&gt; | no | `[]` | Data file globs available during tests |
| `os_env` | map&lt;string, string&gt; | no | `{}` | Buck-compatible environment variables exported before running tests |
| `env_inherit` | list&lt;string&gt; | no | `[]` | Host environment variable names inherited before explicit `env` values |
| `env` | map&lt;string, string&gt; | no | `{}` | Environment variables exported before running tests |
| `test_args` | list&lt;string&gt; | no | `[]` | Additional arguments appended to the test runner |
| `elixir_opts` | list&lt;string&gt; | no | `[]` | Bazel-compatible options passed to the direct Elixir interpreter before test files |
| `setup` | string | no | empty | Shell snippet run after the test environment is prepared and before ExUnit starts |
| `no_start` | bool | no | `false` | Pass `--no-start` to `mix test` when `mix_config` enables Mix mode |
| `ez_deps` | list&lt;string&gt; | no | `[]` | Reserved archive dependencies |
| `tools` | list&lt;string&gt; | no | `[]` | Package-relative executable or support-file globs available to the test setup command |
| `labels` | list&lt;string&gt; | no | `[]` | Labels exposed through `once_test_info` |
| `timeout_ms` | int | no | empty | Optional test timeout in milliseconds |

A non-empty `ez_deps` value fails validation because Once cannot place Erlang
archives on the ExUnit code path.

`elixir_opts` applies only to direct ExUnit mode. When `mix_config` selects Mix
mode, use `test_args`; combining Mix mode with `elixir_opts` fails validation.

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `elixir_app` | The test-environment Elixir application under test |

## Providers

The target emits `once_test_info`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `test` | `default`, `test_results`, `logs` |

## Example

```toml
[[target]]
name = "greeting"
kind = "elixir_library"
srcs = ["lib/**/*.ex"]

[target.attrs]
app_name = "greeting"
mix_env = "test"

[[target]]
name = "greeting_test"
kind = "elixir_test"
srcs = ["test/**/*_test.exs"]
deps = ["./greeting"]

[target.attrs]
labels = ["unit"]
```
