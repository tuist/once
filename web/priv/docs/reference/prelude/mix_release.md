# `mix_release`

Assembles a Mix release from a compiled `mix_project`.

## Description

`mix_release` depends on exactly one target that provides `mix_project`. It
stages the project manifest, lockfile, configuration, declared data and tools,
the compiled project environment, and separately cached dependency
applications into an isolated project.

Optional `pre_tasks` run as ordered, exact task argument vectors before release
assembly. The release task runs with compilation and dependency checks
disabled, so it consumes the graph outputs instead of rebuilding them. The
assembled release is cacheable by default.

Release artifacts are specific to the host operating system and processor
architecture used to assemble them. The release uses the `mix_env` selected by
its `mix_project` dependency. Use a production-environment project when the
artifact is intended for deployment.

## Attributes

| Attribute | Type | Required | Default | Description |
| --- | --- | --- | --- | --- |
| `manifest` | string | no | `mix.exs` | Package-relative Mix project manifest |
| `lockfile` | string | no | `mix.lock` | Package-relative lockfile staged when present |
| `release_name` | string | no | empty | Configured release name; the project default is used when omitted |
| `pre_tasks` | list&lt;list&lt;string&gt;&gt; | no | `[]` | Ordered Mix tasks and their exact arguments run before assembly |
| `release_args` | list&lt;string&gt; | no | `[]` | Additional arguments passed to `mix release` |
| `config` | list&lt;string&gt; | no | `["config/**/*.exs"]` | Configuration files staged into the isolated project |
| `config_files` | list&lt;string&gt; | no | `[]` | Additional configuration files |
| `data` | list&lt;string&gt; | no | `[]` | Project files available to pre-release tasks and assembly |
| `tools` | list&lt;string&gt; | no | `[]` | Executable or support files available to pre-release tasks |
| `os_env` | map&lt;string, string&gt; | no | `{}` | Environment values exported during assembly |
| `env_inherit` | list&lt;string&gt; | no | `[]` | Host environment names inherited by the action |
| `env` | map&lt;string, string&gt; | no | `{}` | Explicit environment values exported during assembly |
| `cacheable` | bool | no | `true` | Whether a successful release can be restored from the action cache |

## Dependency Edges

| Edge | Accepts | Description |
| --- | --- | --- |
| `deps` | `mix_project` | Exactly one compiled first-party Mix project |

## Providers

The target emits `mix_release` with the assembled `release_dir`, release log,
warning log, application name, and affected inputs.

## Capabilities

| Capability | Output groups |
| --- | --- |
| `build` | `default`, `release` |

## Example

```toml
[[target]]
name = "application_prod"
kind = "mix_project"
srcs = ["lib/**/*"]
deps = ["./dependencies_prod"]

[target.attrs]
app_name = "greeting"
mix_env = "prod"

[[target]]
name = "release"
kind = "mix_release"
srcs = ["mix.exs"]
deps = ["./application_prod"]

[target.attrs]
manifest = "mix.exs"
config = ["config/**/*.exs"]
pre_tasks = [
  ["assets.deploy"],
]
```

## Sources

- [Mix releases](https://hexdocs.pm/mix/Mix.Tasks.Release.html) documents
  release naming, configuration, assembly, and runtime behavior.
