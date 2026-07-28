# `mix_project`

Compiles a first-party Mix project against separately cached Elixir
applications.

## Description

`mix_project` runs the compiler pipeline registered by `mix.exs`, including
project compilers that direct `elixirc` cannot reproduce. Dependencies arrive
through `elixir_app` providers and are loaded before compilation. The action
does not compile dependencies. It stages their declared source trees inside an
isolated workspace so compile-time macros and path dependencies observe the
same relative layout as the original workspace.

The build captures the project-owned Mix environment, including the
application directory, compiler manifests, protocol consolidation, and
outputs produced by registered project compilers. Dependency applications
remain separately cached targets and are linked into consumer layouts.
Declared source, configuration, data, private, include, and tool inputs
participate in the cache key.

The optional `run` capability consumes the existing build output. Set
`run_task` for one task, use `run_tasks` for an ordered list of exact task
argument vectors, or supply an unconfigured task with
`once run <target> -- <task>`. Server and interactive runs are uncached by default.
Deterministic checks can opt into action caching with `run_cacheable = true`.
Uncached runs inherit their parent environment so development applications can
launch locally installed tools and read user-level configuration. Explicit
target environment values take precedence. Cacheable runs retain the
constrained compiler path and scratch home.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `app_name` | string | no | target name | Mix application name |
| `mix_env` | string | no | `prod` | Mix environment used for compilation and run tasks |
| `manifest` | string | no | `mix.exs` | Package-relative Mix project manifest |
| `lockfile` | string | no | `mix.lock` | Package-relative lockfile staged when present |
| `config` | list&lt;string&gt; | no | `["config/**/*.exs"]` | Configuration file globs included in the build |
| `config_files` | list&lt;string&gt; | no | `[]` | Additional configuration file globs |
| `data` | list&lt;string&gt; | no | `[]` | Data file globs available to project compilers |
| `priv` | list&lt;string&gt; | no | `[]` | Private application files copied into the application output |
| `resources` | list&lt;string&gt; | no | `[]` | Additional private application files |
| `include` | list&lt;string&gt; | no | `["include/**/*.hrl"]` | Erlang header files copied into the application output |
| `tools` | list&lt;string&gt; | no | `[]` | Workspace files read or executed by custom project compilers |
| `os_env` | map&lt;string, string&gt; | no | `{}` | Environment values exported during build and run actions |
| `env_inherit` | list&lt;string&gt; | no | `[]` | Host environment names inherited by build and run actions |
| `env` | map&lt;string, string&gt; | no | `{}` | Explicit environment values exported during build and run actions |
| `compile_args` | list&lt;string&gt; | no | `[]` | Additional arguments for the compiler pipeline |
| `run_task` | string | no | empty | Mix task exposed through `once run`; when empty, the caller may supply the task after `--` |
| `run_tasks` | list&lt;list&lt;string&gt;&gt; | no | `[]` | Ordered Mix tasks and their exact arguments; mutually exclusive with `run_task` |
| `run_no_compile` | bool | no | `true` | Pass `--no-compile` to the run task |
| `run_no_deps_check` | bool | no | `true` | Pass `--no-deps-check` to the run task |
| `run_args` | list&lt;string&gt; | no | `[]` | Arguments passed before caller-supplied arguments |
| `run_cacheable` | bool | no | `false` | Whether a successful run action can be restored from the action cache; uncached runs inherit their parent environment |

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `elixir_app` | Compiled applications loaded before the project compiler pipeline |

## Providers

The target emits `elixir_app` and `mix_project`. Its provider includes
`app_name`, `mix_env`, `mix_config`, `source_root`, `source_inputs`,
`build_env_dir`, `app_dir`, `ebin_dir`, `priv_dir`, `include_dir`,
`compile_metadata`, `compile_warnings`, `transitive_elixir_apps`, and
`transitive_sources`.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `bytecode` |
| `run` | `default`; requires `bytecode` |

## Example

```toml
[[target]]
name = "application"
kind = "mix_project"
srcs = ["lib/**/*"]
deps = ["./mix_dependencies"]

[target.attrs]
app_name = "greeting"
mix_env = "dev"
manifest = "mix.exs"
run_task = "phx.server"
```

The same target can leave `run_task` empty and select the task at invocation
time:

```sh
once run application -- phx.server
```

When `run_task` is set, arguments after `--` are appended to that task. A
target with `run_tasks` rejects caller-supplied arguments because it would be
ambiguous which task should receive them.

Use `run_no_compile = false` or `run_no_deps_check = false` only when the
selected Mix task rejects those switches or must perform its own checks.

For ordered checks, declare each task and its arguments as a nested list:

```toml
run_tasks = [
  ["deps.unlock", "--check-unused"],
  ["format", "--check-formatted"],
  ["credo"],
]
run_cacheable = true
```

## Sources

- [Mix compilation](https://hexdocs.pm/mix/Mix.Tasks.Compile.html) defines the
  registered compiler pipeline.
- [Mix project configuration](https://hexdocs.pm/mix/Mix.Project.html)
  documents application metadata, environments, and build paths.
