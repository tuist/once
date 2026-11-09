# `elixir_test`

Runs ExUnit tests against an `elixir_app` provider already compiled with
`mix_env = "test"`.

## Description

`elixir_test` depends on exactly one target that provides a complete
`elixir_app` record, such as `elixir_library` or `mix_project`. The application
must compile with `mix_env = "test"`. During the test action, Once creates a
test layout with copies of the compiled application and dependency bytecode,
then runs the tests without recompiling the application.

That split lets a changed test file rerun tests without recompiling the
application for explicitly declared targets. Native discovery uses conservative
project-wide inputs, so a test-file change can also invalidate compilation.
A changed library source invalidates both the compiled library and the dependent
test result.
By default, tests run through direct ExUnit with `elixir`, so a package does not
need `mix.exs`. A `mix_project` dependency selects Mix mode automatically.
Once includes declared project, configuration, data, tool, test, and lockfile
inputs in the action identity before running `mix test --no-compile
--no-deps-check`. By default, the command runs in the original package directory,
with a separate build tree. Library bytecode must already be built by Once in
both modes. Mix mode retains native test aliases while consuming the
already-compiled application. It loads
modules used by Mix's recompilation probes without rerunning project compilers.
Application resources, compile metadata, and protocol consolidation outputs are
staged as explicit action inputs, including when a previous scratch directory
has been removed. Command output is streamed to log artifacts rather than
retained in memory. Mix-mode summaries come from structured ExUnit formatter
events rather than log text. Skipped tests count once, excluded tests are omitted
from executed totals, and invalid tests caused by failed setup count as failures.
This avoids differences between Elixir versions and summary-looking test output.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `_mix_test_defaults` | bool | no | `false` | Resolver-owned marker that delegates test path and file-pattern selection to Mix |
| `staged_workspace` | bool | no | `false` | Run Mix tests in a fresh private copy of declared files with a native `_build/test` layout |
| `mix_config` | string | no | empty | Optional package-relative Mix project file. When omitted, tests run through direct ExUnit without requiring a Mix project |
| `lockfile` | string | no | `mix.lock` | Package-relative lockfile staged when present |
| `config` | list&lt;string&gt; | no | `["config/**/*.exs"]` | Config file globs included in the test action key |
| `config_files` | list&lt;string&gt; | no | `[]` | Buck-compatible alias for additional config file globs |
| `data` | list&lt;string&gt; | no | `[]` | Data file globs available during tests |
| `os_env` | map&lt;string, string&gt; | no | `{}` | Buck-compatible environment variables exported before running tests |
| `env_inherit` | list&lt;string&gt; | no | `[]` | Host environment variable names inherited before explicit `env` values |
| `env` | map&lt;string, string&gt; | no | `{}` | Environment variables exported before running tests |
| `test_args` | list&lt;string&gt; | no | `[]` | Additional arguments appended to the test runner |
| `setup_tasks` | list&lt;list&lt;string&gt;&gt; | no | `[]` | Ordered Mix tasks and their exact arguments run before the test task |
| `elixir_opts` | list&lt;string&gt; | no | `[]` | Bazel-compatible options passed to the direct Elixir interpreter before test files |
| `setup` | string | no | empty | Shell snippet run after the test environment is prepared and before ExUnit starts |
| `no_start` | bool | no | `false` | Pass `--no-start` to `mix test` when `mix_config` enables Mix mode |
| `ez_deps` | list&lt;string&gt; | no | `[]` | Reserved archive dependencies |
| `tools` | list&lt;string&gt; | no | `[]` | Package-relative executable or support-file globs available to the test setup command |
| `labels` | list&lt;string&gt; | no | `[]` | Labels exposed through `once_test_info` |
| `timeout_ms` | int | no | empty | Optional test timeout in milliseconds |
| `cacheable` | bool | no | `true` | Whether successful test results may be restored from the action cache |

A non-empty `ez_deps` value fails validation because Once cannot place Erlang
archives on the ExUnit code path.

`elixir_opts` applies only to direct ExUnit mode. When `mix_config` selects Mix
mode, use `test_args`; combining Mix mode with `elixir_opts` fails validation.
`setup_tasks` also requires Mix mode. Set `cacheable = false` when setup or the
test suite must exercise an external service on every invocation. Uncacheable
tests inherit the host environment, home directory, and tool search path.
Native discovery selects this mode automatically; explicitly declared targets
retain the cacheable default.

### Private native test layout

Set `staged_workspace = true` for Mix suites that write or read paths relative
to `_build/test/lib`. Once recreates a private workspace from declared project,
test, configuration, data, tool, manifest, lockfile, and dependency source inputs.
Workspace-relative sibling path dependencies keep their layout. Unlisted files,
developer build outputs, and repository metadata are not copied implicitly.
Declare every fixture and dynamically loaded script through `srcs`, `data`,
`config`, or `tools`, as appropriate.

Before each execution, Once removes the previous private workspace and copies
fresh application and dependency trees, including their native `.mix` metadata,
to its `_build/test/lib` directory. Mix application lookup and cwd-relative
`_build` paths use the same tree. Tests cannot contaminate later runs by leaving
files in that copy, and their cwd-relative writes do not change developer build
outputs. The test home is fresh and private even for uncached tests; host tools
and the remaining runtime environment are still inherited for uncached tests.

This opt-in changes `File.cwd!()` and the locations of interpreted test files.
It does not rewrite compiled `__DIR__`, source paths, or native manifests. Code
that compares a compiled source directory with runtime cwd should keep the
default mode. This is not a security sandbox: absolute paths, external services,
or code that explicitly accesses the original checkout can still have effects.
Symbolic-link inputs and application trees are rejected rather than permitting
writes through links outside the private tree. Direct ExUnit mode and shell
`setup` are unsupported with this option; use exact `setup_tasks` vectors for
Mix setup. Test discovery reports the staged cwd and a `staged_workspace`
metadata flag. The private cwd is prepared by Once, not a standalone command
that can be executed before staging.

For generated native test targets, configure
[`mix_workspace.test_staged_workspace`](/reference/prelude/mix_workspace).

Each setup task is an exact argument vector. Do not include the `mix`
executable:

```toml
setup_tasks = [
  ["ecto.create", "--quiet", "--no-compile", "--no-deps-check"],
  ["ecto.migrate", "--quiet", "--no-compile", "--no-deps-check"],
]
```

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
